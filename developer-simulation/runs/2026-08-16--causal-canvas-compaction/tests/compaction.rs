use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use causal_canvas_compaction::{
    ApplyError, ApplyOutcome, ClientWatermark, CompactionError, Core, OFFLINE_WINDOW_MINUTES,
    Operation, Payload, ReconnectDecision, open_latest, publish_generation, reconnect_decision,
};

const DAY_MINUTES: u64 = 24 * 60;

#[test]
fn reconnect_window_has_deterministic_full_thirty_day_boundary() {
    // Catches off-by-one comparisons and wall-clock rounding. Inputs are exact
    // integer minutes; 30 days is allowed and 30 days + 1 minute is not.
    let now = 100 * DAY_MINUTES;
    assert_eq!(
        reconnect_decision(now, now - (30 * DAY_MINUTES - 1)),
        ReconnectDecision::CatchUp
    );
    assert_eq!(
        reconnect_decision(now, now - 30 * DAY_MINUTES),
        ReconnectDecision::CatchUp
    );
    assert_eq!(
        reconnect_decision(now, now - (30 * DAY_MINUTES + 1)),
        ReconnectDecision::SnapshotRequired
    );
}

fn op(sequence: u64, payload: Payload) -> Operation {
    Operation {
        id: format!("alice-{sequence}"),
        document_id: "canvas".to_string(),
        actor_id: "alice".to_string(),
        actor_sequence: sequence,
        dependency_clock: if sequence == 1 {
            BTreeMap::new()
        } else {
            BTreeMap::from([("alice".to_string(), sequence - 1)])
        },
        accepted_minute: 10_000 + sequence,
        payload,
    }
}

fn overwrite_heavy_operations() -> Vec<Operation> {
    let large = |ch: char| std::iter::repeat_n(ch, 1_000).collect::<String>();
    vec![
        op(
            1,
            Payload::Create {
                object_id: "shape".to_string(),
                object_kind: "text".to_string(),
            },
        ),
        op(
            2,
            Payload::SetProperty {
                object_id: "shape".to_string(),
                key: "text".to_string(),
                value: large('a'),
            },
        ),
        op(
            3,
            Payload::SetProperty {
                object_id: "shape".to_string(),
                key: "text".to_string(),
                value: large('b'),
            },
        ),
        op(
            4,
            Payload::SetProperty {
                object_id: "shape".to_string(),
                key: "text".to_string(),
                value: large('c'),
            },
        ),
        op(
            5,
            Payload::SetProperty {
                object_id: "shape".to_string(),
                key: "text".to_string(),
                value: large('d'),
            },
        ),
        op(
            6,
            Payload::SetProperty {
                object_id: "shape".to_string(),
                key: "text".to_string(),
                value: large('e'),
            },
        ),
        op(
            7,
            Payload::SetProperty {
                object_id: "shape".to_string(),
                key: "text".to_string(),
                value: large('f'),
            },
        ),
        op(
            8,
            Payload::SetProperty {
                object_id: "shape".to_string(),
                key: "text".to_string(),
                value: large('g'),
            },
        ),
    ]
}

#[test]
fn compaction_counts_permanent_identity_metadata_in_retained_byte_gate() {
    // Catches pruning past an in-window watermark, allowing an expired client
    // to pin history, or depending on client input order for manifest bytes.
    let mut core = Core::new();
    for operation in overwrite_heavy_operations() {
        core.apply(operation);
    }

    let now = 100_000;
    let active = ClientWatermark {
        client_id: "active".to_string(),
        document_id: "canvas".to_string(),
        acknowledged: BTreeMap::from([("alice".to_string(), 7)]),
        acknowledged_minute: now - 10,
    };
    let expired = ClientWatermark {
        client_id: "expired".to_string(),
        document_id: "canvas".to_string(),
        acknowledged: BTreeMap::new(),
        acknowledged_minute: now - (OFFLINE_WINDOW_MINUTES + 1),
    };

    let first = core
        .compact("canvas", &[active.clone(), expired.clone()], now)
        .unwrap();
    let reversed = core.compact("canvas", &[expired, active], now).unwrap();

    assert_eq!(first.retained_operation_ids, ["alice-8"]);
    assert_eq!(
        first.reconnect_decisions,
        BTreeMap::from([
            ("active".to_string(), ReconnectDecision::CatchUp),
            ("expired".to_string(), ReconnectDecision::SnapshotRequired),
        ])
    );
    assert_eq!(first.manifest_json, reversed.manifest_json);
    let percentage_hundredths =
        (first.compacted_bytes * 10_000 + first.uncompacted_bytes / 2) / first.uncompacted_bytes;
    eprintln!(
        "retained-byte gate: compacted={} uncompacted={} percent={}.{:02}",
        first.compacted_bytes,
        first.uncompacted_bytes,
        percentage_hundredths / 100,
        percentage_hundredths % 100
    );
    assert!(first.compacted_bytes * 100 > first.uncompacted_bytes * 40);
}

