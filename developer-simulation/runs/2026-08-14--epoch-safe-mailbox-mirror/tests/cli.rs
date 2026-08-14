use std::process::Command;

#[test]
fn cli_generate_verify_resume_and_summarize_are_runnable() {
    // Break caught: a documented harness command not being wired to the tested library path.
    let temporary = tempfile::tempdir().unwrap();
    let transcript = temporary.path().join("transcript.ndjson");
    let database = temporary.path().join("mirror.sqlite");
    let binary = env!("CARGO_BIN_EXE_mailbox-mirror-lab");

    let generated = Command::new(binary)
        .args(["generate", "--output"])
        .arg(&transcript)
        .args([
            "--seed",
            "29",
            "--mailboxes",
            "30",
            "--messages",
            "300",
            "--responses",
            "1200",
        ])
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    let verified = Command::new(binary)
        .args(["verify", "--transcript"])
        .arg(&transcript)
        .arg("--database")
        .arg(&database)
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );

    let resumed = Command::new(binary)
        .args(["resume", "--transcript"])
        .arg(&transcript)
        .arg("--database")
        .arg(&database)
        .output()
        .unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );

    let summarized = Command::new(binary)
        .args(["summarize", "--database"])
        .arg(&database)
        .output()
        .unwrap();
    assert!(
        summarized.status.success(),
        "{}",
        String::from_utf8_lossy(&summarized.stderr)
    );
    let manifest: serde_json::Value = serde_json::from_slice(&summarized.stdout).unwrap();
    assert_eq!(manifest["live_messages"], 300);
}
