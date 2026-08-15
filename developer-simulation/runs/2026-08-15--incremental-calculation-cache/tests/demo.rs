use financial_snapshot_trial::{GateStatus, run_demo};

#[test]
fn demo_runs_repeatability_chunking_and_concurrency_checks() {
    let path = std::env::temp_dir().join(format!("financial-snapshot-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);

    let manifest = run_demo(&path).unwrap();

    assert_eq!(manifest.clean_builds, 5);
    assert_eq!(manifest.chunk_sizes, vec![1, 7, 64, 511, 4_096]);
    assert_eq!(manifest.local_exactness, GateStatus::Pass);
    assert_eq!(manifest.local_determinism, GateStatus::Pass);
    assert_eq!(manifest.one_way_initialization_gate, GateStatus::Pass);
    assert_eq!(manifest.recovery_continuation_gate, GateStatus::Pass);
    assert_eq!(manifest.reader_tasks, 12);
    assert!(manifest.read_requests >= 24_000);
    assert_eq!(manifest.mixed_generation_responses, 0);
    assert!(manifest.observed_single_store_disk_bytes > 0);
    assert!(
        manifest.observed_100_batch_store_disk_bytes > manifest.observed_single_store_disk_bytes
    );
    assert!(manifest.observed_total_test_disk_bytes > manifest.observed_single_store_disk_bytes);
    assert_eq!(manifest.production_scale_gates, GateStatus::NotRun);
    assert!(!path.exists(), "demo removes its data directory");
}
