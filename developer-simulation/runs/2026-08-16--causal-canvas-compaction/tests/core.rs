use std::collections::BTreeMap;

use causal_canvas_compaction::{ApplyError, ApplyOutcome, Core, Limits, Operation, Payload};

fn clock(entries: &[(&str, u64)]) -> BTreeMap<String, u64> {
    entries
        .iter()
        .map(|(actor, seq)| ((*actor).to_string(), *seq))
        .collect()
}

fn op(id: &str, actor: &str, seq: u64, deps: &[(&str, u64)], payload: Payload) -> Operation {
    Operation {
        id: id.to_string(),
        document_id: "canvas".to_string(),
        actor_id: actor.to_string(),
        actor_sequence: seq,
        dependency_clock: clock(deps),
        accepted_minute: 1_000 + seq,
        payload,
    }
}

#[test]
fn causally_buffered_scalar_history_has_literal_canonical_snapshot() {
    // Catches applying a dependent write before its create, failing to drain the
    // buffer, or emitting state in insertion/hash-map order. Expected JSON is
    // hand-derived from the two operations below.
    let create = op(
        "a-1",
        "alice",
        1,
        &[],
        Payload::Create {
            object_id: "shape-1".to_string(),
            object_kind: "rectangle".to_string(),
        },
    );
    let set_fill = op(
        "a-2",
        "alice",
        2,
        &[("alice", 1)],
        Payload::SetProperty {
            object_id: "shape-1".to_string(),
            key: "fill".to_string(),
            value: "blue".to_string(),
        },
    );

    let mut core = Core::new();
    assert_eq!(core.apply(set_fill), ApplyOutcome::Buffered);
    assert_eq!(core.apply(create), ApplyOutcome::Applied { drained: 1 });

    assert_eq!(
        core.canonical_snapshot("canvas").unwrap(),
        r#"{"document_id":"canvas","objects":[{"id":"shape-1","kind":"rectangle","x":0,"y":0,"properties":{"fill":"blue"}}],"lists":[]}"#
    );
}

#[test]
fn duplicate_is_a_no_op_and_conflicting_id_reuse_is_rejected_atomically() {
    // Catches identity tracking that checks only actor/sequence, counts an exact
    // duplicate, or mutates visible state before discovering an ID conflict.
    let create = op(
        "a-1",
        "alice",
        1,
        &[],
        Payload::Create {
            object_id: "shape-1".to_string(),
            object_kind: "rectangle".to_string(),
        },
    );
    let conflicting = op(
        "a-1",
        "alice",
        1,
        &[],
        Payload::Create {
            object_id: "wrong-shape".to_string(),
            object_kind: "ellipse".to_string(),
        },
    );

    let mut core = Core::new();
    assert_eq!(
        core.apply(create.clone()),
        ApplyOutcome::Applied { drained: 0 }
    );
    let before = core.canonical_snapshot("canvas").unwrap();
    let before_digest = core.snapshot_digest("canvas").unwrap();
    let before_count = core.accepted_count();

    assert_eq!(core.apply(create), ApplyOutcome::Duplicate);
    assert_eq!(core.accepted_count(), before_count);
    assert_eq!(core.snapshot_digest("canvas").unwrap(), before_digest);

    assert_eq!(
        core.apply(conflicting),
        ApplyOutcome::Rejected(ApplyError::ConflictingOperationId)
    );
    assert_eq!(core.accepted_count(), before_count);
    assert_eq!(core.canonical_snapshot("canvas").unwrap(), before);
}

#[test]
fn concurrent_scalar_winner_is_independent_of_delivery_order() {
    // Catches arrival-order LWW. Both writes have the same integer sequence;
    // the reduced model's declared tie-break is actor ID, so "bob" wins.
    let create = op(
        "creator-1",
        "creator",
        1,
        &[],
        Payload::Create {
            object_id: "shape-1".to_string(),
            object_kind: "rectangle".to_string(),
        },
    );
    let alice = op(
        "alice-1",
        "alice",
        1,
        &[("creator", 1)],
        Payload::SetProperty {
            object_id: "shape-1".to_string(),
            key: "fill".to_string(),
            value: "red".to_string(),
        },
    );
    let bob = op(
        "bob-1",
        "bob",
        1,
        &[("creator", 1)],
        Payload::SetProperty {
            object_id: "shape-1".to_string(),
            key: "fill".to_string(),
            value: "blue".to_string(),
        },
    );

    let mut forward = Core::new();
    for operation in [create.clone(), alice.clone(), bob.clone()] {
        forward.apply(operation);
    }
    let mut reverse = Core::new();
    for operation in [create, bob, alice] {
        reverse.apply(operation);
    }

    let expected = r#"{"document_id":"canvas","objects":[{"id":"shape-1","kind":"rectangle","x":0,"y":0,"properties":{"fill":"blue"}}],"lists":[]}"#;
    assert_eq!(forward.canonical_snapshot("canvas").unwrap(), expected);
    assert_eq!(reverse.canonical_snapshot("canvas").unwrap(), expected);
}

