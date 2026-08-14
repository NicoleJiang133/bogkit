use std::process::Command;

use undo_history_lab::baseline::Baseline;
use undo_history_lab::candidate::Candidate;

#[test]
fn exit_after_durable_commit_before_response_recovers_for_both_stores() {
    let binary = env!("CARGO_BIN_EXE_undo-history-lab");
    for kind in ["candidate", "baseline"] {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join(kind);
        let status = Command::new(binary)
            .args(["fault-commit", kind, store.to_str().unwrap()])
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(86), "kind={kind}");

        let undo_count = if kind == "candidate" {
            Candidate::open(&store)
                .unwrap()
                .status()
                .unwrap()
                .undo_count
        } else {
            Baseline::open(&store, 2_000)
                .unwrap()
                .status()
                .unwrap()
                .undo_count
        };
        assert_eq!(undo_count, 1, "kind={kind}");
    }
}
