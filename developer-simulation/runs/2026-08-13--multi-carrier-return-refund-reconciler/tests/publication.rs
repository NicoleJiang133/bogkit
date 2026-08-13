use std::fs;

use return_reconciler_trial1::{
    AuthorizedLine, Event, FailPoint, PaymentStatus, ReturnAuthorization, Snapshot, reconcile,
    reconcile_file,
};

fn temp_dir() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-runtime");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("publication-")
        .tempdir_in(root)
        .unwrap()
}

fn snapshot_json(path: &std::path::Path) {
    let snapshot = Snapshot {
        snapshot_id: "publication-fixture".into(),
        returns: vec![ReturnAuthorization {
            return_id: "return-1".into(),
            captured_cents: 100,
            lines: vec![AuthorizedLine {
                line_id: "line-1".into(),
                sku: "sku-1".into(),
                authorized_qty: 1,
                paid_subtotal_cents: 100,
                tax_cents: 0,
                discount_cents: 0,
                prior_successful_refund_cents: 0,
            }],
        }],
        events: vec![Event::WarehouseScan {
            event_id: "scan-1".into(),
            source_system_id: "warehouse-east".into(),
            source_timestamp: 1,
            ingestion_id: 1,
            return_id: "return-1".into(),
            parcel_id: "parcel-1".into(),
            sku: "sku-1".into(),
            quantity: 1,
        }],
    };
    fs::write(path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
}

fn write_snapshot(path: &std::path::Path, snapshot: &Snapshot) {
    fs::write(path, serde_json::to_vec(snapshot).unwrap()).unwrap();
}

fn overflow_snapshot() -> Snapshot {
    let mut returns = Vec::new();
    let mut events = Vec::new();
    for index in 0..2 {
        let return_id = format!("return-{index}");
        let line_id = format!("line-{index}");
        let sku = format!("sku-{index}");
        returns.push(ReturnAuthorization {
            return_id: return_id.clone(),
            captured_cents: u64::MAX,
            lines: vec![AuthorizedLine {
                line_id,
                sku: sku.clone(),
                authorized_qty: 1,
                paid_subtotal_cents: u64::MAX,
                tax_cents: 0,
                discount_cents: 0,
                prior_successful_refund_cents: 0,
            }],
        });
        events.push(Event::WarehouseScan {
            event_id: format!("scan-{index}"),
            source_system_id: "warehouse-east".into(),
            source_timestamp: 1,
            ingestion_id: index,
            return_id,
            parcel_id: format!("parcel-{index}"),
            sku,
            quantity: 1,
        });
    }
    Snapshot {
        snapshot_id: "overflow-fixture".into(),
        returns,
        events,
    }
}

#[test]
fn injected_publication_failures_preserve_prior_report_or_no_report() {
    // Break caught: opening/truncating the requested output before a
    // complete report exists destroys the last known-good operator plan.
    for failpoint in [
        FailPoint::BeforeOutputCreation,
        FailPoint::AfterTemporaryComplete,
        FailPoint::BeforePublish,
    ] {
        let dir = temp_dir();
        let input = dir.path().join("snapshot.json");
        let output = dir.path().join("report.json");
        snapshot_json(&input);

        let error = reconcile_file(&input, &output, failpoint).unwrap_err();
        assert!(error.contains("injected"));
        assert!(!output.exists());

        fs::write(&output, b"prior-complete-report").unwrap();
        let error = reconcile_file(&input, &output, failpoint).unwrap_err();
        assert!(error.contains("injected"));
        assert_eq!(fs::read(&output).unwrap(), b"prior-complete-report");
    }
}

#[test]
fn output_aliasing_input_is_rejected_without_modifying_input() {
    // Break caught: publishing through a direct path, symlink, or hard link
    // to the input can destroy the authoritative snapshot export.
    let dir = temp_dir();
    let input = dir.path().join("snapshot.json");
    snapshot_json(&input);
    let original = fs::read(&input).unwrap();

    let error = reconcile_file(&input, &input, FailPoint::None).unwrap_err();
    assert!(error.contains("aliases input"));
    assert_eq!(fs::read(&input).unwrap(), original);

    #[cfg(unix)]
    {
        let alias = dir.path().join("hard-link.json");
        fs::hard_link(&input, &alias).unwrap();
        let error = reconcile_file(&input, &alias, FailPoint::None).unwrap_err();
        assert!(error.contains("aliases input"));
        assert_eq!(fs::read(&input).unwrap(), original);
    }
}

#[test]
fn relative_output_path_publishes_and_reports_success() {
    // Break caught: a relative output's parent is the empty path; syncing
    // that path after rename can publish the report but return an error.
    let dir = temp_dir();
    let input = dir.path().join("snapshot.json");
    snapshot_json(&input);
    let original_directory = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let outcome = reconcile_file(&input, std::path::Path::new("report.json"), FailPoint::None);
    std::env::set_current_dir(original_directory).unwrap();

    assert!(outcome.is_ok(), "{outcome:?}");
    assert!(dir.path().join("report.json").exists());
}

#[test]
fn abrupt_process_exits_never_replace_requested_output() {
    // Break caught: an abrupt exit at any publication boundary can truncate
    // the requested plan if it is opened before a complete temp file exists.
    for phase in ["before-output", "after-temp", "before-publish"] {
        for prior_exists in [false, true] {
            let dir = temp_dir();
            let input = dir.path().join("snapshot.json");
            let output = dir.path().join("report.json");
            snapshot_json(&input);
            if prior_exists {
                fs::write(&output, b"prior-complete-report").unwrap();
            }
            let status = std::process::Command::new(env!("CARGO_BIN_EXE_return-reconciler-trial1"))
                .args([
                    "reconcile-exit",
                    phase,
                    input.to_str().unwrap(),
                    output.to_str().unwrap(),
                ])
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(86));
            if prior_exists {
                assert_eq!(fs::read(&output).unwrap(), b"prior-complete-report");
            } else {
                assert!(!output.exists());
            }
        }
    }
}

