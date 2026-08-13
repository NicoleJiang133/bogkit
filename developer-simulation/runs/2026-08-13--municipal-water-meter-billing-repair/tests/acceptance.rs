use std::fs;

use tempfile::tempdir;
use water_repair::{
    BilledInterval, Export, Installation, PriorAdjustment, PublicationFailure, Reading,
    ReadingKind, RepairPlan, canonical_plan_bytes, evaluate_candidate, evaluate_reference,
    generated_case, publish_plan,
};

fn installation(service: u32, install: u64, meter: u32, width: u8) -> Installation {
    Installation {
        service_point_id: service,
        installation_id: install,
        meter_id: meter,
        installed_at: 0,
        removed_at: None,
        register_width: width,
    }
}

fn reading(service: u32, meter: u32, source: u64, at: i64, value: u32) -> Reading {
    Reading {
        service_point_id: service,
        meter_id: meter,
        source_id: source,
        at,
        value_litres: value,
        kind: ReadingKind::Actual,
        supersedes: None,
    }
}

fn bill(service: u32, interval: u64, start: i64, end: i64, old: i64) -> BilledInterval {
    BilledInterval {
        service_point_id: service,
        interval_id: interval,
        start_at: start,
        end_at: end,
        issued_usage_litres: old,
    }
}

#[test]
fn corrected_actual_changes_only_the_two_intervals_anchored_by_the_estimate() {
    let mut estimate = reading(1, 10, 2, 1, 200);
    estimate.kind = ReadingKind::Estimated;
    let mut correction = reading(1, 10, 3, 1, 210);
    correction.kind = ReadingKind::CorrectedActual;
    correction.supersedes = Some(2);
    let export = Export {
        installations: vec![installation(1, 100, 10, 6)],
        readings: vec![
            reading(1, 10, 4, 2, 300),
            correction,
            reading(1, 10, 1, 0, 100),
            reading(1, 10, 5, 3, 400),
            estimate,
        ],
        billed_intervals: vec![
            bill(1, 12, 2, 3, 100),
            bill(1, 10, 0, 1, 100),
            bill(1, 11, 1, 2, 100),
        ],
        prior_adjustments: vec![],
    };

    let plan = evaluate_candidate(&export, 2).unwrap();
    assert_eq!(plan.repairs.len(), 2);
    assert_eq!(plan.repairs[0].interval_id, 10);
    assert_eq!(plan.repairs[0].adjustment_litres, 10);
    assert_eq!(plan.repairs[0].source_reading_ids, vec![1, 3]);
    assert_eq!(plan.repairs[0].reason_code, "CORRECTION_REPAIR");
    assert_eq!(plan.repairs[1].interval_id, 11);
    assert_eq!(plan.repairs[1].adjustment_litres, -10);
    assert_eq!(plan.repairs[1].source_reading_ids, vec![3, 4]);
    assert!(plan.repairs.iter().all(|repair| repair.interval_id != 12));
}

#[test]
fn replacement_boundary_closes_old_and_opens_new_register() {
    let export = Export {
        installations: vec![
            Installation {
                service_point_id: 1,
                installation_id: 100,
                meter_id: 10,
                installed_at: 0,
                removed_at: Some(1),
                register_width: 6,
            },
            Installation {
                service_point_id: 1,
                installation_id: 101,
                meter_id: 11,
                installed_at: 1,
                removed_at: None,
                register_width: 8,
            },
        ],
        readings: vec![
            reading(1, 11, 4, 2, 80),
            reading(1, 10, 1, 0, 900),
            reading(1, 11, 3, 1, 10),
            reading(1, 10, 2, 1, 950),
        ],
        billed_intervals: vec![bill(1, 20, 0, 2, 100)],
        prior_adjustments: vec![],
    };

    let plan = evaluate_candidate(&export, 3).unwrap();
    assert_eq!(plan.repairs.len(), 1);
    assert_eq!(plan.repairs[0].recomputed_usage_litres, 120);
    assert_eq!(plan.repairs[0].adjustment_litres, 20);
    assert_eq!(plan.repairs[0].source_reading_ids, vec![1, 2, 3, 4]);
    assert_eq!(plan.repairs[0].installation_ids, vec![100, 101]);
    assert_eq!(plan.repairs[0].reason_code, "METER_REPLACEMENT_REPAIR");
}

