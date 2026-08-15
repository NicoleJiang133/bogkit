use std::sync::{Arc, Barrier};

use financial_snapshot_trial::{
    Batch, Cell, CellValue, Edit, ErrorCode, Formula, Range, Reject, Trial, Workbook,
};

fn a1() -> Cell {
    Cell::new(0, 0, 0)
}

fn a2() -> Cell {
    Cell::new(0, 1, 0)
}

fn b1() -> Cell {
    Cell::new(0, 0, 1)
}

fn c1() -> Cell {
    Cell::new(0, 0, 2)
}

fn fresh_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "financial-snapshot-trial-{label}-{}",
        std::process::id()
    ))
}

#[test]
fn range_dependency_recalculates_and_publishes_one_generation() {
    let path = fresh_path("range");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    let record = Batch::new(1, vec![Edit::SetLiteral(a2(), 5)]).encode();

    let outcome = trial.apply(&record).unwrap();

    assert_eq!(outcome.recalculated, vec![b1(), c1(), a2()]);
    let visible = trial.reader().read();
    assert_eq!(visible.generation, 1);
    assert_eq!(visible.point(a2()), Some(&CellValue::Number(5)));
    assert_eq!(visible.point(b1()), Some(&CellValue::Number(15)));
    assert_eq!(visible.point(c1()), Some(&CellValue::Number(0)));
    assert!(visible.cells.iter().all(|cell| cell.generation == 1));

    drop(trial);
    let recovered =
        Trial::recover_from_journal(&path, Workbook::deterministic_fixture(), &[record]).unwrap();
    assert_eq!(recovered.reader().read(), visible);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn formula_replacement_rebuilds_range_dependencies() {
    let path = fresh_path("formula");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();

    trial
        .apply(
            &Batch::new(
                1,
                vec![Edit::SetFormula(b1(), Formula::Sum(Range::new(a2(), a2())))],
            )
            .encode(),
        )
        .unwrap();
    let outcome = trial
        .apply(&Batch::new(2, vec![Edit::SetLiteral(a1(), 99)]).encode())
        .unwrap();

    assert_eq!(outcome.recalculated, vec![a1(), c1()]);
    let visible = trial.reader().read();
    assert_eq!(visible.point(b1()), Some(&CellValue::Number(20)));
    assert_eq!(visible.point(c1()), Some(&CellValue::Number(119)));
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn bad_checksum_leaves_last_accepted_generation_unchanged() {
    let path = fresh_path("checksum");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    let before = trial.reader().read();
    let mut bytes = Batch::new(1, vec![Edit::SetLiteral(a1(), 44)]).encode();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x80;

    assert_eq!(trial.apply(&bytes), Err(Reject::BadChecksum));
    assert_eq!(trial.reader().read(), before);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn skipped_sequence_leaves_last_accepted_generation_unchanged() {
    let path = fresh_path("sequence");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    let before = trial.reader().read();

    assert_eq!(
        trial.apply(&Batch::new(2, vec![Edit::SetLiteral(a1(), 44)]).encode()),
        Err(Reject::UnexpectedSequence {
            expected: 1,
            actual: 2,
        })
    );
    assert_eq!(trial.reader().read(), before);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn duplicate_sequence_leaves_last_accepted_generation_unchanged() {
    let path = fresh_path("duplicate");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    trial
        .apply(&Batch::new(1, vec![Edit::SetLiteral(a1(), 44)]).encode())
        .unwrap();
    let before = trial.reader().read();

    assert_eq!(
        trial.apply(&Batch::new(1, vec![Edit::SetLiteral(a1(), 45)]).encode()),
        Err(Reject::UnexpectedSequence {
            expected: 2,
            actual: 1,
        })
    );
    assert_eq!(trial.reader().read(), before);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn arithmetic_overflow_leaves_last_accepted_generation_unchanged() {
    let path = fresh_path("overflow");
    let _ = std::fs::remove_dir_all(&path);
    let mut workbook = Workbook::deterministic_fixture();
    workbook.set_literal(a1(), i128::MAX);
    let mut trial = Trial::open(&path, workbook).unwrap();
    trial.bootstrap().unwrap();
    let before = trial.reader().read();

    assert_eq!(
        trial.apply(&Batch::new(1, vec![Edit::AdjustLiteral(a1(), 1)]).encode()),
        Err(Reject::IntegerOverflow)
    );
    assert_eq!(trial.reader().read(), before);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn truncated_record_leaves_last_accepted_generation_unchanged() {
    let path = fresh_path("truncated");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    let before = trial.reader().read();
    let mut bytes = Batch::new(1, vec![Edit::SetLiteral(a1(), 44)]).encode();
    bytes.truncate(bytes.len() - 3);

    assert_eq!(trial.apply(&bytes), Err(Reject::TruncatedRecord));
    assert_eq!(trial.reader().read(), before);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn unknown_formula_opcode_leaves_last_accepted_generation_unchanged() {
    let path = fresh_path("unknown-formula");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    let before = trial.reader().read();
    let mut bytes = Batch::new(
        1,
        vec![Edit::SetFormula(b1(), Formula::Sum(Range::new(a1(), a2())))],
    )
    .encode();
    bytes[31] = 255;
    let checksum = bytes[..bytes.len() - 8]
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    let checksum_at = bytes.len() - 8;
    bytes[checksum_at..].copy_from_slice(&checksum.to_le_bytes());

    assert_eq!(trial.apply(&bytes), Err(Reject::UnknownFormulaOpcode(255)));
    assert_eq!(trial.reader().read(), before);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn cycle_and_missing_reference_errors_are_stable_and_ordered() {
    let path = fresh_path("errors");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    let visible = trial.reader().read();

    assert_eq!(
        visible.errors(),
        vec![
            (Cell::new(1, 0, 0), ErrorCode::Cycle),
            (Cell::new(1, 0, 1), ErrorCode::Cycle),
            (Cell::new(1, 1, 0), ErrorCode::MissingReference),
        ]
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn canonical_snapshot_bytes_are_reference_backed() {
    let path = fresh_path("bytes");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    trial
        .apply(&Batch::new(1, vec![Edit::SetLiteral(a2(), 5)]).encode())
        .unwrap();

    assert_eq!(
        String::from_utf8(trial.reader().read().canonical_bytes()).unwrap(),
        "generation=1\n0:0:0=N:10\n0:0:1=N:15\n0:0:2=N:0\n0:1:0=N:5\n1:0:0=E:CYCLE\n1:0:1=E:CYCLE\n1:1:0=E:MISSING_REFERENCE\n"
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn rectangular_read_returns_coordinate_order_from_one_generation() {
    let path = fresh_path("rectangular");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    let visible = trial.reader().read();

    assert_eq!(
        visible.rectangular(Range::new(a1(), a2())),
        vec![
            (a1(), &CellValue::Number(10)),
            (a2(), &CellValue::Number(20)),
        ]
    );
    assert!(visible.cells.iter().all(|cell| cell.generation == 0));
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn twelve_readers_never_observe_a_mixed_generation() {
    let path = fresh_path("concurrency");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    let reader = trial.reader();
    let start = Arc::new(Barrier::new(13));

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..12 {
            let reader = reader.clone();
            let start = Arc::clone(&start);
            handles.push(scope.spawn(move || {
                start.wait();
                for _ in 0..2_000 {
                    let visible = reader.read();
                    assert!(
                        visible
                            .cells
                            .iter()
                            .all(|cell| cell.generation == visible.generation)
                    );
                }
            }));
        }
        start.wait();
        for sequence in 1..=100 {
            trial
                .apply(
                    &Batch::new(sequence, vec![Edit::SetLiteral(a2(), i128::from(sequence))])
                        .encode(),
                )
                .unwrap();
        }
        for handle in handles {
            handle.join().unwrap();
        }
    });

    assert_eq!(trial.reader().read().generation, 100);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn bootstrap_twice_rejects_without_rewinding_visible_state() {
    let path = fresh_path("bootstrap-twice");
    let _ = std::fs::remove_dir_all(&path);
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
    trial.bootstrap().unwrap();
    trial
        .apply(&Batch::new(1, vec![Edit::SetLiteral(a2(), 5)]).encode())
        .unwrap();
    let before = trial.reader().read();
    let before_bytes = before.canonical_bytes();

    assert_eq!(trial.bootstrap(), Err(Reject::AlreadyInitialized));
    assert_eq!(trial.reader().read(), before);
    assert_eq!(trial.reader().read().canonical_bytes(), before_bytes);
    assert_eq!(
        trial.apply(&Batch::new(1, vec![Edit::SetLiteral(a2(), 7)]).encode()),
        Err(Reject::UnexpectedSequence {
            expected: 2,
            actual: 1,
        })
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn open_on_existing_store_rejects_without_changing_persisted_state() {
    let path = fresh_path("open-existing");
    let _ = std::fs::remove_dir_all(&path);
    let record = Batch::new(1, vec![Edit::SetLiteral(a2(), 5)]).encode();
    let expected = {
        let mut trial = Trial::open(&path, Workbook::deterministic_fixture()).unwrap();
        trial.bootstrap().unwrap();
        trial.apply(&record).unwrap();
        trial.reader().read()
    };

    assert!(matches!(
        Trial::open(&path, Workbook::deterministic_fixture()),
        Err(Reject::AlreadyInitialized)
    ));
    let recovered =
        Trial::recover_from_journal(&path, Workbook::deterministic_fixture(), &[record]).unwrap();
    assert_eq!(recovered.reader().read(), expected);
    assert_eq!(
        recovered.reader().read().canonical_bytes(),
        expected.canonical_bytes()
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn close_recover_and_continue_matches_uninterrupted_formula_trace() {
    let uninterrupted_path = fresh_path("uninterrupted");
    let resumed_path = fresh_path("resumed");
    let _ = std::fs::remove_dir_all(&uninterrupted_path);
    let _ = std::fs::remove_dir_all(&resumed_path);
    let first = Batch::new(1, vec![Edit::SetLiteral(a2(), 5)]).encode();
    let formula = Batch::new(
        2,
        vec![Edit::SetFormula(b1(), Formula::Sum(Range::new(a2(), a2())))],
    )
    .encode();
    let next = Batch::new(3, vec![Edit::SetLiteral(a1(), 99)]).encode();

    let uninterrupted = {
        let mut trial =
            Trial::open(&uninterrupted_path, Workbook::deterministic_fixture()).unwrap();
        trial.bootstrap().unwrap();
        trial.apply(&first).unwrap();
        trial.apply(&formula).unwrap();
        trial.apply(&next).unwrap();
        trial.reader().read()
    };
    let resumed = {
        let mut trial = Trial::open(&resumed_path, Workbook::deterministic_fixture()).unwrap();
        trial.bootstrap().unwrap();
        trial.apply(&first).unwrap();
        trial.apply(&formula).unwrap();
        drop(trial);
        let journal = vec![first.clone(), formula.clone()];
        let mut trial =
            Trial::recover_from_journal(&resumed_path, Workbook::deterministic_fixture(), &journal)
                .unwrap();
        trial.apply(&next).unwrap();
        trial.reader().read()
    };

    assert_eq!(resumed, uninterrupted);
    assert_eq!(resumed.canonical_bytes(), uninterrupted.canonical_bytes());
    std::fs::remove_dir_all(uninterrupted_path).unwrap();
    std::fs::remove_dir_all(resumed_path).unwrap();
}
