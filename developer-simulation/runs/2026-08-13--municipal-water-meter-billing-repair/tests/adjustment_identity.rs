use std::fs;
use std::process::Command;

use tempfile::tempdir;
use water_repair::{
    BilledInterval, Export, Installation, PriorAdjustment, Reading, ReadingKind,
    evaluate_candidate, evaluate_reference,
};

fn base_export() -> Export {
    Export {
        installations: vec![Installation {
            service_point_id: 1,
            installation_id: 100,
            meter_id: 10,
            installed_at: 0,
            removed_at: None,
            register_width: 6,
        }],
        readings: vec![
            Reading {
                service_point_id: 1,
                meter_id: 10,
                source_id: 1,
                at: 0,
                value_litres: 100,
                kind: ReadingKind::Actual,
                supersedes: None,
            },
            Reading {
                service_point_id: 1,
                meter_id: 10,
                source_id: 2,
                at: 1,
                value_litres: 200,
                kind: ReadingKind::Actual,
                supersedes: None,
            },
            Reading {
                service_point_id: 1,
                meter_id: 10,
                source_id: 3,
                at: 2,
                value_litres: 300,
                kind: ReadingKind::Actual,
                supersedes: None,
            },
        ],
        billed_intervals: vec![
            BilledInterval {
                service_point_id: 1,
                interval_id: 10,
                start_at: 0,
                end_at: 1,
                issued_usage_litres: 80,
            },
            BilledInterval {
                service_point_id: 1,
                interval_id: 11,
                start_at: 1,
                end_at: 2,
                issued_usage_litres: 80,
            },
        ],
        prior_adjustments: Vec::new(),
    }
}

fn adjustment(interval_id: u64, adjustment_litres: i64) -> PriorAdjustment {
    PriorAdjustment {
        service_point_id: 1,
        interval_id,
        adjustment_id: 500,
        adjustment_litres,
    }
}

#[test]
fn same_adjustment_id_on_different_intervals_rejects_before_derivation() {
    let mut export = base_export();
    export.prior_adjustments = vec![adjustment(10, 5), adjustment(11, 5)];

    for order in [export.prior_adjustments.clone(), {
        let mut reversed = export.prior_adjustments.clone();
        reversed.reverse();
        reversed
    }] {
        export.prior_adjustments = order;
        assert_eq!(
            evaluate_candidate(&export, 1).unwrap_err(),
            "CONFLICTING_ADJUSTMENT_ID:500"
        );
        assert_eq!(
            evaluate_reference(&export).unwrap_err(),
            "CONFLICTING_ADJUSTMENT_ID:500"
        );
    }
}

#[test]
fn exact_adjustment_retry_is_idempotent_for_candidate_and_reference() {
    let mut once = base_export();
    once.prior_adjustments = vec![adjustment(10, 5)];
    let mut retried = once.clone();
    retried.prior_adjustments.push(adjustment(10, 5));

    let expected = evaluate_candidate(&once, 1).unwrap();
    assert_eq!(evaluate_candidate(&retried, 2).unwrap(), expected);
    assert_eq!(evaluate_reference(&retried).unwrap(), expected);
    retried.prior_adjustments.reverse();
    assert_eq!(evaluate_candidate(&retried, 64).unwrap(), expected);
    assert_eq!(evaluate_reference(&retried).unwrap(), expected);
}

#[test]
fn conflicting_amount_reuse_rejects_identically_and_preserves_prior_report() {
    let mut export = base_export();
    export.prior_adjustments = vec![adjustment(10, 5), adjustment(10, 6)];
    let expected_error = "CONFLICTING_ADJUSTMENT_ID:500";
    assert_eq!(evaluate_candidate(&export, 7).unwrap_err(), expected_error);
    assert_eq!(evaluate_reference(&export).unwrap_err(), expected_error);

    let directory = tempdir().unwrap();
    let input = directory.path().join("conflict.json");
    let output = directory.path().join("prior-plan.json");
    let prior = b"prior-complete-report\n";
    fs::write(&input, serde_json::to_vec_pretty(&export).unwrap()).unwrap();
    fs::write(&output, prior).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_water-repair"))
        .arg("process")
        .arg("--input")
        .arg(&input)
        .arg("--output")
        .arg(&output)
        .status()
        .unwrap();
    assert!(!result.success());
    assert_eq!(fs::read(&output).unwrap(), prior);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}