#[test]
fn ambiguous_regressions_fail_closed_and_six_and_eight_digit_rollovers_differ() {
    let export = Export {
        installations: vec![
            installation(1, 101, 11, 6),
            installation(2, 102, 12, 8),
            installation(3, 103, 13, 6),
        ],
        readings: vec![
            reading(1, 11, 1, 0, 600_000),
            reading(1, 11, 2, 1, 100_000),
            reading(2, 12, 3, 0, 99_999_999),
            reading(2, 12, 4, 1, 0),
            reading(3, 13, 5, 0, 100),
            reading(3, 13, 6, 1, 90),
        ],
        billed_intervals: vec![
            bill(1, 10, 0, 1, 0),
            bill(2, 20, 0, 1, 0),
            bill(3, 30, 0, 1, 0),
        ],
        prior_adjustments: vec![],
    };

    let plan = evaluate_candidate(&export, 50).unwrap();
    assert_eq!(plan.repairs.len(), 1);
    assert_eq!(plan.repairs[0].service_point_id, 2);
    assert_eq!(plan.repairs[0].recomputed_usage_litres, 1);
    assert_eq!(plan.review_cases.len(), 2);
    assert_eq!(
        plan.review_cases[0].reason_code,
        "AMBIGUOUS_REGISTER_REGRESSION"
    );
    assert_eq!(plan.review_cases[0].detail_ids, vec![1, 2]);
    assert_eq!(
        plan.review_cases[1].reason_code,
        "AMBIGUOUS_REGISTER_REGRESSION"
    );
}

#[test]
fn exact_duplicate_is_idempotent_but_conflicting_source_id_is_global_failure() {
    let base = Export {
        installations: vec![installation(1, 100, 10, 6)],
        readings: vec![reading(1, 10, 1, 0, 100), reading(1, 10, 2, 1, 150)],
        billed_intervals: vec![bill(1, 10, 0, 1, 40)],
        prior_adjustments: vec![],
    };
    let expected = evaluate_candidate(&base, 1).unwrap();
    let mut duplicated = base.clone();
    duplicated.readings.push(duplicated.readings[0].clone());
    assert_eq!(evaluate_candidate(&duplicated, 2).unwrap(), expected);

    let mut conflicting = base;
    conflicting.readings.push(reading(1, 10, 1, 0, 101));
    assert_eq!(
        evaluate_candidate(&conflicting, 100).unwrap_err(),
        "CONFLICTING_SOURCE_ID:1"
    );
}

#[test]
fn invalid_service_does_not_suppress_unrelated_valid_repair() {
    let export = Export {
        installations: vec![installation(1, 100, 10, 6), installation(2, 200, 20, 6)],
        readings: vec![
            reading(1, 10, 1, 0, 0),
            reading(1, 10, 2, 1, 10),
            reading(2, 999, 3, 0, 0),
            reading(2, 20, 4, 1, 10),
        ],
        billed_intervals: vec![bill(1, 10, 0, 1, 5), bill(2, 20, 0, 1, 5)],
        prior_adjustments: vec![],
    };
    let plan = evaluate_candidate(&export, 4).unwrap();
    assert_eq!(plan.repairs.len(), 1);
    assert_eq!(plan.repairs[0].service_point_id, 1);
    assert_eq!(plan.review_cases.len(), 1);
    assert_eq!(plan.review_cases[0].service_point_id, 2);
    assert_eq!(
        plan.review_cases[0].reason_code,
        "READING_FOR_INACTIVE_METER"
    );
}

#[test]
fn overlap_gap_malformed_width_out_of_range_and_equal_time_are_review_cases() {
    let cases = [
        (
            Export {
                installations: vec![
                    Installation {
                        service_point_id: 1,
                        installation_id: 1,
                        meter_id: 1,
                        installed_at: 0,
                        removed_at: Some(2),
                        register_width: 6,
                    },
                    Installation {
                        service_point_id: 1,
                        installation_id: 2,
                        meter_id: 2,
                        installed_at: 1,
                        removed_at: None,
                        register_width: 6,
                    },
                ],
                readings: vec![],
                billed_intervals: vec![],
                prior_adjustments: vec![],
            },
            "OVERLAPPING_INSTALLATIONS",
        ),
        (
            Export {
                installations: vec![
                    Installation {
                        service_point_id: 1,
                        installation_id: 1,
                        meter_id: 1,
                        installed_at: 0,
                        removed_at: Some(1),
                        register_width: 6,
                    },
                    Installation {
                        service_point_id: 1,
                        installation_id: 2,
                        meter_id: 2,
                        installed_at: 2,
                        removed_at: None,
                        register_width: 6,
                    },
                ],
                readings: vec![],
                billed_intervals: vec![],
                prior_adjustments: vec![],
            },
            "INSTALLATION_GAP_NO_POLICY",
        ),
        (
            Export {
                installations: vec![installation(1, 1, 1, 7)],
                readings: vec![],
                billed_intervals: vec![],
                prior_adjustments: vec![],
            },
            "MALFORMED_REGISTER_WIDTH",
        ),
        (
            Export {
                installations: vec![installation(1, 1, 1, 6)],
                readings: vec![reading(1, 1, 10, 0, 1_000_000)],
                billed_intervals: vec![],
                prior_adjustments: vec![],
            },
            "REGISTER_VALUE_OUT_OF_RANGE",
        ),
        (
            Export {
                installations: vec![installation(1, 1, 1, 6)],
                readings: vec![reading(1, 1, 10, 0, 1), reading(1, 1, 11, 0, 2)],
                billed_intervals: vec![],
                prior_adjustments: vec![],
            },
            "AMBIGUOUS_EQUAL_TIMESTAMP",
        ),
    ];
    for (export, expected) in cases {
        let plan = evaluate_candidate(&export, 1).unwrap();
        assert_eq!(plan.review_cases[0].reason_code, expected);
    }
}

