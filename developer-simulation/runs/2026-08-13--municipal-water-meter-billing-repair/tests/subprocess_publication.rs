#![cfg(unix)]

use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use tempfile::tempdir;

const ABORT_SIGNAL: i32 = 6;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/disclosed.json")
}

fn expected_plan() -> Vec<u8> {
    fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/disclosed-expected.json"))
        .unwrap()
}

fn run_process(output: &Path, crash_point: Option<&str>) -> ExitStatus {
    let mut command = Command::new(env!("CARGO_BIN_EXE_water-repair"));
    command
        .arg("process")
        .arg("--input")
        .arg(fixture())
        .arg("--output")
        .arg(output)
        .arg("--batch-size")
        .arg("7");
    if let Some(point) = crash_point {
        command.arg("--crash").arg(point);
    }
    command.status().expect("subprocess starts")
}

fn temporary_files(directory: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".tmp"))
        })
        .collect();
    files.sort();
    files
}

#[test]
fn abrupt_exit_before_staging_preserves_existing_report_and_creates_no_partial() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("plan.json");
    let prior = b"prior-complete-report\n";
    fs::write(&output, prior).unwrap();

    let status = run_process(&output, Some("before-staging"));

    assert_eq!(status.signal(), Some(ABORT_SIGNAL));
    assert_eq!(fs::read(&output).unwrap(), prior);
    assert!(temporary_files(directory.path()).is_empty());
}

#[test]
fn abrupt_exit_before_staging_leaves_requested_report_absent() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("plan.json");

    let status = run_process(&output, Some("before-staging"));

    assert_eq!(status.signal(), Some(ABORT_SIGNAL));
    assert!(!output.exists());
    assert!(temporary_files(directory.path()).is_empty());
}

#[test]
fn abrupt_exit_after_complete_temp_before_final_preserves_existing_report() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("plan.json");
    let prior = b"prior-complete-report\n";
    fs::write(&output, prior).unwrap();

    let status = run_process(&output, Some("before-final"));

    assert_eq!(status.signal(), Some(ABORT_SIGNAL));
    assert_eq!(fs::read(&output).unwrap(), prior);
    let staged = temporary_files(directory.path());
    assert_eq!(staged.len(), 1, "crash must occur after one complete temp");
    assert_eq!(fs::read(&staged[0]).unwrap(), expected_plan());

    assert!(run_process(&output, None).success());
    assert_ne!(fs::read(&output).unwrap(), prior);
    assert!(temporary_files(directory.path()).is_empty());
}

#[test]
fn abrupt_exit_after_complete_temp_leaves_requested_report_absent_until_restart() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("plan.json");

    let status = run_process(&output, Some("before-final"));

    assert_eq!(status.signal(), Some(ABORT_SIGNAL));
    assert!(!output.exists());
    let staged = temporary_files(directory.path());
    assert_eq!(staged.len(), 1);
    assert_eq!(fs::read(&staged[0]).unwrap(), expected_plan());

    assert!(run_process(&output, None).success());
    assert!(output.exists());
    assert!(temporary_files(directory.path()).is_empty());
}
