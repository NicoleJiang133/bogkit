use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::Instant;

use causal_canvas_compaction::{
    ApplyOutcome, ClientWatermark, Core, Limits, Operation, Payload, open_latest,
    publish_generation, run_reduced_oracle_suite, validate_ndjson_line,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().collect();
    match arguments.get(1).map(String::as_str) {
        Some("replay") => replay_command(&arguments[2..]),
        Some("oracle") => oracle_command(&arguments[2..]),
        Some("workload") => workload_command(&arguments[2..]),
        Some("largest") => largest_command(&arguments[2..]),
        _ => Err(
            "usage: causal-canvas-compaction <replay INPUT STATE DOC NOW|oracle [HISTORIES SCHEDULES ACTORS]|workload [OPERATIONS]|largest OBJECTS ELEMENTS STATE>"
                .to_string(),
        ),
    }
}

fn replay_command(arguments: &[String]) -> Result<(), String> {
    let [input, state, document_id, now_minute] = arguments else {
        return Err("replay requires INPUT STATE DOC NOW".to_string());
    };
    let now_minute = now_minute
        .parse::<u64>()
        .map_err(|_| "NOW must be an integer minute".to_string())?;
    let limits = Limits::default();
    let mut core = Core::with_limits(limits);
    let mut accepted = 0usize;
    read_bounded_lines(
        &Path::new(input).join("operations.ndjson"),
        limits.max_line_bytes,
        |line| {
            let operation = validate_ndjson_line(line, limits)
                .map_err(|error| format!("operation validation error: {error:?}"))?;
            match core.apply(operation) {
                ApplyOutcome::Applied { .. } | ApplyOutcome::Buffered => {
                    accepted += 1;
                    Ok(())
                }
                ApplyOutcome::Duplicate => Ok(()),
                ApplyOutcome::Rejected(error) => Err(format!("apply error: {error:?}")),
            }
        },
    )?;
    let pending = core.pending_diagnosis();
    if pending.pending_operations != 0 {
        return Err(format!(
            "{} operations remain causally blocked; cycle={}",
            pending.pending_operations, pending.has_cycle
        ));
    }

    let watermarks_bytes = read_small_file(&Path::new(input).join("watermarks.json"), 1_048_576)?;
    let watermarks: Vec<ClientWatermark> = serde_json::from_slice(&watermarks_bytes)
        .map_err(|error| format!("watermark JSON error: {error}"))?;
    let crash_bytes = read_small_file(&Path::new(input).join("crash_schedules.json"), 1_048_576)?;
    let _: Vec<serde_json::Value> = serde_json::from_slice(&crash_bytes)
        .map_err(|error| format!("crash schedule JSON error: {error}"))?;

    let artifact = core
        .compact(document_id, &watermarks, now_minute)
        .map_err(|error| format!("compaction error: {error}"))?;
    publish_generation(Path::new(state), 1, &artifact, None)
        .map_err(|error| format!("publication error: {error}"))?;
    println!(
        "{{\"accepted_operations\":{accepted},\"pending_operations\":0,\"snapshot_digest\":\"{}\",\"uncompacted_bytes\":{},\"compacted_bytes\":{}}}",
        causal_canvas_compaction::stable_digest(artifact.snapshot_json.as_bytes()),
        artifact.uncompacted_bytes,
        artifact.compacted_bytes
    );
    Ok(())
}

fn oracle_command(arguments: &[String]) -> Result<(), String> {
    let histories = parse_or(arguments.first(), 50)?;
    let schedules = parse_or(arguments.get(1), 20)?;
    let actors = parse_or(arguments.get(2), 64)?;
    let summary = run_reduced_oracle_suite(histories, schedules, actors);
    println!(
        "{{\"histories\":{},\"schedules_per_history\":{},\"comparisons\":{},\"failures\":{},\"digest\":\"{}\"}}",
        summary.histories,
        summary.schedules_per_history,
        summary.comparisons,
        summary.failures,
        summary.digest
    );
    if summary.failures == 0 {
        Ok(())
    } else {
        Err("reduced oracle comparisons failed".to_string())
    }
}

fn workload_command(arguments: &[String]) -> Result<(), String> {
    let operation_count = parse_or(arguments.first(), 100_000)?;
    if operation_count < 1 {
        return Err("operation count must be positive".to_string());
    }
    let started = Instant::now();
    let mut core = Core::new();
    let mut batch_latencies = Vec::new();
    let mut batch_started = Instant::now();
    for sequence in 1..=operation_count {
        let sequence = u64::try_from(sequence).map_err(|_| "operation count too large")?;
        let payload = if sequence == 1 {
            Payload::Create {
                object_id: "shape".to_string(),
                object_kind: "rectangle".to_string(),
            }
        } else {
            Payload::SetProperty {
                object_id: "shape".to_string(),
                key: "value".to_string(),
                value: sequence.to_string(),
            }
        };
        let dependency_clock = if sequence == 1 {
            BTreeMap::new()
        } else {
            BTreeMap::from([("actor".to_string(), sequence - 1)])
        };
        let outcome = core.apply(Operation {
            id: format!("actor-{sequence}"),
            document_id: "workload".to_string(),
            actor_id: "actor".to_string(),
            actor_sequence: sequence,
            dependency_clock,
            accepted_minute: sequence,
            payload,
        });
        if !matches!(outcome, ApplyOutcome::Applied { .. }) {
            return Err(format!("unexpected workload apply result: {outcome:?}"));
        }
        if sequence % 200 == 0 {
            batch_latencies.push(batch_started.elapsed().as_micros());
            batch_started = Instant::now();
        }
    }
    batch_latencies.sort_unstable();
    let warmed = batch_latencies.get(5..).unwrap_or(&batch_latencies);
    let p95_index = warmed.len().saturating_sub(1) * 95 / 100;
    let p95_micros = warmed.get(p95_index).copied().unwrap_or(0);
    let elapsed_millis = started.elapsed().as_millis();
    let digest = core
        .snapshot_digest("workload")
        .map_err(|error| format!("snapshot error: {error}"))?;
    println!(
        "{{\"operations\":{operation_count},\"elapsed_ms\":{elapsed_millis},\"batch_200_p95_us\":{p95_micros},\"snapshot_digest\":\"{digest}\"}}"
    );
    Ok(())
}