#[test]
fn prior_adjustments_are_part_of_old_usage_and_issued_interval_is_not_rewritten() {
    let export = Export {
        installations: vec![installation(1, 100, 10, 6)],
        readings: vec![reading(1, 10, 1, 0, 100), reading(1, 10, 2, 1, 180)],
        billed_intervals: vec![bill(1, 10, 0, 1, 50)],
        prior_adjustments: vec![PriorAdjustment {
            service_point_id: 1,
            interval_id: 10,
            adjustment_id: 500,
            adjustment_litres: 20,
        }],
    };
    let plan = evaluate_candidate(&export, 1).unwrap();
    assert_eq!(plan.repairs[0].old_usage_litres, 70);
    assert_eq!(plan.repairs[0].recomputed_usage_litres, 80);
    assert_eq!(plan.repairs[0].adjustment_litres, 10);
    assert_eq!(export.billed_intervals[0].issued_usage_litres, 50);
}

fn permute<T>(items: &mut [T], mut state: u64) {
    for index in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let target = usize::try_from(state % (index as u64 + 1)).unwrap();
        items.swap(index, target);
    }
}

#[test]
fn ten_permutations_three_batch_sizes_are_byte_identical() {
    let base = generated_case(42, 12);
    let expected = canonical_plan_bytes(&evaluate_candidate(&base, 1).unwrap()).unwrap();
    for seed in 0..10 {
        let mut shuffled = base.clone();
        permute(&mut shuffled.readings, seed + 1);
        permute(&mut shuffled.installations, seed + 101);
        permute(&mut shuffled.billed_intervals, seed + 201);
        for batch_size in [1, 7, 64] {
            let actual =
                canonical_plan_bytes(&evaluate_candidate(&shuffled, batch_size).unwrap()).unwrap();
            assert_eq!(actual, expected, "seed {seed}, batch {batch_size}");
        }
    }
}

#[test]
fn candidate_agrees_with_chronological_oracle_on_one_hundred_seeds() {
    for seed in 0..100 {
        let export = generated_case(seed, 8);
        assert_eq!(
            evaluate_candidate(&export, 5).unwrap(),
            evaluate_reference(&export).unwrap(),
            "seed {seed}"
        );
    }
}

#[test]
fn failures_preserve_prior_report_and_success_replaces_it_completely() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("plan.json");
    fs::write(&output, b"prior-complete-report\n").unwrap();
    let prior = fs::read(&output).unwrap();
    let plan = RepairPlan::default();

    assert!(publish_plan(None, &output, &plan, PublicationFailure::BeforeTemporary).is_err());
    assert_eq!(fs::read(&output).unwrap(), prior);
    assert!(publish_plan(None, &output, &plan, PublicationFailure::BeforeFinalRename).is_err());
    assert_eq!(fs::read(&output).unwrap(), prior);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);

    publish_plan(None, &output, &plan, PublicationFailure::None).unwrap();
    assert_eq!(
        fs::read(&output).unwrap(),
        canonical_plan_bytes(&plan).unwrap()
    );
}

#[test]
fn detectable_input_output_identity_is_rejected_without_modification() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("export.json");
    fs::write(&input, b"immutable-input\n").unwrap();
    let result = publish_plan(
        Some(&input),
        &input,
        &RepairPlan::default(),
        PublicationFailure::None,
    );
    assert_eq!(result.unwrap_err(), "INPUT_OUTPUT_PATH_IDENTITY");
    assert_eq!(fs::read(&input).unwrap(), b"immutable-input\n");
}
