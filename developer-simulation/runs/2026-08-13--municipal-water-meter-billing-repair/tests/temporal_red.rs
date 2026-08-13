use water_repair::{
    BilledInterval, Export, Installation, Reading, ReadingKind, evaluate_candidate,
};

#[test]
fn six_digit_maximum_to_zero_is_an_exact_rollover_repair() {
    let export = Export {
        installations: vec![Installation {
            service_point_id: 1,
            installation_id: 500,
            meter_id: 10,
            installed_at: 0,
            removed_at: None,
            register_width: 6,
        }],
        readings: vec![
            Reading {
                service_point_id: 1,
                meter_id: 10,
                source_id: 100,
                at: 0,
                value_litres: 999_999,
                kind: ReadingKind::Actual,
                supersedes: None,
            },
            Reading {
                service_point_id: 1,
                meter_id: 10,
                source_id: 101,
                at: 1,
                value_litres: 0,
                kind: ReadingKind::Actual,
                supersedes: None,
            },
        ],
        billed_intervals: vec![BilledInterval {
            service_point_id: 1,
            interval_id: 900,
            start_at: 0,
            end_at: 1,
            issued_usage_litres: 0,
        }],
        prior_adjustments: vec![],
    };

    let plan = evaluate_candidate(&export, 1).expect("valid rollover history");
    assert!(plan.review_cases.is_empty());
    assert_eq!(plan.repairs.len(), 1);
    assert_eq!(plan.repairs[0].old_usage_litres, 0);
    assert_eq!(plan.repairs[0].recomputed_usage_litres, 1);
    assert_eq!(plan.repairs[0].adjustment_litres, 1);
    assert_eq!(plan.repairs[0].source_reading_ids, vec![100, 101]);
    assert_eq!(plan.repairs[0].installation_ids, vec![500]);
    assert_eq!(plan.repairs[0].installation_meter_ids, vec![10]);
    assert_eq!(plan.repairs[0].reason_code, "ROLLOVER_REPAIR");
}
