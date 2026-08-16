mod common;

use common::{ROOT_0, ROOT_1, ROOT_2, ROOT_3, TestSigner, hex32};
use transparency_checkpoint_verifier::{Engine, ErrorCode, KnownLog, Registry};

fn build_engine(signer: &TestSigner, ids: &[&str]) -> Engine {
    let logs = ids
        .iter()
        .map(|id| KnownLog {
            log_id: (*id).to_string(),
            key_id: "key-1".to_string(),
            algorithm: 1,
            public_key: signer.public_key(),
        })
        .collect();
    Engine::new(Registry::new(logs).unwrap())
}

#[test]
fn out_of_order_candidate_waits_then_advances_without_skipping_base() {
    let signer = TestSigner::new();
    let mut engine = build_engine(&signer, &["log-a"]);
    let leaf_1 = hex32("2ae1c19c0cbd378e46c927a9f3611923ec07cc1ae357502a09536d455275cf21");
    let leaf_2 = hex32("d81f51781eeb8f46a0e112e86ca335896ccc12cee00e6cfcf58a3501129dfc24");

    let first = signer.envelope("log-a", "key-1", 0, 1, ROOT_1, &[], 1);
    assert_eq!(engine.submit(&first, 10).code, ErrorCode::Advanced);

    let third = signer.envelope("log-a", "key-1", 2, 3, ROOT_3, &[leaf_2], 3);
    assert_eq!(engine.submit(&third, 30).code, ErrorCode::PendingBase);
    assert_eq!(engine.current("log-a").unwrap().tree_size, 1);

    let second = signer.envelope("log-a", "key-1", 1, 2, ROOT_2, &[leaf_1], 2);
    let outcome = engine.submit(&second, 20);
    assert_eq!(outcome.code, ErrorCode::Advanced);
    assert_eq!(outcome.advanced_sizes, vec![2, 3]);
    let current = engine.current("log-a").unwrap();
    assert_eq!(current.tree_size, 3);
    assert_eq!(current.archive_cursor, 30);
    assert_eq!(current.raw_proof, leaf_2);
}

#[test]
fn invalid_identity_proof_or_signature_never_changes_state() {
    let signer = TestSigner::new();
    let other = TestSigner::new();
    let mut engine = build_engine(&signer, &["log-a"]);
    let before = engine.snapshot();

    let unknown = signer.envelope("log-x", "key-1", 0, 1, ROOT_1, &[], 1);
    assert_eq!(engine.submit(&unknown, 1).code, ErrorCode::UnknownLog);
    assert_eq!(engine.snapshot(), before);

    let wrong_key = other.envelope("log-a", "key-1", 0, 1, ROOT_1, &[], 1);
    assert_eq!(
        engine.submit(&wrong_key, 1).code,
        ErrorCode::SignatureInvalid
    );
    assert_eq!(engine.snapshot(), before);

    let invalid_proof = signer.envelope("log-a", "key-1", 0, 1, ROOT_1, &[[9; 32]], 1);
    assert_eq!(
        engine.submit(&invalid_proof, 1).code,
        ErrorCode::ProofInvalid
    );
    assert_eq!(engine.current("log-a").unwrap().tree_size, 0);
}

#[test]
fn duplicate_is_idempotent_and_equivocation_retains_both_roots_once() {
    let signer = TestSigner::new();
    let mut engine = build_engine(&signer, &["log-a"]);
    let first = signer.envelope("log-a", "key-1", 0, 1, ROOT_1, &[], 1);
    assert_eq!(engine.submit(&first, 10).code, ErrorCode::Advanced);
    let after_first = engine.snapshot();

    assert_eq!(engine.submit(&first, 10).code, ErrorCode::Duplicate);
    assert_eq!(engine.snapshot(), after_first);

    let alternate = [0xa5_u8; 32];
    let conflict = signer.envelope("log-a", "key-1", 0, 1, alternate, &[], 2);
    assert_eq!(engine.submit(&conflict, 11).code, ErrorCode::Equivocation);
    assert_eq!(engine.submit(&conflict, 11).code, ErrorCode::Duplicate);
    let alerts = &engine.snapshot().equivocations;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].first_root, ROOT_1);
    assert_eq!(alerts[0].second_root, alternate);
}

#[test]
fn cross_log_interleavings_produce_identical_durable_bytes() {
    let signer = TestSigner::new();
    let ids = [
        "log-a", "log-b", "log-c", "log-d", "log-e", "log-f", "log-g", "log-h",
    ];
    let envelopes: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            signer.envelope(
                id,
                "key-1",
                0,
                1,
                ROOT_1,
                &[],
                u64::try_from(index).unwrap(),
            )
        })
        .collect();
    let mut baseline = build_engine(&signer, &ids);
    for (index, raw) in envelopes.iter().enumerate() {
        baseline.submit(raw, u64::try_from(index).unwrap());
    }
    let expected = baseline.stable_bytes();

    for interleaving in 0..20 {
        let mut order: Vec<usize> = (0..ids.len()).collect();
        order.rotate_left(interleaving % ids.len());
        if interleaving % 2 == 1 {
            order.reverse();
        }
        let mut candidate = build_engine(&signer, &ids);
        for index in order {
            candidate.submit(&envelopes[index], u64::try_from(index).unwrap());
        }
        assert_eq!(
            candidate.stable_bytes(),
            expected,
            "interleaving {interleaving}"
        );
    }
}

