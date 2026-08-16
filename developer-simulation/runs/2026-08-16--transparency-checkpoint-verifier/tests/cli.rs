use std::fs;
use std::process::Command;

use tempfile::tempdir;

fn verifier() -> Command {
    Command::new(env!("CARGO_BIN_EXE_transparency-checkpoint-verifier"))
}

#[test]
fn replay_reports_hand_checked_root_for_four_leaf_hashes() {
    let output = verifier().args(["replay", "4"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"leaves\":4"));
    assert!(
        stdout.contains(
            "\"root\":\"b15d2b1b07adada9b13b555c08062b1ae78ad1b0b7e99d97d942c936a6244439\""
        )
    );
}

#[test]
fn compare_distinguishes_identical_and_different_ledgers() {
    let dir = tempdir().unwrap();
    let expected = dir.path().join("expected.jsonl");
    let same = dir.path().join("same.jsonl");
    let different = dir.path().join("different.jsonl");
    fs::write(&expected, b"{\"code\":\"ADVANCED\"}\n").unwrap();
    fs::write(&same, b"{\"code\":\"ADVANCED\"}\n").unwrap();
    fs::write(&different, b"{\"code\":\"PROOF_INVALID\"}\n").unwrap();

    let matched = verifier()
        .arg("compare")
        .arg(&expected)
        .arg(&same)
        .output()
        .unwrap();
    assert!(matched.status.success());
    assert_eq!(matched.stdout, b"MATCH\n");

    let mismatch = verifier()
        .arg("compare")
        .arg(&expected)
        .arg(&different)
        .output()
        .unwrap();
    assert_eq!(mismatch.status.code(), Some(2));
    assert_eq!(mismatch.stdout, b"MISMATCH\n");
}

#[test]
fn demo_publishes_and_reopens_eight_logs() {
    let dir = tempdir().unwrap();
    let output = verifier().arg("demo").arg(dir.path()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\"logs\":8"));
    assert!(dir.path().join("CURRENT").exists());
}

#[test]
fn incremental_benchmark_reports_p95_for_real_verification_path() {
    let output = verifier()
        .args(["bench-incremental", "100"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["iterations"], 100);
    assert!(value["p95_us"].as_u64().unwrap() < 5_000);
}
