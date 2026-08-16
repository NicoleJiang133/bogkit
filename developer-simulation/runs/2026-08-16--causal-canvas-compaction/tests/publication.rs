use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use causal_canvas_compaction::{
    ClientWatermark, Core, FaultInjection, FaultStage, Operation, Payload, open_latest,
    publish_generation,
};

fn temporary_store(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "causal-canvas-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn core_with_value(value: &str) -> Core {
    let mut core = Core::new();
    core.apply(Operation {
        id: "actor-1".to_string(),
        document_id: "canvas".to_string(),
        actor_id: "actor".to_string(),
        actor_sequence: 1,
        dependency_clock: BTreeMap::new(),
        accepted_minute: 1,
        payload: Payload::Create {
            object_id: "shape".to_string(),
            object_kind: "rectangle".to_string(),
        },
    });
    core.apply(Operation {
        id: "actor-2".to_string(),
        document_id: "canvas".to_string(),
        actor_id: "actor".to_string(),
        actor_sequence: 2,
        dependency_clock: BTreeMap::from([("actor".to_string(), 1)]),
        accepted_minute: 2,
        payload: Payload::SetProperty {
            object_id: "shape".to_string(),
            key: "fill".to_string(),
            value: value.to_string(),
        },
    });
    core
}

#[test]
fn in_process_returned_error_at_each_stage_never_exposes_partial_generation() {
    // Catches moving CURRENT before all generation files have completed, and
    // catches retry logic that cannot finish the same generation idempotently.
    let stages = [
        FaultStage::GenerationCreated,
        FaultStage::DataFlushed,
        FaultStage::ManifestFlushed,
        FaultStage::GenerationRenamed,
        FaultStage::DirectorySynced,
    ];
    for stage in stages {
        let root = temporary_store(&format!("returned-error-{stage:?}"));
        let old = core_with_value("old").compact("canvas", &[], 100).unwrap();
        let new = core_with_value("new").compact("canvas", &[], 100).unwrap();
        publish_generation(&root, 1, &old, None).unwrap();

        let fault = FaultInjection { stage };
        assert!(publish_generation(&root, 2, &new, Some(fault)).is_err());
        let readable = open_latest(&root).unwrap();
        let visible = readable.core.canonical_snapshot("canvas").unwrap();
        assert!(
            visible == old.snapshot_json || visible == new.snapshot_json,
            "stage {stage:?} exposed neither complete generation"
        );

        publish_generation(&root, 2, &new, None).unwrap();
        publish_generation(&root, 2, &new, None).unwrap();
        let completed = open_latest(&root).unwrap();
        assert_eq!(completed.generation, 2);
        assert_eq!(
            completed.core.canonical_snapshot("canvas").unwrap(),
            new.snapshot_json
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn corrupt_current_manifest_falls_back_to_prior_valid_generation() {
    // Catches trusting CURRENT without content validation or selecting a corrupt
    // newest directory instead of the deterministic prior complete generation.
    let root = temporary_store("corrupt-fallback");
    let old = core_with_value("old").compact("canvas", &[], 100).unwrap();
    let new = core_with_value("new").compact("canvas", &[], 100).unwrap();
    publish_generation(&root, 1, &old, None).unwrap();
    publish_generation(&root, 2, &new, None).unwrap();
    fs::write(
        root.join("generation-00000000000000000002/manifest.json"),
        b"{\"corrupt\":true}",
    )
    .unwrap();

    let reopened = open_latest(&root).unwrap();
    assert_eq!(reopened.generation, 1);
    assert_eq!(
        reopened.core.canonical_snapshot("canvas").unwrap(),
        old.snapshot_json
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_manifest_metadata_falls_back_or_fails() {
    // Catches validating only selected payload digests while trusting mutable
    // semantic metadata. reconnect_decisions affects client behavior and must
    // be bound by the complete-manifest integrity check.
    let root = temporary_store("manifest-metadata");
    let old = core_with_value("old").compact("canvas", &[], 100).unwrap();
    let client = ClientWatermark {
        client_id: "browser".to_string(),
        document_id: "canvas".to_string(),
        acknowledged: BTreeMap::from([("actor".to_string(), 2)]),
        acknowledged_minute: 100,
    };
    let new = core_with_value("new")
        .compact("canvas", &[client], 100)
        .unwrap();
    publish_generation(&root, 1, &old, None).unwrap();
    publish_generation(&root, 2, &new, None).unwrap();

    let manifest_path = root.join("generation-00000000000000000002/manifest.json");
    let original = fs::read_to_string(&manifest_path).unwrap();
    let changed = original.replace("\"catch_up\"", "\"snapshot_required\"");
    assert_ne!(changed, original);
    fs::write(manifest_path, changed).unwrap();

    match open_latest(&root) {
        Ok(reopened) => assert_eq!(reopened.generation, 1),
        Err(error) => assert_eq!(error.to_string(), "no complete valid generation"),
    }
    fs::remove_dir_all(root).unwrap();
}
