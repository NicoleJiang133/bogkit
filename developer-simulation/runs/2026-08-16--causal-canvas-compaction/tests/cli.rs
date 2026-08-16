use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use causal_canvas_compaction::open_latest;

fn temporary_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("canvas-cli-{label}-{nonce}"))
}

#[test]
fn replay_command_reads_ndjson_compacts_publishes_and_reopens() {
    // Catches a harness that only demonstrates in-memory helpers, ignores the
    // declared directory inputs, or reports success without readable state.
    let input = temporary_path("input");
    let state = temporary_path("state");
    fs::create_dir_all(&input).unwrap();
    let operations = concat!(
        "{\"id\":\"a-1\",\"document_id\":\"canvas\",\"actor_id\":\"a\",\"actor_sequence\":1,\"dependency_clock\":{},\"accepted_minute\":1,\"payload\":{\"kind\":\"create\",\"object_id\":\"shape\",\"object_kind\":\"rectangle\"}}\n",
        "{\"id\":\"a-2\",\"document_id\":\"canvas\",\"actor_id\":\"a\",\"actor_sequence\":2,\"dependency_clock\":{\"a\":1},\"accepted_minute\":2,\"payload\":{\"kind\":\"set_property\",\"object_id\":\"shape\",\"key\":\"fill\",\"value\":\"blue\"}}\n"
    );
    fs::write(input.join("operations.ndjson"), operations).unwrap();
    fs::write(
        input.join("watermarks.json"),
        "[{\"client_id\":\"client\",\"document_id\":\"canvas\",\"acknowledged\":{\"a\":2},\"acknowledged_minute\":10}]",
    )
    .unwrap();
    fs::write(input.join("crash_schedules.json"), "[]").unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_causal-canvas-compaction"))
        .args([
            "replay",
            input.to_str().unwrap(),
            state.to_str().unwrap(),
            "canvas",
            "10",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("\"accepted_operations\":2"));
    let reopened = open_latest(&state).unwrap();
    assert_eq!(
        reopened.core.canonical_snapshot("canvas").unwrap(),
        r#"{"document_id":"canvas","objects":[{"id":"shape","kind":"rectangle","x":0,"y":0,"properties":{"fill":"blue"}}],"lists":[]}"#
    );

    fs::remove_dir_all(input).unwrap();
    fs::remove_dir_all(state).unwrap();
}

#[test]
fn largest_command_builds_publishes_and_times_a_requested_shape() {
    // Catches reporting an open benchmark without actually publishing and
    // reopening the requested object/list cardinalities.
    let state = temporary_path("largest-state");
    let result = Command::new(env!("CARGO_BIN_EXE_causal-canvas-compaction"))
        .args(["largest", "10", "5", state.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(summary["objects"], 10);
    assert_eq!(summary["list_elements"], 5);
    let reopened = open_latest(&state).unwrap();
    let snapshot = reopened.core.canonical_snapshot("largest").unwrap();
    assert!(snapshot.contains("\"id\":\"object-000009\""));
    assert!(snapshot.contains("\"id\":\"element-000004\""));
    fs::remove_dir_all(state).unwrap();
}
