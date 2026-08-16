mod common;

use std::fs;

use common::{ROOT_1, ROOT_2, ROOT_3, TestSigner, hex32};
use tempfile::tempdir;
use transparency_checkpoint_verifier::{
    DirectoryStore, Engine, ErrorCode, KnownLog, PublicationStage, Registry,
};

fn snapshot_with_one_checkpoint() -> transparency_checkpoint_verifier::EngineSnapshot {
    let signer = TestSigner::new();
    let registry = Registry::new(vec![KnownLog {
        log_id: "log-a".to_string(),
        key_id: "key-1".to_string(),
        algorithm: 1,
        public_key: signer.public_key(),
    }])
    .unwrap();
    let mut engine = Engine::new(registry);
    let raw = signer.envelope("log-a", "key-1", 0, 1, ROOT_1, &[], 1);
    assert_eq!(engine.submit(&raw, 42).code, ErrorCode::Advanced);
    engine.snapshot()
}

#[test]
fn every_completed_stage_returned_error_exposes_old_or_new_complete_snapshot() {
    let old = EngineSnapshotBuilder::empty();
    let new = snapshot_with_one_checkpoint();
    for stage in PublicationStage::ALL {
        let dir = tempdir().unwrap();
        let store = DirectoryStore::new(dir.path());
        store.publish(&old, None).unwrap();
        let result = store.publish(&new, Some(stage));
        assert!(result.is_err(), "stage {stage:?}");
        let visible = store.reopen().unwrap();
        assert!(visible == old || visible == new, "torn state at {stage:?}");
    }
}

#[test]
fn generic_returned_error_retry_converges_to_clean_publication_bytes() {
    let snapshot = snapshot_with_one_checkpoint();
    let clean_dir = tempdir().unwrap();
    let clean = DirectoryStore::new(clean_dir.path());
    clean.publish(&snapshot, None).unwrap();
    let expected = clean.visible_bytes().unwrap();

    for stage in PublicationStage::ALL {
        let dir = tempdir().unwrap();
        let store = DirectoryStore::new(dir.path());
        let _ = store.publish(&snapshot, Some(stage));
        store.publish(&snapshot, None).unwrap();
        assert_eq!(store.visible_bytes().unwrap(), expected, "stage {stage:?}");
    }
}

#[test]
fn separately_corrupt_state_proof_manifest_and_decision_files_fail_closed() {
    let snapshot = snapshot_with_one_checkpoint();
    for name in [
        "state.json",
        "proofs.json",
        "manifest.json",
        "decisions.jsonl",
    ] {
        let dir = tempdir().unwrap();
        let store = DirectoryStore::new(dir.path());
        store.publish(&snapshot, None).unwrap();
        let generation = fs::read_to_string(dir.path().join("CURRENT")).unwrap();
        fs::write(dir.path().join(generation.trim()).join(name), b"corrupt").unwrap();
        assert!(store.reopen().is_err(), "accepted corrupt {name}");
    }
}

#[test]
fn republish_rejects_corrupt_existing_generation_and_preserves_current() {
    let old = EngineSnapshotBuilder::empty();
    let new = snapshot_with_one_checkpoint();
    let dir = tempdir().unwrap();
    let store = DirectoryStore::new(dir.path());

    store.publish(&old, None).unwrap();
    let old_generation = fs::read_to_string(dir.path().join("CURRENT")).unwrap();
    store.publish(&new, None).unwrap();
    let new_generation = fs::read_to_string(dir.path().join("CURRENT")).unwrap();
    assert_ne!(old_generation, new_generation);
    store.publish(&old, None).unwrap();
    fs::write(
        dir.path().join(new_generation.trim()).join("state.json"),
        b"corrupt",
    )
    .unwrap();

    let before_pointer = fs::read(dir.path().join("CURRENT")).unwrap();
    assert_eq!(store.publish(&new, None), Err(ErrorCode::StoreCorrupt));
    assert_eq!(
        fs::read(dir.path().join("CURRENT")).unwrap(),
        before_pointer
    );
    assert_eq!(store.reopen().unwrap(), old);
}

#[test]
fn reopened_engine_continues_and_drains_pending_without_divergence() {
    let signer = TestSigner::new();
    let registry = Registry::new(vec![KnownLog {
        log_id: "log-a".to_string(),
        key_id: "key-1".to_string(),
        algorithm: 1,
        public_key: signer.public_key(),
    }])
    .unwrap();
    let leaf_1 = hex32("2ae1c19c0cbd378e46c927a9f3611923ec07cc1ae357502a09536d455275cf21");
    let leaf_2 = hex32("d81f51781eeb8f46a0e112e86ca335896ccc12cee00e6cfcf58a3501129dfc24");
    let first = signer.envelope("log-a", "key-1", 0, 1, ROOT_1, &[], 1);
    let second = signer.envelope("log-a", "key-1", 1, 2, ROOT_2, &[leaf_1], 2);
    let third = signer.envelope("log-a", "key-1", 2, 3, ROOT_3, &[leaf_2], 3);
    let mut engine = Engine::new(registry.clone());
    assert_eq!(engine.submit(&first, 10).code, ErrorCode::Advanced);
    assert_eq!(engine.submit(&third, 30).code, ErrorCode::PendingBase);
    let before_publish = engine.snapshot();

    let dir = tempdir().unwrap();
    let store = DirectoryStore::new(dir.path());
    store.publish(&before_publish, None).unwrap();
    let reopened = store.reopen().unwrap();
    let mut resumed = Engine::from_snapshot(registry.clone(), reopened).unwrap();
    let mut control = Engine::from_snapshot(registry.clone(), before_publish).unwrap();

    assert_eq!(resumed.submit(&second, 20).code, ErrorCode::Advanced);
    assert_eq!(control.submit(&second, 20).code, ErrorCode::Advanced);
    assert_eq!(resumed.current("log-a").unwrap().tree_size, 3);
    assert!(resumed.snapshot().pending["log-a"].is_empty());
    assert_eq!(resumed.stable_bytes(), control.stable_bytes());

    let mut invalid = resumed.snapshot();
    invalid.logs.clear();
    assert!(Engine::from_snapshot(registry, invalid).is_err());
}

struct EngineSnapshotBuilder;

impl EngineSnapshotBuilder {
    fn empty() -> transparency_checkpoint_verifier::EngineSnapshot {
        Engine::new(Registry::new(Vec::new()).unwrap()).snapshot()
    }
}
