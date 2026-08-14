use std::panic::{AssertUnwindSafe, catch_unwind};

use undo_history_lab::baseline::Baseline;
use undo_history_lab::candidate::Candidate;
use undo_history_lab::{Command, Group, Object, ReferenceModel};

fn object() -> Object {
    Object {
        id: 1,
        z: 0,
        transform: [1_000, 0, 0, 1_000, 10, 20],
        visible: true,
        fill: 0x0102_03ff,
        points: vec![(30, 40)],
    }
}

fn invalid_groups() -> Vec<Group> {
    vec![
        Group::new(
            "move-x-min",
            vec![Command::Move {
                id: 1,
                dx: i64::MIN,
                dy: 0,
            }],
        ),
        Group::new(
            "move-y-min",
            vec![Command::Move {
                id: 1,
                dx: 0,
                dy: i64::MIN,
            }],
        ),
        Group::new(
            "point-x-min",
            vec![Command::MovePoint {
                id: 1,
                index: 0,
                dx: i64::MIN,
                dy: 0,
            }],
        ),
        Group::new(
            "point-y-min",
            vec![Command::MovePoint {
                id: 1,
                index: 0,
                dx: 0,
                dy: i64::MIN,
            }],
        ),
        Group::new(
            "later-invalid",
            vec![
                Command::Recolor {
                    id: 1,
                    fill: 0xaabb_ccff,
                },
                Command::Move {
                    id: 1,
                    dx: i64::MIN,
                    dy: 0,
                },
            ],
        ),
        Group::new(
            "move-x-destination",
            vec![Command::Move {
                id: 1,
                dx: i64::MAX,
                dy: 0,
            }],
        ),
        Group::new(
            "move-y-destination",
            vec![Command::Move {
                id: 1,
                dx: 0,
                dy: i64::MAX,
            }],
        ),
        Group::new(
            "point-x-destination",
            vec![Command::MovePoint {
                id: 1,
                index: 0,
                dx: i64::MAX,
                dy: 0,
            }],
        ),
        Group::new(
            "point-y-destination",
            vec![Command::MovePoint {
                id: 1,
                index: 0,
                dx: 0,
                dy: i64::MAX,
            }],
        ),
    ]
}

#[test]
fn inverse_and_destination_overflow_is_rejected_before_model_mutation() {
    let mut model = ReferenceModel::default();
    model
        .commit(Group::new(
            "seed",
            vec![Command::Create { object: object() }],
        ))
        .unwrap();
    let before = model.canonical_json().unwrap();

    for group in invalid_groups() {
        let result = catch_unwind(AssertUnwindSafe(|| model.commit(group)));
        let error = result
            .expect("overflow must return an error, not panic")
            .unwrap_err();
        assert_eq!(error.code(), "coordinate_overflow");
        assert_eq!(model.canonical_json().unwrap(), before);
    }
}

#[test]
fn candidate_reopens_unchanged_after_each_overflow_then_accepts_a_valid_write() {
    let directory = tempfile::tempdir().unwrap();
    {
        let mut candidate = Candidate::open(directory.path()).unwrap();
        candidate
            .commit(Group::new(
                "seed",
                vec![Command::Create { object: object() }],
            ))
            .unwrap();
    }
    let before = Candidate::open(directory.path())
        .unwrap()
        .canonical_json()
        .unwrap();

    for group in invalid_groups() {
        let mut candidate = Candidate::open(directory.path()).unwrap();
        let result = catch_unwind(AssertUnwindSafe(|| candidate.commit(group)));
        let error = result
            .expect("overflow must return an error, not panic")
            .unwrap_err();
        assert_eq!(error.code(), "coordinate_overflow");
        drop(candidate);
        assert_eq!(
            Candidate::open(directory.path())
                .unwrap()
                .canonical_json()
                .unwrap(),
            before
        );
    }

    {
        let mut candidate = Candidate::open(directory.path()).unwrap();
        candidate
            .commit(Group::new(
                "valid",
                vec![Command::Move {
                    id: 1,
                    dx: 5,
                    dy: -7,
                }],
            ))
            .unwrap();
    }
    assert_eq!(
        Candidate::open(directory.path())
            .unwrap()
            .status()
            .unwrap()
            .undo_count,
        2
    );
}

#[test]
fn baseline_reopens_unchanged_after_each_overflow_then_accepts_a_valid_write() {
    let directory = tempfile::tempdir().unwrap();
    {
        let mut baseline = Baseline::open(directory.path(), 2_000).unwrap();
        baseline
            .commit(Group::new(
                "seed",
                vec![Command::Create { object: object() }],
            ))
            .unwrap();
    }
    let before = Baseline::open(directory.path(), 2_000)
        .unwrap()
        .canonical_json()
        .unwrap();

    for group in invalid_groups() {
        let mut baseline = Baseline::open(directory.path(), 2_000).unwrap();
        let result = catch_unwind(AssertUnwindSafe(|| baseline.commit(group)));
        let error = result
            .expect("overflow must return an error, not panic")
            .unwrap_err();
        assert_eq!(error.code(), "coordinate_overflow");
        drop(baseline);
        assert_eq!(
            Baseline::open(directory.path(), 2_000)
                .unwrap()
                .canonical_json()
                .unwrap(),
            before
        );
    }

    {
        let mut baseline = Baseline::open(directory.path(), 2_000).unwrap();
        baseline
            .commit(Group::new(
                "valid",
                vec![Command::Move {
                    id: 1,
                    dx: 5,
                    dy: -7,
                }],
            ))
            .unwrap();
    }
    assert_eq!(
        Baseline::open(directory.path(), 2_000)
            .unwrap()
            .status()
            .unwrap()
            .undo_count,
        2
    );
}