#[test]
fn report_wide_cent_overflow_is_rejected_before_publication() {
    // Break caught: release-mode u64 summation can wrap two valid MAX-cent
    // return plans into a plausible but false report-wide summary.
    let snapshot = overflow_snapshot();
    let error = reconcile(&snapshot).unwrap_err();
    assert!(error.contains("report proposed cents overflow"));

    for prior_exists in [false, true] {
        let dir = temp_dir();
        let input = dir.path().join("snapshot.json");
        let output = dir.path().join("report.json");
        write_snapshot(&input, &snapshot);
        if prior_exists {
            fs::write(&output, b"prior-complete-report").unwrap();
        }

        let error = reconcile_file(&input, &output, FailPoint::None).unwrap_err();
        assert!(error.contains("report proposed cents overflow"));
        if prior_exists {
            assert_eq!(fs::read(&output).unwrap(), b"prior-complete-report");
        } else {
            assert!(!output.exists());
        }
    }
}

fn payment_validation_snapshot(payment_return_id: &str, payment_line_id: &str) -> Snapshot {
    let mut returns = Vec::new();
    let mut events = Vec::new();
    for index in 0..2_u64 {
        let return_id = format!("return-{index}");
        let line_id = format!("line-{index}");
        let sku = format!("sku-{index}");
        returns.push(ReturnAuthorization {
            return_id: return_id.clone(),
            captured_cents: 700,
            lines: vec![AuthorizedLine {
                line_id,
                sku: sku.clone(),
                authorized_qty: 1,
                paid_subtotal_cents: 700,
                tax_cents: 0,
                discount_cents: 0,
                prior_successful_refund_cents: 0,
            }],
        });
        events.push(Event::WarehouseScan {
            event_id: format!("scan-{index}"),
            source_system_id: "warehouse-east".into(),
            source_timestamp: 1,
            ingestion_id: index,
            return_id,
            parcel_id: format!("parcel-{index}"),
            sku,
            quantity: 1,
        });
    }
    events.push(Event::PaymentResult {
        event_id: "payment-invalid-line".into(),
        source_system_id: "payment-provider".into(),
        source_timestamp: 2,
        ingestion_id: 2,
        return_id: payment_return_id.into(),
        line_id: payment_line_id.into(),
        refund_id: "refund-1".into(),
        amount_cents: 700,
        status: PaymentStatus::Pending,
    });
    Snapshot {
        snapshot_id: "invalid-payment-fixture".into(),
        returns,
        events,
    }
}

#[test]
fn unknown_and_wrong_return_payment_lines_are_rejected_before_publication() {
    // Break caught: a pending result for a missing/misowned line can be
    // silently ignored, allowing a second refund proposal to survive.
    for snapshot in [
        payment_validation_snapshot("return-0", "line-missing"),
        payment_validation_snapshot("return-0", "line-1"),
    ] {
        for prior_exists in [false, true] {
            let dir = temp_dir();
            let input = dir.path().join("snapshot.json");
            let output = dir.path().join("report.json");
            write_snapshot(&input, &snapshot);
            if prior_exists {
                fs::write(&output, b"prior-complete-report").unwrap();
            }

            let error = reconcile_file(&input, &output, FailPoint::None).unwrap_err();
            assert!(error.contains("payment result references line"));
            if prior_exists {
                assert_eq!(fs::read(&output).unwrap(), b"prior-complete-report");
            } else {
                assert!(!output.exists());
            }
        }
    }
}
