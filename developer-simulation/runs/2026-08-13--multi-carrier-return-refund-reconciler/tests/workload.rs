use return_reconciler_trial1::{
    deterministic_shuffle, generate_representative, reconcile, reference_reconcile,
};

#[test]
fn representative_generator_has_exact_disclosed_shape() {
    // Break caught: a "large" benchmark can silently omit the required
    // shape or adversarial population and therefore measure an easier job.
    let snapshot = generate_representative(20_260_813);
    assert_eq!(snapshot.returns.len(), 25_000);
    assert_eq!(
        snapshot
            .returns
            .iter()
            .map(|r| r.lines.len())
            .sum::<usize>(),
        70_000
    );
    assert_eq!(snapshot.events.len(), 250_000);
    let parcels = snapshot
        .events
        .iter()
        .filter_map(|event| match event {
            return_reconciler_trial1::Event::WarehouseScan {
                return_id,
                parcel_id,
                ..
            }
            | return_reconciler_trial1::Event::Carrier {
                return_id,
                parcel_id,
                ..
            } => Some((return_id, parcel_id)),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert_eq!(parcels, 40_000);
    assert_eq!(
        snapshot
            .returns
            .iter()
            .filter(|authorization| authorization.return_id.starts_with("adv-return-"))
            .count(),
        500
    );
    assert!(
        snapshot
            .returns
            .iter()
            .flat_map(|authorization| &authorization.lines)
            .filter(|line| {
                line.authorized_qty == 2
                    && (line.paid_subtotal_cents + line.tax_cents - line.discount_cents) % 2 == 1
            })
            .count()
            >= 100,
        "representative data must exercise remainder-cent allocation"
    );
}

#[test]
fn ten_deterministic_shuffles_are_byte_identical() {
    // Break caught: hash iteration or arrival-order conflict/correction logic
    // can make operator plans differ for identical immutable inputs.
    let snapshot = generate_representative(20_260_813);
    let expected = serde_json::to_vec(&reconcile(&snapshot).unwrap()).unwrap();
    for seed in 0..10 {
        let mut shuffled = snapshot.clone();
        deterministic_shuffle(&mut shuffled, seed);
        assert_eq!(
            serde_json::to_vec(&reconcile(&shuffled).unwrap()).unwrap(),
            expected
        );
    }
}

#[test]
fn representative_candidate_and_reference_agree_exactly() {
    // Break caught: candidate optimizations can diverge only at realistic
    // cardinality or on one of the 500 seeded adversarial returns.
    let snapshot = generate_representative(20_260_813);
    assert_eq!(
        reconcile(&snapshot).unwrap(),
        reference_reconcile(&snapshot).unwrap()
    );
}