#[test]
fn compacted_state_reopens_with_tombstones_and_accepts_in_window_pending_edit() {
    // Catches publishing only visible JSON while losing the deletion metadata or
    // causal frontier needed to validate a supported reconnecting actor's edit.
    let mut core = Core::new();
    core.apply(Operation {
        id: "creator-1".to_string(),
        document_id: "canvas".to_string(),
        actor_id: "creator".to_string(),
        actor_sequence: 1,
        dependency_clock: BTreeMap::new(),
        accepted_minute: 1,
        payload: Payload::Create {
            object_id: "gone".to_string(),
            object_kind: "rectangle".to_string(),
        },
    });
    core.apply(Operation {
        id: "deleter-1".to_string(),
        document_id: "canvas".to_string(),
        actor_id: "deleter".to_string(),
        actor_sequence: 1,
        dependency_clock: BTreeMap::from([("creator".to_string(), 1)]),
        accepted_minute: 2,
        payload: Payload::ObjectDelete {
            object_id: "gone".to_string(),
        },
    });
    let client = ClientWatermark {
        client_id: "offline".to_string(),
        document_id: "canvas".to_string(),
        acknowledged: BTreeMap::from([("creator".to_string(), 1), ("deleter".to_string(), 1)]),
        acknowledged_minute: 9_900,
    };

    let artifact = core.compact("canvas", &[client], 10_000).unwrap();
    let mut reopened = artifact.reopen().unwrap();
    let pending_edit = Operation {
        id: "offline-1".to_string(),
        document_id: "canvas".to_string(),
        actor_id: "offline".to_string(),
        actor_sequence: 1,
        dependency_clock: BTreeMap::from([("deleter".to_string(), 1)]),
        accepted_minute: 10_000,
        payload: Payload::SetProperty {
            object_id: "gone".to_string(),
            key: "fill".to_string(),
            value: "red".to_string(),
        },
    };
    assert_eq!(
        reopened.apply(pending_edit),
        ApplyOutcome::Applied { drained: 0 }
    );
    assert_eq!(
        reopened.canonical_snapshot("canvas").unwrap(),
        r#"{"document_id":"canvas","objects":[],"lists":[]}"#
    );
}

#[test]
fn compaction_refuses_a_causally_incomplete_state() {
    // Catches publishing a snapshot while accepted operations remain buffered,
    // which could otherwise make an incomplete generation appear complete.
    let mut core = Core::new();
    core.apply(Operation {
        id: "a-2".to_string(),
        document_id: "canvas".to_string(),
        actor_id: "a".to_string(),
        actor_sequence: 2,
        dependency_clock: BTreeMap::from([("a".to_string(), 1)]),
        accepted_minute: 2,
        payload: Payload::ObjectDelete {
            object_id: "shape".to_string(),
        },
    });
    assert_eq!(
        core.compact("canvas", &[], 10),
        Err(CompactionError::PendingOperations)
    );
}

#[test]
fn conflicting_operation_id_reuse_remains_rejected_after_compact_reopen() {
    // Catches compacting away an accepted operation's permanent identity
    // fingerprint. The conflicting reuse has the next valid actor sequence, so
    // only the operation-ID lifecycle rule can reject it.
    let mut core = Core::new();
    assert_eq!(
        core.apply(Operation {
            id: "reused-id".to_string(),
            document_id: "canvas".to_string(),
            actor_id: "alice".to_string(),
            actor_sequence: 1,
            dependency_clock: BTreeMap::new(),
            accepted_minute: 1,
            payload: Payload::Create {
                object_id: "shape".to_string(),
                object_kind: "rectangle".to_string(),
            },
        }),
        ApplyOutcome::Applied { drained: 0 }
    );
    let artifact = core.compact("canvas", &[], 10).unwrap();
    let conflicting_reuse = Operation {
        id: "reused-id".to_string(),
        document_id: "canvas".to_string(),
        actor_id: "alice".to_string(),
        actor_sequence: 2,
        dependency_clock: BTreeMap::from([("alice".to_string(), 1)]),
        accepted_minute: 2,
        payload: Payload::ObjectDelete {
            object_id: "shape".to_string(),
        },
    };

    let root = temporary_store("identity-lifecycle");
    publish_generation(&root, 1, &artifact, None).unwrap();
    let mut published = open_latest(&root).unwrap().core;
    assert_conflicting_reuse_is_unchanged(&mut published, conflicting_reuse.clone());
    fs::remove_dir_all(root).unwrap();

    let mut reopened = artifact.reopen().unwrap();
    assert_conflicting_reuse_is_unchanged(&mut reopened, conflicting_reuse);
}

fn assert_conflicting_reuse_is_unchanged(core: &mut Core, operation: Operation) {
    let visible_before = core.canonical_snapshot("canvas").unwrap();
    let digest_before = core.snapshot_digest("canvas").unwrap();
    let count_before = core.accepted_count();
    assert_eq!(
        core.apply(operation),
        ApplyOutcome::Rejected(ApplyError::ConflictingOperationId)
    );
    assert_eq!(core.canonical_snapshot("canvas").unwrap(), visible_before);
    assert_eq!(core.snapshot_digest("canvas").unwrap(), digest_before);
    assert_eq!(core.accepted_count(), count_before);
}

fn temporary_store(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "causal-compaction-{label}-{}-{nonce}",
        std::process::id()
    ))
}
