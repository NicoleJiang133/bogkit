use std::env;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use return_reconciler_trial1::{
    FailPoint, Report, Snapshot, deterministic_shuffle, generate_representative, reconcile,
    reconcile_file, reference_reconcile, verify_report,
};

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: &[String]) -> Result<(), String> {
    match arguments {
        [command, input, output] if command == "reconcile" => {
            let report = reconcile_file(Path::new(input), Path::new(output), FailPoint::None)?;
            println!(
                "published advisory report: {} returns, {} review, {} proposed cents",
                report.summary.return_count,
                report.summary.review_returns,
                report.summary.proposed_cents
            );
            Ok(())
        }
        [command, phase, input, output] if command == "reconcile-exit" => {
            let failpoint = match phase.as_str() {
                "before-output" => FailPoint::ExitBeforeOutputCreation,
                "after-temp" => FailPoint::ExitAfterTemporaryComplete,
                "before-publish" => FailPoint::ExitBeforePublish,
                _ => return Err("unknown exit phase".to_string()),
            };
            let _ = reconcile_file(Path::new(input), Path::new(output), failpoint)?;
            Err("exit injection did not fire".to_string())
        }
        [command, input, report] if command == "verify" => {
            let snapshot: Snapshot = read_json(Path::new(input))?;
            let report: Report = read_json(Path::new(report))?;
            verify_report(&snapshot, &report)?;
            println!("verified advisory report for {}", snapshot.snapshot_id);
            Ok(())
        }
        [command, output] if command == "generate-representative" => {
            write_json(Path::new(output), &generate_representative(20_260_813))?;
            println!("generated disclosed representative snapshot at {output}");
            Ok(())
        }
        [command] if command == "benchmark" => benchmark(),
        _ => Err("usage: return-reconciler-trial1 reconcile <snapshot.json> <report.json> | verify <snapshot.json> <report.json> | generate-representative <snapshot.json> | benchmark".to_string()),
    }
}

fn benchmark() -> Result<(), String> {
    let generated_at = Instant::now();
    let snapshot = generate_representative(20_260_813);
    let generation_ms = generated_at.elapsed().as_millis();

    let candidate_at = Instant::now();
    let expected = reconcile(&snapshot)?;
    let candidate_ms = candidate_at.elapsed().as_millis();

    let reference_at = Instant::now();
    let reference = reference_reconcile(&snapshot)?;
    let reference_ms = reference_at.elapsed().as_millis();
    if expected != reference {
        return Err("candidate differs from independent reference".to_string());
    }

    let expected_bytes = serde_json::to_vec(&expected).map_err(|error| error.to_string())?;
    let shuffles_at = Instant::now();
    for seed in 0..10 {
        let mut shuffled = snapshot.clone();
        deterministic_shuffle(&mut shuffled, seed);
        let actual =
            serde_json::to_vec(&reconcile(&shuffled)?).map_err(|error| error.to_string())?;
        if actual != expected_bytes {
            return Err(format!("shuffle {seed} changed report bytes"));
        }
    }
    let shuffles_ms = shuffles_at.elapsed().as_millis();
    println!(
        "representative_ok returns={} lines={} events={} adversarial=500 generation_ms={} candidate_ms={} reference_ms={} ten_shuffles_ms={} report_bytes={}",
        snapshot.returns.len(),
        snapshot
            .returns
            .iter()
            .map(|authorization| authorization.lines.len())
            .sum::<usize>(),
        snapshot.events.len(),
        generation_ms,
        candidate_ms,
        reference_ms,
        shuffles_ms,
        expected_bytes.len()
    );
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    serde_json::from_reader(BufReader::new(file)).map_err(|error| error.to_string())
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if path.exists() {
        return Err(format!("refusing to overwrite {}", path.display()));
    }
    let file = File::create(path).map_err(|error| error.to_string())?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, value).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())?;
    fs::metadata(path).map_err(|error| error.to_string())?;
    Ok(())
}
