use std::process::Command;

#[test]
fn subprocess_exits_preserve_complete_old_or_next_state() {
    let root = std::env::temp_dir().join(format!(
        "cold-chain-fault-cli-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&root);

    let output = Command::new(env!("CARGO_BIN_EXE_cold-chain-repair"))
        .arg("fault-demo")
        .arg(&root)
        .output()
        .expect("run fault demo");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("fault boundaries: passed"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn generated_ndjson_repairs_idempotently_through_the_cli() {
    let root = std::env::temp_dir().join(format!(
        "cold-chain-repair-cli-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let archive = root.join("archive.ndjson");
    let state = root.join("state");
    let binary = env!("CARGO_BIN_EXE_cold-chain-repair");

    assert!(
        Command::new(binary)
            .args(["generate", archive.to_str().unwrap(), "23", "2", "96"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new(binary)
            .args(["repair", archive.to_str().unwrap(), state.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    let first = std::fs::read(state.join("incidents.json")).unwrap();
    assert!(
        Command::new(binary)
            .args(["repair", archive.to_str().unwrap(), state.to_str().unwrap()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(std::fs::read(state.join("incidents.json")).unwrap(), first);
    let _ = std::fs::remove_dir_all(&root);
}
