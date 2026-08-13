use return_reconciler_trial1::{
    AuthorizedLine, Event, PaymentStatus, ReturnAuthorization, Snapshot, reconcile,
    reference_reconcile,
};

fn line() -> AuthorizedLine {
    AuthorizedLine {
        line_id: "line-1".into(),
        sku: "sku-1".into(),
        authorized_qty: 1,
        paid_subtotal_cents: 1_000,
        tax_cents: 80,
        discount_cents: 80,
        prior_successful_refund_cents: 0,
    }
}

fn authorization() -> ReturnAuthorization {
    ReturnAuthorization {
        return_id: "return-1".into(),
        captured_cents: 1_000,
        lines: vec![line()],
    }
}

fn scan(event_id: &str, quantity: u32) -> Event {
    Event::WarehouseScan {
        event_id: event_id.into(),
        source_system_id: "warehouse-east".into(),
        source_timestamp: 10,
        ingestion_id: 10,
        return_id: "return-1".into(),
        parcel_id: "parcel-1".into(),
        sku: "sku-1".into(),
        quantity,
    }
}

fn scan_for(event_id: &str, return_id: &str, parcel_id: &str, sku: &str, quantity: u32) -> Event {
    Event::WarehouseScan {
        event_id: event_id.into(),
        source_system_id: "warehouse-east".into(),
        source_timestamp: 10,
        ingestion_id: 10,
        return_id: return_id.into(),
        parcel_id: parcel_id.into(),
        sku: sku.into(),
        quantity,
    }
}

fn payment(event_id: &str, refund_id: &str, status: PaymentStatus) -> Event {
    Event::PaymentResult {
        event_id: event_id.into(),
        source_system_id: "payment-provider".into(),
        source_timestamp: 20,
        ingestion_id: 20,
        return_id: "return-1".into(),
        line_id: "line-1".into(),
        refund_id: refund_id.into(),
        amount_cents: 1_000,
        status,
    }
}

#[test]
fn conflicting_retry_is_quarantined_and_changes_no_proposed_refund() {
    // Break caught: accepting either payload for a reused event ID can create
    // a refund even though the source event is untrustworthy.
    let snapshot = Snapshot {
        snapshot_id: "fixture-conflict".into(),
        returns: vec![authorization()],
        events: vec![scan("scan-conflict", 1), scan("scan-conflict", 2)],
    };

    let report = reconcile(&snapshot).expect("valid snapshot should reconcile");

    assert_eq!(report.summary.proposed_cents, 0);
    assert_eq!(report.summary.conflicting_event_ids, 1);
    assert_eq!(report.returns[0].lines[0].proposed_quantity, 0);
    assert_eq!(report.returns[0].lines[0].proposed_cents, 0);
    assert_eq!(report.returns[0].disposition, "review");
    assert_eq!(report.returns[0].review_codes, ["conflicting_event_id"]);
}

// Keep the public payment enum in the first compile contract so later tests
// cannot accidentally model pending and failed as one boolean.
#[test]
fn payment_status_contract_has_distinct_states() {
    assert_ne!(PaymentStatus::Successful, PaymentStatus::Failed);
    assert_ne!(PaymentStatus::Failed, PaymentStatus::Pending);
}

#[test]
fn split_parcels_conserve_quantity_and_allocate_remainder_cents_deterministically() {
    // Break caught: counting parcels independently can refund the same
    // authorization twice or lose the one remainder cent.
    let mut auth = authorization();
    auth.lines[0].authorized_qty = 2;
    auth.lines[0].paid_subtotal_cents = 1_001;
    auth.captured_cents = 1_001;
    let snapshot = Snapshot {
        snapshot_id: "fixture-split".into(),
        returns: vec![auth],
        events: vec![
            scan_for("scan-b", "return-1", "parcel-b", "sku-1", 1),
            scan_for("scan-a", "return-1", "parcel-a", "sku-1", 1),
        ],
    };

    let report = reconcile(&snapshot).unwrap();
    let line = &report.returns[0].lines[0];
    assert_eq!(line.accepted_scan_quantity, 2);
    assert_eq!(line.review_scan_quantity, 0);
    assert_eq!(line.proposed_quantity, 2);
    assert_eq!(line.proposed_cents, 1_001);
    assert_eq!(line.provenance_event_ids, ["scan-a", "scan-b"]);
}

