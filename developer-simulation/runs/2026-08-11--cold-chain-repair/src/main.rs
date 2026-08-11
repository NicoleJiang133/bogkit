use std::error::Error;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use cold_chain_repair::{
    Observation, Record, candidate_from_records, canonical_json, generated_fixture, read_ndjson,
    reference_from_records, shuffled, write_ndjson,
};
use fold::pipeline::terminal;
use fold::stream::KeyedStream;

const INJECTED_EXIT: i32 = 86;
const NEXT_SNAPSHOT: &[u8] = b"complete-next-state\n";
const BENCHMARK_FREEZERS: usize = 400;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("generate") => {
            let archive = required_path(&args, 2)?;
            let seed = required_number(&args, 3, "seed")?;
            let freezers = required_number(&args, 4, "freezer count")?;
            let readings = required_number(&args, 5, "readings per freezer")?;
            generate(&archive, seed, freezers, readings)
        }
        Some("repair") => {
            let archive = required_path(&args, 2)?;
            let state = required_path(&args, 3)?;
            repair(&archive, &state)
        }
        Some("demo") => demo(args.get(2).map(PathBuf::from)),
        Some("verify") => {
            let seeds = args.get(2).map_or(Ok(100_usize), |value| value.parse())?;
            verify(seeds, args.get(3).map(PathBuf::from))
        }
        Some("benchmark") => {
            let observations = args
                .get(2)
                .map_or(Ok(1_000_000_usize), |value| value.parse())?;
            benchmark(observations, args.get(3).map(PathBuf::from))
        }
        Some("benchmark-reference") => {
            let observations = args
                .get(2)
                .map_or(Ok(1_000_000_usize), |value| value.parse())?;
            benchmark_reference(observations, args.get(3).map(PathBuf::from))
        }
        Some("fault-demo") => {
            let root = args.get(2).map_or_else(
                || std::env::temp_dir().join("cold-chain-fault-demo"),
                PathBuf::from,
            );
            fault_demo(&root)
        }
        Some("__exit-index-transaction") => {
            let path = required_path(&args, 2)?;
            exit_inside_index_transaction(&path)
        }
        Some("__exit-snapshot") => {
            let position = args.get(2).ok_or("missing snapshot fault position")?;
            let path = required_path(&args, 3)?;
            exit_during_snapshot(position, &path)
        }
        _ => Err("usage: cold-chain-repair <generate|repair|demo|verify|benchmark|benchmark-reference|fault-demo> [arguments]".into()),
    }
}

fn required_number<T>(args: &[String], position: usize, label: &str) -> Result<T, Box<dyn Error>>
where
    T: std::str::FromStr,
    T::Err: Error + 'static,
{
    args.get(position)
        .ok_or_else(|| format!("missing {label}").into())
        .and_then(|value| value.parse().map_err(Into::into))
}

fn required_path(args: &[String], position: usize) -> Result<PathBuf, Box<dyn Error>> {
    args.get(position)
        .map(PathBuf::from)
        .ok_or_else(|| "missing path argument".into())
}

fn runtime_root(given: Option<PathBuf>, label: &str) -> PathBuf {
    given.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("cold-chain-{label}-{}", std::process::id()))
    })
}

fn generate(
    archive: &Path,
    seed: u64,
    freezers: usize,
    readings: usize,
) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = archive.parent() {
        fs::create_dir_all(parent)?;
    }
    let records = generated_fixture(seed, freezers, readings);
    write_ndjson(&records, BufWriter::new(File::create(archive)?))?;
    println!(
        "generated {} records at {}",
        records.len(),
        archive.display()
    );
    Ok(())
}

fn repair(archive: &Path, state: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(state)?;
    let records = read_ndjson(BufReader::new(File::open(archive)?))?;
    let export = candidate_from_records(&records, &state.join("fold-index"))?;
    let bytes = canonical_json(&export)?;
    write_snapshot(&state.join("incidents.json"), &bytes)?;
    println!(
        "repaired {} records into {}",
        records.len(),
        state.join("incidents.json").display()
    );
    Ok(())
}