fn largest_command(arguments: &[String]) -> Result<(), String> {
    let [object_count, element_count, state] = arguments else {
        return Err("largest requires OBJECTS ELEMENTS STATE".to_string());
    };
    let object_count = object_count
        .parse::<usize>()
        .map_err(|_| "OBJECTS must be an integer".to_string())?;
    let element_count = element_count
        .parse::<usize>()
        .map_err(|_| "ELEMENTS must be an integer".to_string())?;
    let build_started = Instant::now();
    let mut core = Core::new();
    let mut sequence = 0_u64;
    for index in 0..object_count {
        sequence = sequence
            .checked_add(1)
            .ok_or_else(|| "sequence overflow".to_string())?;
        apply_generated(
            &mut core,
            sequence,
            Payload::Create {
                object_id: format!("object-{index:06}"),
                object_kind: "rectangle".to_string(),
            },
        )?;
    }
    for index in 0..element_count {
        sequence = sequence
            .checked_add(1)
            .ok_or_else(|| "sequence overflow".to_string())?;
        apply_generated(
            &mut core,
            sequence,
            Payload::ListInsert {
                list_id: "layers".to_string(),
                element_id: format!("element-{index:06}"),
                left: index
                    .checked_sub(1)
                    .map(|previous| format!("element-{previous:06}")),
                value: format!("object-{:06}", index % object_count.max(1)),
            },
        )?;
    }
    let build_millis = build_started.elapsed().as_millis();
    let compact_started = Instant::now();
    let artifact = core
        .compact("largest", &[], sequence)
        .map_err(|error| format!("compaction error: {error}"))?;
    let compact_millis = compact_started.elapsed().as_millis();
    let publish_started = Instant::now();
    publish_generation(Path::new(state), 1, &artifact, None)
        .map_err(|error| format!("publication error: {error}"))?;
    let publish_millis = publish_started.elapsed().as_millis();
    let open_started = Instant::now();
    let reopened = open_latest(Path::new(state)).map_err(|error| format!("open error: {error}"))?;
    let open_millis = open_started.elapsed().as_millis();
    let digest = reopened
        .core
        .snapshot_digest("largest")
        .map_err(|error| format!("snapshot error: {error}"))?;
    println!(
        "{{\"objects\":{object_count},\"list_elements\":{element_count},\"build_ms\":{build_millis},\"compact_ms\":{compact_millis},\"publish_ms\":{publish_millis},\"open_ms\":{open_millis},\"compacted_bytes\":{},\"snapshot_digest\":\"{digest}\"}}",
        artifact.compacted_bytes
    );
    Ok(())
}

fn apply_generated(core: &mut Core, sequence: u64, payload: Payload) -> Result<(), String> {
    let dependency_clock = if sequence == 1 {
        BTreeMap::new()
    } else {
        BTreeMap::from([("generator".to_string(), sequence - 1)])
    };
    let outcome = core.apply(Operation {
        id: format!("generator-{sequence}"),
        document_id: "largest".to_string(),
        actor_id: "generator".to_string(),
        actor_sequence: sequence,
        dependency_clock,
        accepted_minute: sequence,
        payload,
    });
    if matches!(outcome, ApplyOutcome::Applied { .. }) {
        Ok(())
    } else {
        Err(format!("unexpected generated apply result: {outcome:?}"))
    }
}

fn parse_or(value: Option<&String>, default: usize) -> Result<usize, String> {
    value.map_or(Ok(default), |value| {
        value
            .parse::<usize>()
            .map_err(|_| format!("invalid integer: {value}"))
    })
}

fn read_small_file(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.len() > maximum {
        return Err(format!("{} exceeds {maximum} bytes", path.display()));
    }
    fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn read_bounded_lines(
    path: &Path,
    maximum: usize,
    mut on_line: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    loop {
        let mut line = Vec::with_capacity(maximum.min(8_192));
        let reached_eof = loop {
            let available = reader
                .fill_buf()
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if available.is_empty() {
                break true;
            }
            let take = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if line.len().saturating_add(take) > maximum.saturating_add(1) {
                return Err(format!("{} contains an oversized line", path.display()));
            }
            let has_newline = available[take - 1] == b'\n';
            line.extend_from_slice(&available[..take]);
            reader.consume(take);
            if has_newline {
                break false;
            }
        };
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if !line.is_empty() {
            on_line(&line)?;
        }
        if reached_eof {
            break;
        }
    }
    Ok(())
}