#[test]
fn exact_retry_is_idempotent_and_event_order_does_not_change_bytes() {
    // Break caught: iterating arrival order or counting an exact retry can
    // change quantities, provenance ordering, or serialized output.
    let events = vec![scan("scan-1", 1), scan("scan-1", 1)];
    let forward = Snapshot {
        snapshot_id: "fixture-idempotent".into(),
        returns: vec![authorization()],
        events: events.clone(),
    };
    let reverse = Snapshot {
        snapshot_id: "fixture-idempotent".into(),
        returns: vec![authorization()],
        events: events.into_iter().rev().collect(),
    };

    let forward_bytes = serde_json::to_vec(&reconcile(&forward).unwrap()).unwrap();
    let reverse_bytes = serde_json::to_vec(&reconcile(&reverse).unwrap()).unwrap();
    assert_eq!(forward_bytes, reverse_bytes);
    assert_eq!(
        reconcile(&forward).unwrap().summary.exact_duplicate_events,
        1
    );
    assert_eq!(reconcile(&forward).unwrap().summary.proposed_cents, 1_000);
}

#[test]
fn correction_before_target_in_input_supersedes_only_that_scan() {
    // Break caught: applying corrections in arrival order can orphan a valid
    // correction or mutate every scan for the same SKU.
    let mut auth = authorization();
    auth.lines[0].authorized_qty = 2;
    auth.lines[0].paid_subtotal_cents = 2_000;
    auth.captured_cents = 2_000;
    let correction = Event::Correction {
        event_id: "correction-1".into(),
        source_system_id: "warehouse-east".into(),
        source_timestamp: 11,
        ingestion_id: 1,
        return_id: "return-1".into(),
        target_event_id: "scan-target".into(),
        replacement_sku: "sku-1".into(),
        replacement_quantity: 1,
    };
    let snapshot = Snapshot {
        snapshot_id: "fixture-correction".into(),
        returns: vec![auth],
        events: vec![
            correction,
            scan_for("scan-target", "return-1", "parcel-a", "sku-1", 2),
        ],
    };

    let report = reconcile(&snapshot).unwrap();
    let line = &report.returns[0].lines[0];
    assert_eq!(line.proposed_quantity, 1);
    assert_eq!(line.proposed_cents, 1_000);
    assert_eq!(line.provenance_event_ids, ["correction-1", "scan-target"]);
}

#[test]
fn excess_and_unknown_units_are_reviewed_but_never_refunded() {
    // Break caught: matching by return alone can turn over-quantity or
    // unknown SKU units into refundable units.
    let snapshot = Snapshot {
        snapshot_id: "fixture-unmatched".into(),
        returns: vec![authorization()],
        events: vec![
            scan_for("scan-known", "return-1", "parcel-a", "sku-1", 2),
            scan_for("scan-unknown", "return-1", "parcel-b", "sku-other", 1),
        ],
    };

    let report = reconcile(&snapshot).unwrap();
    let plan = &report.returns[0];
    assert_eq!(plan.lines[0].proposed_quantity, 1);
    assert_eq!(plan.lines[0].review_scan_quantity, 1);
    assert_eq!(plan.unmatched_scan_quantity, 1);
    assert_eq!(plan.lines[0].proposed_cents, 1_000);
    assert_eq!(plan.disposition, "review");
    assert_eq!(plan.review_codes, ["excess_quantity", "unauthorized_sku"]);
}

#[test]
fn payment_results_distinguish_success_pending_and_failure() {
    // Break caught: treating all non-success results alike can issue a
    // second refund while the first attempt is still inconclusive.
    let base = Snapshot {
        snapshot_id: "fixture-payment".into(),
        returns: vec![authorization()],
        events: vec![scan("scan-1", 1)],
    };

    let mut failed = base.clone();
    failed
        .events
        .push(payment("payment-failed", "refund-1", PaymentStatus::Failed));
    assert_eq!(reconcile(&failed).unwrap().summary.proposed_cents, 1_000);

    let mut pending = base.clone();
    pending.events.push(payment(
        "payment-pending",
        "refund-1",
        PaymentStatus::Pending,
    ));
    let pending_report = reconcile(&pending).unwrap();
    assert_eq!(pending_report.summary.proposed_cents, 0);
    assert_eq!(
        pending_report.returns[0].review_codes,
        ["payment_inconclusive"]
    );

    let mut successful_then_delayed_pending = base;
    successful_then_delayed_pending.events.extend([
        payment("payment-success", "refund-1", PaymentStatus::Successful),
        payment("payment-delayed", "refund-1", PaymentStatus::Pending),
    ]);
    let successful_report = reconcile(&successful_then_delayed_pending).unwrap();
    assert_eq!(successful_report.summary.proposed_cents, 0);
    assert_eq!(
        successful_report.returns[0].review_codes,
        ["payment_already_successful"]
    );
}

