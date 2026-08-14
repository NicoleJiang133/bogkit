use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use undo_history_lab::baseline::Baseline;
use undo_history_lab::candidate::{Candidate, compact_directory};
use undo_history_lab::fixture::{Fixture, generate_fixture};
use undo_history_lab::runner::run_fixture;
use undo_history_lab::{Command, Group, Object};

fn main() {
    if let Err(error) = dispatch() {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
}

fn dispatch() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments.next().ok_or_else(usage)?;
    match mode.as_str() {
        "generate" => {
            let output = arguments.next().ok_or_else(usage)?;
            let seed = optional_parse(arguments.next(), 1_u64)?;
            let objects = optional_parse(arguments.next(), 60_000_usize)?;
            let actions = optional_parse(arguments.next(), 250_000_usize)?;
            ensure_no_more(arguments)?;
            let fixture = generate_fixture(seed, objects, actions)?;
            write_json(Path::new(&output), &fixture)?;
            println!(
                "{}",
                serde_json::json!({
                    "mode": "generate",
                    "seed": seed,
                    "objects": objects,
                    "actions": actions,
                    "output": output,
                    "expected_final": fixture.expected_final,
                })
            );
        }
        "run" => {
            let fixture_path = arguments.next().ok_or_else(usage)?;
            let output = arguments.next().ok_or_else(usage)?;
            ensure_no_more(arguments)?;
            let fixture: Fixture = serde_json::from_slice(&std::fs::read(fixture_path)?)?;
            let report = run_fixture(&fixture, output)?;
            println!("{}", report.canonical_json);
            println!("{}", serde_json::to_string(&report)?);
        }
        "recover" | "inspect" => {
            let kind = arguments.next().ok_or_else(usage)?;
            let store = arguments.next().ok_or_else(usage)?;
            ensure_no_more(arguments)?;
            inspect_store(mode.as_str(), &kind, Path::new(&store))?;
        }
        "compact" => {
            let kind = arguments.next().ok_or_else(usage)?;
            let store = arguments.next().ok_or_else(usage)?;
            let keep = optional_parse(arguments.next(), 20_000_usize)?;
            ensure_no_more(arguments)?;
            let status = match kind.as_str() {
                "candidate" => compact_directory(&store, keep)?,
                "baseline" => {
                    let mut baseline = Baseline::open(&store, 2_000)?;
                    baseline.compact(keep)?
                }
                _ => return Err(usage().into()),
            };
            println!(
                "{}",
                serde_json::json!({ "mode": "compact", "kind": kind, "keep": keep, "status": status })
            );
        }
        "fault-commit" => {
            let kind = arguments.next().ok_or_else(usage)?;
            let store = arguments.next().ok_or_else(usage)?;
            ensure_no_more(arguments)?;
            let group = fault_group();
            match kind.as_str() {
                "candidate" => {
                    Candidate::open(&store)?.commit(group)?;
                }
                "baseline" => {
                    Baseline::open(&store, 2_000)?.commit(group)?;
                }
                _ => return Err(usage().into()),
            }
            std::process::exit(86);
        }
        _ => return Err(usage().into()),
    }
    Ok(())
}

fn inspect_store(mode: &str, kind: &str, store: &Path) -> Result<(), Box<dyn Error>> {
    match kind {
        "candidate" => {
            let candidate = Candidate::open(store)?;
            println!("{}", candidate.canonical_json()?);
            println!(
                "{}",
                serde_json::json!({
                    "mode": mode,
                    "kind": kind,
                    "status": candidate.status()?,
                    "diagnostic": null,
                })
            );
        }
        "baseline" => {
            let baseline = Baseline::open(store, 2_000)?;
            println!("{}", baseline.canonical_json()?);
            println!(
                "{}",
                serde_json::json!({
                    "mode": mode,
                    "kind": kind,
                    "status": baseline.status()?,
                    "diagnostic": baseline.diagnostic(),
                })
            );
        }
        _ => return Err(usage().into()),
    }
    Ok(())
}

fn fault_group() -> Group {
    Group::new(
        "fault-group",
        vec![Command::Create {
            object: Object {
                id: 0xf00d,
                z: 0,
                transform: [1_000, 0, 0, 1_000, 0, 0],
                visible: true,
                fill: 0xff,
                points: vec![],
            },
        }],
    )
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), Box<dyn Error>> {
    let bytes = serde_json::to_vec(value)?;
    let mut file = File::create(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn optional_parse<T: std::str::FromStr>(
    value: Option<String>,
    default: T,
) -> Result<T, Box<dyn Error>>
where
    T::Err: Error + 'static,
{
    value.map_or(Ok(default), |text| text.parse().map_err(Into::into))
}

fn ensure_no_more(mut arguments: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    if arguments.next().is_some() {
        Err(usage().into())
    } else {
        Ok(())
    }
}

fn usage() -> &'static str {
    "usage: undo-history-lab generate <fixture.json> [seed] [objects] [actions] | run <fixture.json> <output-dir> | recover <candidate|baseline> <store> | compact <candidate|baseline> <store> [keep] | inspect <candidate|baseline> <store>"
}