#[test]
fn bit_flips_in_signed_size_root_and_identity_never_advance() {
    let signer = TestSigner::new();
    let raw = signer.envelope("log-a", "key-1", 0, 1, ROOT_1, &[], 1);
    let signed_prefix_len = raw.len() - 8 - 8 - 2 - 64;
    for bit in 0..(signed_prefix_len * 8) {
        let mut corrupt = raw.clone();
        corrupt[bit / 8] ^= 1 << (bit % 8);
        let mut engine = build_engine(&signer, &["log-a"]);
        assert_ne!(engine.submit(&corrupt, 1).code, ErrorCode::Advanced);
        assert_eq!(engine.current("log-a").unwrap().tree_size, 0);
    }
}

#[test]
fn pending_limit_and_size_rollback_fail_closed() {
    let signer = TestSigner::new();
    let mut engine = build_engine(&signer, &["log-a"]);
    for i in 0_u8..16 {
        let i = u64::from(i);
        let raw = signer.envelope(
            "log-a",
            "key-1",
            100 + i,
            200 + i,
            [u8::try_from(i).expect("test range"); 32],
            &[],
            i,
        );
        assert_eq!(engine.submit(&raw, i).code, ErrorCode::PendingBase);
    }
    let seventeenth = signer.envelope("log-a", "key-1", 999, 1_000, [99; 32], &[], 999);
    assert_eq!(
        engine.submit(&seventeenth, 999).code,
        ErrorCode::PendingLimit
    );

    let first = signer.envelope("log-a", "key-1", 0, 1, ROOT_1, &[], 1);
    let mut fresh = build_engine(&signer, &["log-a"]);
    fresh.submit(&first, 1);
    let rollback = signer.envelope("log-a", "key-1", 0, 0, ROOT_0, &[], 2);
    assert_eq!(fresh.submit(&rollback, 2).code, ErrorCode::SizeRollback);
}

#[test]
fn archive_cursor_rollback_is_rejected_without_mutation() {
    let signer = TestSigner::new();
    let mut engine = build_engine(&signer, &["log-a"]);
    let leaf_1 = hex32("2ae1c19c0cbd378e46c927a9f3611923ec07cc1ae357502a09536d455275cf21");
    let first = signer.envelope("log-a", "key-1", 0, 1, ROOT_1, &[], 1);
    assert_eq!(engine.submit(&first, 100).code, ErrorCode::Advanced);

    let second = signer.envelope("log-a", "key-1", 1, 2, ROOT_2, &[leaf_1], 2);
    let before = engine.snapshot();
    let rejected = engine.submit(&second, 99);
    assert_eq!(rejected.code, ErrorCode::ArchiveCursorRollback);
    assert_eq!(engine.snapshot(), before);

    assert_eq!(engine.submit(&second, 101).code, ErrorCode::Advanced);
    assert_eq!(engine.current("log-a").unwrap().tree_size, 2);
    assert_eq!(engine.current("log-a").unwrap().archive_cursor, 101);
}

#[test]
fn same_base_candidates_are_all_decided_after_advance() {
    let signer = TestSigner::new();
    let mut engine = build_engine(&signer, &["log-a"]);
    let leaf_1 = hex32("2ae1c19c0cbd378e46c927a9f3611923ec07cc1ae357502a09536d455275cf21");
    let leaf_2 = hex32("d81f51781eeb8f46a0e112e86ca335896ccc12cee00e6cfcf58a3501129dfc24");
    let first = signer.envelope("log-a", "key-1", 0, 1, ROOT_1, &[], 1);
    assert_eq!(engine.submit(&first, 10).code, ErrorCode::Advanced);

    let third_a = signer.envelope("log-a", "key-1", 2, 3, ROOT_3, &[leaf_2], 30);
    let third_b = signer.envelope("log-a", "key-1", 2, 3, ROOT_3, &[leaf_2], 31);
    assert_eq!(engine.submit(&third_a, 30).code, ErrorCode::PendingBase);
    assert_eq!(engine.submit(&third_b, 30).code, ErrorCode::PendingBase);

    let second = signer.envelope("log-a", "key-1", 1, 2, ROOT_2, &[leaf_1], 20);
    assert_eq!(engine.submit(&second, 20).code, ErrorCode::Advanced);
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.logs["log-a"].tree_size, 3);
    assert!(snapshot.pending.get("log-a").is_none_or(Vec::is_empty));
    assert_eq!(snapshot.seen_envelopes.len(), 4);
}

#[test]
fn equal_key_pending_delivery_orders_have_identical_bytes() {
    let signer = TestSigner::new();
    let leaf_1 = hex32("2ae1c19c0cbd378e46c927a9f3611923ec07cc1ae357502a09536d455275cf21");
    let leaf_2 = hex32("d81f51781eeb8f46a0e112e86ca335896ccc12cee00e6cfcf58a3501129dfc24");
    let first = signer.envelope("log-a", "key-1", 0, 1, ROOT_1, &[], 1);
    let second = signer.envelope("log-a", "key-1", 1, 2, ROOT_2, &[leaf_1], 2);
    let third_a = signer.envelope("log-a", "key-1", 2, 3, ROOT_3, &[leaf_2], 30);
    let third_b = signer.envelope("log-a", "key-1", 2, 3, ROOT_3, &[leaf_2], 31);

    let mut left = build_engine(&signer, &["log-a"]);
    left.submit(&first, 10);
    left.submit(&third_a, 30);
    left.submit(&third_b, 30);
    left.submit(&second, 20);

    let mut right = build_engine(&signer, &["log-a"]);
    right.submit(&first, 10);
    right.submit(&third_b, 30);
    right.submit(&third_a, 30);
    right.submit(&second, 20);

    assert_eq!(left.stable_bytes(), right.stable_bytes());
}