#[test]
fn tombstones_block_resurrection_and_deleted_list_neighbors_still_order_children() {
    // Catches object edits recreating deleted objects, list reinsertion reviving a
    // tombstone, or compaction/order logic losing a child whose anchor is deleted.
    let operations = [
        op(
            "create-1",
            "creator",
            1,
            &[],
            Payload::Create {
                object_id: "shape-1".to_string(),
                object_kind: "rectangle".to_string(),
            },
        ),
        op(
            "insert-a",
            "lisa",
            1,
            &[("creator", 1)],
            Payload::ListInsert {
                list_id: "layers".to_string(),
                element_id: "e1".to_string(),
                left: None,
                value: "A".to_string(),
            },
        ),
        op(
            "delete-object",
            "deleter",
            1,
            &[("creator", 1)],
            Payload::ObjectDelete {
                object_id: "shape-1".to_string(),
            },
        ),
        op(
            "late-move",
            "mover",
            1,
            &[("creator", 1)],
            Payload::Move {
                object_id: "shape-1".to_string(),
                x: 90,
                y: 40,
            },
        ),
        op(
            "recreate-object",
            "resurrector",
            1,
            &[("deleter", 1)],
            Payload::Create {
                object_id: "shape-1".to_string(),
                object_kind: "ellipse".to_string(),
            },
        ),
        op(
            "delete-a",
            "lisa",
            2,
            &[("lisa", 1)],
            Payload::ListDelete {
                list_id: "layers".to_string(),
                element_id: "e1".to_string(),
            },
        ),
        op(
            "insert-b",
            "liam",
            1,
            &[("creator", 1), ("lisa", 1)],
            Payload::ListInsert {
                list_id: "layers".to_string(),
                element_id: "e2".to_string(),
                left: Some("e1".to_string()),
                value: "B".to_string(),
            },
        ),
        op(
            "reinsert-a",
            "ruth",
            1,
            &[("lisa", 2)],
            Payload::ListInsert {
                list_id: "layers".to_string(),
                element_id: "e1".to_string(),
                left: None,
                value: "resurrected".to_string(),
            },
        ),
    ];

    let mut core = Core::new();
    for operation in operations {
        core.apply(operation);
    }

    assert_eq!(
        core.canonical_snapshot("canvas").unwrap(),
        r#"{"document_id":"canvas","objects":[],"lists":[{"id":"layers","elements":[{"id":"e2","value":"B"}]}]}"#
    );
}

#[test]
fn pending_work_is_bounded_and_dependency_cycles_are_reported() {
    // Catches unbounded buffering, accepting invalid sequence boundaries, or
    // classifying an actual cycle as an ordinary missing dependency.
    let limits = Limits {
        max_pending_operations: 2,
        ..Limits::default()
    };
    let mut core = Core::with_limits(limits);
    let cycle_a = op(
        "a-1",
        "a",
        1,
        &[("b", 1)],
        Payload::ObjectDelete {
            object_id: "x".to_string(),
        },
    );
    let cycle_b = op(
        "b-1",
        "b",
        1,
        &[("a", 1)],
        Payload::ObjectDelete {
            object_id: "y".to_string(),
        },
    );
    let missing = op(
        "c-1",
        "c",
        1,
        &[("never-seen", 1)],
        Payload::ObjectDelete {
            object_id: "z".to_string(),
        },
    );

    assert_eq!(core.apply(cycle_a), ApplyOutcome::Buffered);
    assert_eq!(core.apply(cycle_b), ApplyOutcome::Buffered);
    assert!(core.pending_diagnosis().has_cycle);
    assert_eq!(core.pending_diagnosis().pending_operations, 2);
    assert_eq!(
        core.apply(missing),
        ApplyOutcome::Rejected(ApplyError::PendingLimit)
    );
    assert_eq!(core.accepted_count(), 2);

    let mut invalid = op(
        "zero",
        "actor",
        0,
        &[],
        Payload::ObjectDelete {
            object_id: "x".to_string(),
        },
    );
    assert_eq!(
        core.apply(invalid.clone()),
        ApplyOutcome::Rejected(ApplyError::InvalidSequence)
    );
    invalid.actor_sequence = u64::MAX;
    assert_eq!(
        core.apply(invalid),
        ApplyOutcome::Rejected(ApplyError::SequenceOverflow)
    );
}
