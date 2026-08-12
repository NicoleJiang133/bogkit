use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[test]
fn generated_corpus_runs_through_gate_with_declared_shape() {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("generated-{}-{id}", std::process::id()));
    let generated = Command::new(env!("CARGO_BIN_EXE_fixture-generator"))
        .args([
            "--root",
            root.to_str().expect("root path"),
            "--zones",
            "12",
            "--records-per-zone",
            "24",
            "--changed-zones",
            "3",
        ])
        .output()
        .expect("run fixture generator");
    assert!(
        generated.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&generated.stderr)
    );
    assert!(
        String::from_utf8_lossy(&generated.stdout)
            .contains("zones=12 records_per_snapshot=288 changed_zones=3")
    );

    let report = root.join("report.json");
    let gate = Command::new(env!("CARGO_BIN_EXE_dns-change-gate"))
        .args([
            "--old-root",
            root.join("old").to_str().expect("old root"),
            "--new-root",
            root.join("new").to_str().expect("new root"),
            "--policy",
            root.join("policy.conf").to_str().expect("policy"),
            "--output",
            report.to_str().expect("report"),
        ])
        .output()
        .expect("run generated corpus");
    assert!(
        gate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&gate.stderr)
    );
    let body = fs::read_to_string(&report).expect("generated report");
    assert!(body.contains("zone00000.example."));
    assert!(body.contains("zone00011.example."));
    assert_eq!(body.matches("\"zone\":").count(), 12);
    assert_eq!(body.matches("SOA_ADVANCE").count(), 3);
    assert_eq!(body.matches("\"kind\":\"addition\"").count(), 66);
    assert_eq!(body.matches("\"kind\":\"removal\"").count(), 66);
    fs::remove_dir_all(root).expect("clean fixture root");
}
