use firewall_policy_impact::{
    Report, analyze_files_to_report, read_and_validate_pair, replay_samples, report_json,
    validation_suite, verify_report, write_benchmark_fixture,
};
use std::path::Path;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().collect();
    match arguments.get(1).map(String::as_str) {
        Some("analyze") if arguments.len() == 5 => {
            let old_path = Path::new(&arguments[2]);
            let proposed_path = Path::new(&arguments[3]);
            let report_path = Path::new(&arguments[4]);
            let report = analyze_files_to_report(old_path, proposed_path, report_path)?;
            println!(
                "outcome={} change_regions={} reachable_rules={} unreachable_rules={} report={}",
                report.summary.outcome,
                report.changes.len(),
                report.summary.proposed_reachable_rules,
                report.summary.proposed_unreachable_rules,
                report_path.display()
            );
            Ok(())
        }
        Some("verify") if arguments.len() == 5 => {
            let (old, proposed) =
                read_and_validate_pair(Path::new(&arguments[2]), Path::new(&arguments[3]))?;
            let bytes = std::fs::read(&arguments[4])
                .map_err(|error| format!("cannot read report {}: {error}", arguments[4]))?;
            let report: Report = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid report JSON or schema: {error}"))?;
            let (change_witnesses, reachability_witnesses) =
                verify_report(&old, &proposed, &report)?;
            println!(
                "verified exact_report=true change_witnesses={change_witnesses} reachability_witnesses={reachability_witnesses}"
            );
            Ok(())
        }
        Some("sample-replay") if arguments.len() == 5 => {
            let (old, proposed) =
                read_and_validate_pair(Path::new(&arguments[2]), Path::new(&arguments[3]))?;
            let sample_bytes = std::fs::read(&arguments[4])
                .map_err(|error| format!("cannot read samples {}: {error}", arguments[4]))?;
            let replay = replay_samples(&old, &proposed, &sample_bytes)?;
            let mut output =
                serde_json::to_vec_pretty(&replay).map_err(|error| error.to_string())?;
            output.push(b'\n');
            print!("{}", String::from_utf8(output).unwrap());
            Ok(())
        }
        Some("generate-benchmark") if arguments.len() == 5 => {
            let rule_count = parse_usize(&arguments[3], "rule count")?;
            let seed = parse_u64(&arguments[4], "seed")?;
            write_benchmark_fixture(Path::new(&arguments[2]), rule_count, seed)?;
            println!(
                "generated old_rules={rule_count} proposed_rules={rule_count} seed={seed} directory={}",
                arguments[2]
            );
            Ok(())
        }
        Some("validate-suite") if arguments.len() == 6 => {
            let small_pairs = parse_usize(&arguments[2], "small-pair count")?;
            let full_pairs = parse_usize(&arguments[3], "full-width pair count")?;
            let probes = parse_usize(&arguments[4], "probe count")?;
            let seed = parse_u64(&arguments[5], "seed")?;
            if full_pairs == 0 {
                return Err("full-width pair count must be greater than zero".to_string());
            }
            println!(
                "{}",
                validation_suite(small_pairs, full_pairs, probes, seed)?
            );
            Ok(())
        }
        Some("canonicalize-report") if arguments.len() == 3 => {
            let bytes = std::fs::read(&arguments[2])
                .map_err(|error| format!("cannot read report {}: {error}", arguments[2]))?;
            let report: Report = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid report JSON or schema: {error}"))?;
            print!("{}", String::from_utf8(report_json(&report)?).unwrap());
            Ok(())
        }
        _ => Err(usage()),
    }
}

fn parse_usize(text: &str, name: &str) -> Result<usize, String> {
    text.parse().map_err(|_| format!("invalid {name} {text:?}"))
}

fn parse_u64(text: &str, name: &str) -> Result<u64, String> {
    text.parse().map_err(|_| format!("invalid {name} {text:?}"))
}

fn usage() -> String {
    "usage:\n  firewall-policy-impact analyze OLD.json PROPOSED.json REPORT.json\n  firewall-policy-impact verify OLD.json PROPOSED.json REPORT.json\n  firewall-policy-impact sample-replay OLD.json PROPOSED.json SAMPLES.json\n  firewall-policy-impact generate-benchmark DIRECTORY RULE_COUNT SEED\n  firewall-policy-impact validate-suite SMALL_PAIRS FULL_PAIRS TOTAL_PROBES SEED\n  firewall-policy-impact canonicalize-report REPORT.json".to_string()
}