#[test]
fn line_and_return_payment_caps_are_never_exceeded() {
    // Break caught: independently refunding line values can exceed the
    // captured payment after authoritative prior refunds.
    let authorization = ReturnAuthorization {
        return_id: "return-1".into(),
        captured_cents: 1_000,
        lines: vec![
            AuthorizedLine {
                line_id: "line-a".into(),
                sku: "sku-a".into(),
                authorized_qty: 1,
                paid_subtotal_cents: 700,
                tax_cents: 0,
                discount_cents: 0,
                prior_successful_refund_cents: 100,
            },
            AuthorizedLine {
                line_id: "line-b".into(),
                sku: "sku-b".into(),
                authorized_qty: 1,
                paid_subtotal_cents: 700,
                tax_cents: 0,
                discount_cents: 0,
                prior_successful_refund_cents: 0,
            },
        ],
    };
    let snapshot = Snapshot {
        snapshot_id: "fixture-caps".into(),
        returns: vec![authorization],
        events: vec![
            scan_for("scan-a", "return-1", "parcel-a", "sku-a", 1),
            scan_for("scan-b", "return-1", "parcel-b", "sku-b", 1),
        ],
    };

    let report = reconcile(&snapshot).unwrap();
    assert_eq!(report.returns[0].lines[0].proposed_quantity, 0);
    assert_eq!(report.returns[0].lines[0].proposed_cents, 0);
    assert_eq!(report.returns[0].lines[1].proposed_quantity, 1);
    assert_eq!(report.returns[0].lines[1].proposed_cents, 700);
    assert_eq!(report.summary.proposed_cents, 700);
    assert_eq!(
        report.returns[0].review_codes,
        ["insufficient_whole_unit_capacity"]
    );
}

#[test]
fn cap_smaller_than_one_unit_never_emits_a_partially_funded_quantity() {
    // Break caught: min(requested cents, remaining return cap) can emit one
    // proposed unit paired with fewer cents than that deterministic unit costs.
    let authorization = ReturnAuthorization {
        return_id: "return-1".into(),
        captured_cents: 400,
        lines: vec![AuthorizedLine {
            line_id: "line-1".into(),
            sku: "sku-1".into(),
            authorized_qty: 2,
            paid_subtotal_cents: 1_001,
            tax_cents: 0,
            discount_cents: 0,
            prior_successful_refund_cents: 0,
        }],
    };
    let snapshot = Snapshot {
        snapshot_id: "fixture-sub-unit-cap".into(),
        returns: vec![authorization],
        events: vec![scan("scan-1", 1)],
    };

    let report = reconcile(&snapshot).unwrap();
    let line = &report.returns[0].lines[0];
    assert_eq!(line.accepted_scan_quantity, 1);
    assert_eq!(line.proposed_quantity, 0);
    assert_eq!(line.proposed_cents, 0);
    assert!(line.proposed_cents <= 400);
    assert_eq!(report.summary.proposed_quantity, 0);
    assert_eq!(report.summary.proposed_cents, 0);
    assert_eq!(report.returns[0].disposition, "review");
    assert_eq!(
        report.returns[0].review_codes,
        ["insufficient_whole_unit_capacity"]
    );
}

#[test]
fn reference_and_candidate_agree_on_disclosed_adversarial_fixture() {
    // Break caught: the optimized candidate can share a systematic mistake
    // with its own internal checks; this expected result comes from a
    // separately implemented calculation.
    let snapshot = Snapshot {
        snapshot_id: "fixture-reference".into(),
        returns: vec![authorization()],
        events: vec![
            scan_for("scan-known", "return-1", "parcel-a", "sku-1", 2),
            scan_for("scan-unknown", "return-1", "parcel-b", "sku-other", 1),
        ],
    };
    assert_eq!(
        serde_json::to_vec(&reconcile(&snapshot).unwrap()).unwrap(),
        serde_json::to_vec(&reference_reconcile(&snapshot).unwrap()).unwrap()
    );
}

#[test]
fn independent_verifier_rejects_tampered_provenance_even_when_totals_still_balance() {
    // Break caught: checking only caps and summary totals lets a report claim
    // provenance from an event that was never in the snapshot.
    let snapshot = Snapshot {
        snapshot_id: "fixture-verifier".into(),
        returns: vec![authorization()],
        events: vec![scan("scan-1", 1)],
    };
    let mut report = reconcile(&snapshot).unwrap();
    report.returns[0].lines[0]
        .provenance_event_ids
        .push("fabricated-event".into());

    assert!(return_reconciler_trial1::verify_report(&snapshot, &report).is_err());
}