fn demo(given: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let root = runtime_root(given, "demo");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root)?;
    let archive = root.join("archive.ndjson");
    generate(&archive, 7, 2, 96)?;
    let records = read_ndjson(BufReader::new(File::open(&archive)?))?;
    let expected = reference_from_records(&records)?;
    let candidate = candidate_from_records(&records, &root.join("fold-index"))?;
    if candidate != expected {
        return Err("candidate differs from the independent reducer".into());
    }
    let bytes = canonical_json(&candidate)?;
    write_snapshot(&root.join("incidents.json"), &bytes)?;
    print!("{}", String::from_utf8(bytes)?);
    println!("demo: candidate exactly matched independent reference");
    Ok(())
}

fn verify(seeds: usize, given: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let root = runtime_root(given, "verify");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root)?;

    for seed in 0..seeds {
        let seed_u64 = u64::try_from(seed)?;
        let records = generated_fixture(seed_u64, 3, 96);
        let expected = reference_from_records(&records)?;
        let shuffled_records = shuffled(&records, seed_u64 ^ 0x5eed_5eed);

        let shuffled_path = root.join("shuffled");
        let candidate = candidate_from_records(&shuffled_records, &shuffled_path)?;
        if candidate != expected {
            return Err(format!("shuffled seed {seed} differs from reference").into());
        }
        fs::remove_dir_all(&shuffled_path)?;

        let mut repeated = shuffled_records.clone();
        repeated.extend(shuffled_records.iter().take(37).cloned());
        let repeated_path = root.join("repeated");
        let candidate = candidate_from_records(&repeated, &repeated_path)?;
        if candidate != expected {
            return Err(format!("repeated seed {seed} differs from reference").into());
        }
        fs::remove_dir_all(&repeated_path)?;
    }

    println!(
        "differential seeds: {seeds} passed (chronological reference, shuffled and repeated candidate batches)"
    );
    Ok(())
}

fn benchmark(observations: usize, given: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let root = runtime_root(given, "benchmark");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root)?;
    let (records, seen_observations) = sized_benchmark_fixture(observations);
    let started = Instant::now();
    let export = candidate_from_records(&records, &root.join("fold-index"))?;
    let elapsed = started.elapsed();
    write_snapshot(&root.join("incidents.json"), &canonical_json(&export)?)?;
    println!(
        "benchmark: {seen_observations} observations, {BENCHMARK_FREEZERS} freezers, {:.3}s elapsed",
        elapsed.as_secs_f64()
    );
    Ok(())
}

fn benchmark_reference(observations: usize, given: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let root = runtime_root(given, "benchmark-reference");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root)?;
    let (records, seen_observations) = sized_benchmark_fixture(observations);
    let started = Instant::now();
    let export = reference_from_records(&records)?;
    let elapsed = started.elapsed();
    write_snapshot(&root.join("incidents.json"), &canonical_json(&export)?)?;
    println!(
        "reference benchmark: {seen_observations} observations, {BENCHMARK_FREEZERS} freezers, {:.3}s elapsed",
        elapsed.as_secs_f64()
    );
    Ok(())
}

fn sized_benchmark_fixture(observations: usize) -> (Vec<Record>, usize) {
    let freezers = BENCHMARK_FREEZERS;
    let readings_per_freezer = observations.div_ceil(freezers);
    let mut records = generated_fixture(91, freezers, readings_per_freezer);
    let mut seen_observations = 0_usize;
    records.retain(|record| match record {
        Record::Observation(_) if seen_observations < observations => {
            seen_observations += 1;
            true
        }
        Record::Observation(_) => false,
        Record::Configuration(_) => true,
    });
    (records, seen_observations)
}

