use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mailbox_mirror_lab::{
    GeneratorConfig, SqliteBaseline, apply_transcript, generate_transcript, verify_transcript,
};
use serde::Serialize;

#[derive(Parser)]
#[command(about = "Deterministic epoch-safe mailbox mirror laboratory")]
struct Cli {
    #[command(subcommand)]
    command: HarnessCommand,
}

#[derive(Subcommand)]
enum HarnessCommand {
    Generate {
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, default_value_t = 80)]
        mailboxes: usize,
        #[arg(long, default_value_t = 300_000)]
        messages: usize,
        #[arg(long, default_value_t = 600_000)]
        responses: usize,
    },
    Apply {
        #[arg(long)]
        transcript: PathBuf,
        #[arg(long)]
        database: PathBuf,
    },
    Resume {
        #[arg(long)]
        transcript: PathBuf,
        #[arg(long)]
        database: PathBuf,
    },
    Verify {
        #[arg(long)]
        transcript: PathBuf,
        #[arg(long)]
        database: PathBuf,
    },
    Summarize {
        #[arg(long)]
        database: PathBuf,
    },
}

#[derive(Serialize)]
struct ManifestSummary {
    live_messages: usize,
    manifest: mailbox_mirror_lab::Manifest,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        HarnessCommand::Generate {
            output,
            seed,
            mailboxes,
            messages,
            responses,
        } => {
            let file = File::create(&output).map_err(|_| "cannot create transcript".to_owned())?;
            let mut writer = BufWriter::new(file);
            let summary = generate_transcript(
                &GeneratorConfig {
                    seed,
                    mailboxes,
                    live_messages: messages,
                    responses,
                },
                &mut writer,
            )
            .map_err(|error| error.to_string())?;
            writer
                .flush()
                .map_err(|_| "cannot flush transcript".to_owned())?;
            print_json(&summary)
        }
        HarnessCommand::Apply {
            transcript,
            database,
        } => {
            if database.exists() {
                return Err("database already exists; use resume".to_owned());
            }
            let reader = open_transcript(&transcript)?;
            let summary =
                apply_transcript(reader, &database, false).map_err(|error| error.to_string())?;
            print_json(&summary)
        }
        HarnessCommand::Resume {
            transcript,
            database,
        } => {
            let reader = open_transcript(&transcript)?;
            let summary =
                apply_transcript(reader, &database, true).map_err(|error| error.to_string())?;
            print_json(&summary)
        }
        HarnessCommand::Verify {
            transcript,
            database,
        } => {
            if database.exists() {
                return Err("verification database already exists".to_owned());
            }
            let reader = open_transcript(&transcript)?;
            let summary =
                verify_transcript(reader, &database).map_err(|error| error.to_string())?;
            print_json(&summary)
        }
        HarnessCommand::Summarize { database } => {
            let sqlite = SqliteBaseline::open(&database).map_err(|error| error.to_string())?;
            let manifest = sqlite.manifest().map_err(|error| error.to_string())?;
            print_json(&ManifestSummary {
                live_messages: manifest.live_messages(),
                manifest,
            })
        }
    }
}

fn open_transcript(path: &PathBuf) -> Result<BufReader<File>, String> {
    File::open(path)
        .map(BufReader::new)
        .map_err(|_| "cannot open transcript".to_owned())
}

fn print_json(value: &impl Serialize) -> Result<(), String> {
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, value).map_err(|_| "cannot encode output".to_owned())?;
    writer
        .write_all(b"\n")
        .map_err(|_| "cannot write output".to_owned())
}
