use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use water_repair::{
    Export, PublicationCrash, PublicationFailure, canonical_plan_bytes, evaluate_candidate,
    evaluate_reference, generate_representative, generated_case, publish_plan,
    publish_plan_with_crash,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("water-repair: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next().ok_or_else(usage)?;
    let trailing: Vec<_> = arguments.collect();
    let flags = parse_flags(&trailing)?;
    match command.as_str() {
        "process" => process(&flags),
        "demo" => demo(&flags),
        "verify" => verify(&flags),
        "scale" => scale(&flags),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: water-repair <process|demo|verify|scale> [--input PATH] --output PATH [--batch-size N] [--inject before-temporary|before-final] [--crash before-staging|before-final]".to_string()
}

fn parse_flags(arguments: &[String]) -> Result<BTreeMap<String, String>, String> {
    if !arguments.len().is_multiple_of(2) {
        return Err(usage());
    }
    let mut flags = BTreeMap::new();
    for pair in arguments.chunks_exact(2) {
        let key = pair[0].strip_prefix("--").ok_or_else(usage)?.to_string();
        if flags.insert(key.clone(), pair[1].clone()).is_some() {
            return Err(format!("duplicate flag --{key}"));
        }
    }
    Ok(flags)
}

fn required_path(flags: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, String> {
    flags
        .get(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing --{name}"))
}

fn batch_size(flags: &BTreeMap<String, String>) -> Result<usize, String> {
    flags.get("batch-size").map_or(Ok(1_024), |value| {
        value
            .parse::<usize>()
            .map_err(|error| format!("invalid --batch-size: {error}"))
    })
}

fn injection(flags: &BTreeMap<String, String>) -> Result<PublicationFailure, String> {
    match flags.get("inject").map(String::as_str) {
        None => Ok(PublicationFailure::None),
        Some("before-temporary") => Ok(PublicationFailure::BeforeTemporary),
        Some("before-final") => Ok(PublicationFailure::BeforeFinalRename),
        Some(value) => Err(format!("unknown injection point: {value}")),
    }
}

fn crash_point(flags: &BTreeMap<String, String>) -> Result<PublicationCrash, String> {
    match flags.get("crash").map(String::as_str) {
        None => Ok(PublicationCrash::None),
        Some("before-staging") => Ok(PublicationCrash::BeforeStaging),
        Some("before-final") => Ok(PublicationCrash::BeforeFinalRename),
        Some(value) => Err(format!("unknown crash point: {value}")),
    }
}

fn read_export(path: &Path) -> Result<Export, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn process(flags: &BTreeMap<String, String>) -> Result<(), String> {
    let input = required_path(flags, "input")?;
    let output = required_path(flags, "output")?;
    let export = read_export(&input)?;
    let plan = evaluate_candidate(&export, batch_size(flags)?)?;
    let reference = evaluate_reference(&export)?;
    if plan != reference {
        return Err("candidate/reference mismatch; no report published".to_string());
    }
    let crash = crash_point(flags)?;
    if crash == PublicationCrash::None {
        publish_plan(Some(&input), &output, &plan, injection(flags)?)?;
    } else {
        if injection(flags)? != PublicationFailure::None {
            return Err("--inject and --crash are mutually exclusive".to_string());
        }
        publish_plan_with_crash(Some(&input), &output, &plan, crash)?;
    }
    println!(
        "published {} repair(s), {} review case(s) to {}",
        plan.repairs.len(),
        plan.review_cases.len(),
        output.display()
    );
    Ok(())
}

fn demo(flags: &BTreeMap<String, String>) -> Result<(), String> {
    let input = flags
        .get("input")
        .map_or_else(|| PathBuf::from("fixtures/disclosed.json"), PathBuf::from);
    let mut local_flags = flags.clone();
    local_flags.insert("input".to_string(), input.display().to_string());
    process(&local_flags)
}

fn verify(flags: &BTreeMap<String, String>) -> Result<(), String> {
    let fixture = flags
        .get("input")
        .map_or_else(|| PathBuf::from("fixtures/disclosed.json"), PathBuf::from);
    let disclosed = read_export(&fixture)?;
    let disclosed_plan = evaluate_candidate(&disclosed, 7)?;
    if disclosed_plan != evaluate_reference(&disclosed)? {
        return Err("disclosed fixture differs from chronological reference".to_string());
    }
    for seed in 0..100 {
        let export = generated_case(seed, 8);
        if evaluate_candidate(&export, 5)? != evaluate_reference(&export)? {
            return Err(format!("candidate/reference mismatch at seed {seed}"));
        }
    }
    let base = generated_case(42, 12);
    let expected = canonical_plan_bytes(&evaluate_candidate(&base, 1)?)?;
    for permutation in 0..10 {
        let mut shuffled = base.clone();
        permute(&mut shuffled.readings, permutation + 1);
        permute(&mut shuffled.installations, permutation + 101);
        permute(&mut shuffled.billed_intervals, permutation + 201);
        for batch in [1, 7, 64] {
            let actual = canonical_plan_bytes(&evaluate_candidate(&shuffled, batch)?)?;
            if actual != expected {
                return Err(format!(
                    "canonical mismatch at permutation {permutation}, batch {batch}"
                ));
            }
        }
    }
    println!(
        "verified disclosed fixture, 100 generated seeds, and 10 permutations x 3 batch sizes"
    );
    Ok(())
}

fn permute<T>(items: &mut [T], mut state: u64) {
    for index in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let target = usize::try_from(state % (index as u64 + 1)).expect("index always fits");
        items.swap(index, target);
    }
}

fn scale(flags: &BTreeMap<String, String>) -> Result<(), String> {
    let output = required_path(flags, "output")?;
    let started = Instant::now();
    let generated = generate_representative();
    let generated_at = started.elapsed();
    let readings = generated.readings.len();
    let service_points = generated
        .installations
        .iter()
        .map(|installation| installation.service_point_id)
        .max()
        .map_or(0, |maximum| maximum + 1);
    let meters = generated.installations.len();
    let bills = generated.billed_intervals.len();
    let plan = evaluate_candidate(&generated, batch_size(flags)?)?;
    let evaluated_at = started.elapsed();
    publish_plan(None, &output, &plan, PublicationFailure::None)?;
    let total = started.elapsed();
    println!(
        "best-case requested-count shape: {service_points} service points, {meters} meters, {readings} readings, {bills} bills"
    );
    println!(
        "result: {} repairs, {} review cases",
        plan.repairs.len(),
        plan.review_cases.len()
    );
    println!(
        "elapsed: generation={:.3}s evaluation={:.3}s total_with_publication={:.3}s",
        generated_at.as_secs_f64(),
        evaluated_at.saturating_sub(generated_at).as_secs_f64(),
        total.as_secs_f64()
    );
    println!(
        "host: os={} arch={} logical_parallelism={}; peak_rss_mib={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
        peak_rss_bytes().map_or_else(|| "unavailable".to_string(), format_mib)
    );
    Ok(())
}

#[cfg(unix)]
fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `getrusage` initializes the provided `rusage` when it returns
    // zero. The pointer is valid for writes and read only after that result.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    // SAFETY: a zero result from `getrusage` guarantees initialization.
    let usage = unsafe { usage.assume_init() };
    let raw = u64::try_from(usage.ru_maxrss).ok()?;
    #[cfg(target_os = "macos")]
    let bytes = raw;
    #[cfg(not(target_os = "macos"))]
    let bytes = raw.checked_mul(1_024)?;
    Some(bytes)
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

fn format_mib(bytes: u64) -> String {
    const MIB: u64 = 1_024 * 1_024;
    let whole = bytes / MIB;
    let tenth = (bytes % MIB) * 10 / MIB;
    format!("{whole}.{tenth}")
}