fn fault_demo(root: &Path) -> Result<(), Box<dyn Error>> {
    let _ = fs::remove_dir_all(root);
    fs::create_dir_all(root)?;
    let index_path = root.join("fold-index");
    let old_records = generated_fixture(17, 2, 48);
    let old_export = reference_from_records(&old_records)?;
    if candidate_from_records(&old_records, &index_path)? != old_export {
        return Err("initial index differs from reference".into());
    }

    let current_exe = std::env::current_exe()?;
    let status = Command::new(&current_exe)
        .arg("__exit-index-transaction")
        .arg(&index_path)
        .status()?;
    if status.code() != Some(INJECTED_EXIT) {
        return Err(format!("unexpected index fault status {status}").into());
    }
    if candidate_from_records(&old_records, &index_path)? != old_export {
        return Err("uncommitted index transaction became visible".into());
    }

    let mut next_records = old_records.clone();
    next_records.push(Record::Observation(Observation {
        observation_id: "restart-new-reading".into(),
        freezer_id: "freezer-000".into(),
        observed_at: 48 * 30,
        received_at: 48 * 30 + 60,
        temp_tenths: -200,
        gateway_sequence: 49,
        replaces_observation_id: None,
    }));
    let next_export = reference_from_records(&next_records)?;
    if candidate_from_records(&next_records, &index_path)? != next_export {
        return Err("restart did not converge after index exit".into());
    }

    let snapshot_path = root.join("incidents.json");
    let old_snapshot = b"complete-prior-state\n";
    write_snapshot(&snapshot_path, old_snapshot)?;
    let status = Command::new(&current_exe)
        .arg("__exit-snapshot")
        .arg("before")
        .arg(&snapshot_path)
        .status()?;
    if status.code() != Some(INJECTED_EXIT) || fs::read(&snapshot_path)? != old_snapshot {
        return Err("exit before rename did not preserve prior snapshot".into());
    }
    write_snapshot(&snapshot_path, NEXT_SNAPSHOT)?;
    if fs::read(&snapshot_path)? != NEXT_SNAPSHOT {
        return Err("restart did not converge snapshot".into());
    }

    write_snapshot(&snapshot_path, old_snapshot)?;
    let status = Command::new(&current_exe)
        .arg("__exit-snapshot")
        .arg("after")
        .arg(&snapshot_path)
        .status()?;
    if status.code() != Some(INJECTED_EXIT) || fs::read(&snapshot_path)? != NEXT_SNAPSHOT {
        return Err("exit after rename did not preserve next snapshot".into());
    }
    println!(
        "fault boundaries: passed (uncommitted index, before rename, after rename, restart convergence)"
    );
    Ok(())
}

fn exit_inside_index_transaction(path: &Path) -> ! {
    let mut index = KeyedStream::new(path, terminal::Table::new("accepted_records"));
    index.wtx(|transaction| {
        let crash_only = Record::Observation(Observation {
            observation_id: "must-not-commit".into(),
            freezer_id: "freezer-without-config".into(),
            observed_at: 0,
            received_at: 0,
            temp_tenths: -200,
            gateway_sequence: 0,
            replaces_observation_id: None,
        });
        transaction.upsert(&"observation:must-not-commit".to_string(), &crash_only);
        std::process::exit(INJECTED_EXIT);
    })
}

fn exit_during_snapshot(position: &str, path: &Path) -> Result<(), Box<dyn Error>> {
    let parent = path.parent().ok_or("snapshot must have a parent")?;
    fs::create_dir_all(parent)?;
    let temporary = temporary_snapshot_path(path);
    let mut file = File::create(&temporary)?;
    file.write_all(NEXT_SNAPSHOT)?;
    file.sync_all()?;
    if position == "before" {
        std::process::exit(INJECTED_EXIT);
    }
    fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()?;
    if position == "after" {
        std::process::exit(INJECTED_EXIT);
    }
    Err("snapshot position must be before or after".into())
}

fn write_snapshot(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let parent = path.parent().ok_or("snapshot must have a parent")?;
    fs::create_dir_all(parent)?;
    let temporary = temporary_snapshot_path(path);
    let mut file = File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn temporary_snapshot_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".next");
    PathBuf::from(name)
}
