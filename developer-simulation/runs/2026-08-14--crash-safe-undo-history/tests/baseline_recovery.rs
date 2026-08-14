use std::fs;

use undo_history_lab::baseline::Baseline;
use undo_history_lab::{Command, Group, Object};

fn object(id: u128) -> Object {
    Object {
        id,
        z: 0,
        transform: [1_000, 0, 0, 1_000, 0, 0],
        visible: true,
        fill: 0x0102_03ff,
        points: vec![(0, 0)],
    }
}

#[test]
fn every_truncation_of_the_final_small_record_keeps_earlier_history() {
    let source = tempfile::tempdir().unwrap();
    let second_start = {
        let mut baseline = Baseline::open(source.path(), 2_000).unwrap();
        baseline
            .commit(Group::new(
                "one",
                vec![Command::Create { object: object(1) }],
            ))
            .unwrap();
        let start = usize::try_from(fs::metadata(baseline.journal_path()).unwrap().len()).unwrap();
        baseline
            .commit(Group::new(
                "two",
                vec![Command::Create { object: object(2) }],
            ))
            .unwrap();
        start
    };
    let complete = fs::read(source.path().join("history.jsonl")).unwrap();

    for cut in (second_start + 1)..complete.len() {
        let trial = tempfile::tempdir().unwrap();
        fs::write(trial.path().join("history.jsonl"), &complete[..cut]).unwrap();
        let recovered = Baseline::open(trial.path(), 2_000).unwrap();
        assert_eq!(recovered.status().unwrap().undo_count, 1, "cut={cut}");
        assert_eq!(
            recovered.diagnostic().discarded_tail_offset,
            Some(second_start as u64),
            "cut={cut}"
        );
    }
}

#[test]
fn checksum_invalid_final_record_is_discarded_at_its_offset() {
    let source = tempfile::tempdir().unwrap();
    let second_start = {
        let mut baseline = Baseline::open(source.path(), 2_000).unwrap();
        baseline
            .commit(Group::new(
                "one",
                vec![Command::Create { object: object(1) }],
            ))
            .unwrap();
        let start = usize::try_from(fs::metadata(baseline.journal_path()).unwrap().len()).unwrap();
        baseline
            .commit(Group::new(
                "two",
                vec![Command::Create { object: object(2) }],
            ))
            .unwrap();
        start
    };
    let path = source.path().join("history.jsonl");
    let mut corrupt = fs::read(&path).unwrap();
    let payload_byte = corrupt[second_start..]
        .iter()
        .position(|byte| *byte == b't')
        .map(|position| second_start + position)
        .unwrap();
    corrupt[payload_byte] = b'x';
    fs::write(&path, corrupt).unwrap();

    let recovered = Baseline::open(source.path(), 2_000).unwrap();
    assert_eq!(recovered.status().unwrap().undo_count, 1);
    assert_eq!(
        recovered.diagnostic().discarded_tail_offset,
        Some(second_start as u64)
    );
}

#[test]
fn snapshot_interval_counts_committed_groups_not_undo_actions() {
    let directory = tempfile::tempdir().unwrap();
    let mut baseline = Baseline::open(directory.path(), 2).unwrap();
    baseline
        .commit(Group::new(
            "one",
            vec![Command::Create { object: object(1) }],
        ))
        .unwrap();
    baseline.undo(1).unwrap();
    assert!(!directory.path().join("snapshot.json").exists());

    baseline
        .commit(Group::new(
            "two",
            vec![Command::Create { object: object(2) }],
        ))
        .unwrap();
    assert!(directory.path().join("snapshot.json").exists());
}

#[test]
fn partial_final_tail_is_truncated_before_later_acknowledged_append() {
    let directory = tempfile::tempdir().unwrap();
    let second_start = {
        let mut baseline = Baseline::open(directory.path(), 2_000).unwrap();
        baseline
            .commit(Group::new(
                "one",
                vec![Command::Create { object: object(1) }],
            ))
            .unwrap();
        let offset = fs::metadata(baseline.journal_path()).unwrap().len();
        baseline
            .commit(Group::new(
                "torn",
                vec![Command::Create { object: object(2) }],
            ))
            .unwrap();
        offset
    };
    let journal_path = directory.path().join("history.jsonl");
    let mut bytes = fs::read(&journal_path).unwrap();
    bytes.pop();
    fs::write(&journal_path, bytes).unwrap();

    {
        let mut recovered = Baseline::open(directory.path(), 2_000).unwrap();
        assert_eq!(fs::metadata(&journal_path).unwrap().len(), second_start);
        recovered
            .commit(Group::new(
                "later",
                vec![Command::Create { object: object(3) }],
            ))
            .unwrap();
    }

    let reopened = Baseline::open(directory.path(), 2_000).unwrap();
    assert_eq!(reopened.status().unwrap().undo_count, 2);
    assert!(reopened.model().document().objects().contains_key(&1));
    assert!(reopened.model().document().objects().contains_key(&3));
}

#[test]
fn checksum_invalid_final_tail_is_truncated_before_later_acknowledged_append() {
    let directory = tempfile::tempdir().unwrap();
    let second_start = {
        let mut baseline = Baseline::open(directory.path(), 2_000).unwrap();
        baseline
            .commit(Group::new(
                "one",
                vec![Command::Create { object: object(1) }],
            ))
            .unwrap();
        let offset = fs::metadata(baseline.journal_path()).unwrap().len();
        baseline
            .commit(Group::new(
                "bad-checksum",
                vec![Command::Create { object: object(2) }],
            ))
            .unwrap();
        offset
    };
    let journal_path = directory.path().join("history.jsonl");
    let mut bytes = fs::read(&journal_path).unwrap();
    let payload_byte = bytes[usize::try_from(second_start).unwrap()..]
        .iter()
        .position(|byte| *byte == b'b')
        .map(|position| usize::try_from(second_start).unwrap() + position)
        .unwrap();
    bytes[payload_byte] = b'x';
    fs::write(&journal_path, bytes).unwrap();

    {
        let mut recovered = Baseline::open(directory.path(), 2_000).unwrap();
        assert_eq!(fs::metadata(&journal_path).unwrap().len(), second_start);
        recovered
            .commit(Group::new(
                "later",
                vec![Command::Create { object: object(3) }],
            ))
            .unwrap();
    }

    let reopened = Baseline::open(directory.path(), 2_000).unwrap();
    assert_eq!(reopened.status().unwrap().undo_count, 2);
    assert!(reopened.model().document().objects().contains_key(&1));
    assert!(reopened.model().document().objects().contains_key(&3));
}

#[test]
fn exact_retry_does_not_append_or_advance_unique_commit_snapshot_interval() {
    let directory = tempfile::tempdir().unwrap();
    let first = Group::new("one", vec![Command::Create { object: object(1) }]);
    let first_outcome = {
        let mut baseline = Baseline::open(directory.path(), 2).unwrap();
        let outcome = baseline.commit(first.clone()).unwrap();
        let journal_len = fs::metadata(baseline.journal_path()).unwrap().len();
        assert_eq!(baseline.commit(first.clone()).unwrap(), outcome);
        assert_eq!(
            fs::metadata(baseline.journal_path()).unwrap().len(),
            journal_len
        );
        assert!(!directory.path().join("snapshot.json").exists());
        outcome
    };

    let mut reopened = Baseline::open(directory.path(), 2).unwrap();
    let journal_len = fs::metadata(reopened.journal_path()).unwrap().len();
    assert_eq!(reopened.commit(first).unwrap(), first_outcome);
    assert_eq!(
        fs::metadata(reopened.journal_path()).unwrap().len(),
        journal_len
    );
    assert!(!directory.path().join("snapshot.json").exists());
    reopened
        .commit(Group::new(
            "two",
            vec![Command::Create { object: object(2) }],
        ))
        .unwrap();
    assert!(directory.path().join("snapshot.json").exists());
    drop(reopened);

    assert_eq!(
        Baseline::open(directory.path(), 2)
            .unwrap()
            .status()
            .unwrap()
            .undo_count,
        2
    );
}

#[test]
fn exact_dedup_content_survives_baseline_restart_and_compaction() {
    let directory = tempfile::tempdir().unwrap();
    let group = Group::new(
        "stable",
        vec![Command::Create {
            object: object(400),
        }],
    );
    let outcome = {
        let mut baseline = Baseline::open(directory.path(), 2_000).unwrap();
        let outcome = baseline.commit(group.clone()).unwrap();
        baseline.compact(1).unwrap();
        outcome
    };

    let mut reopened = Baseline::open(directory.path(), 2_000).unwrap();
    let compacted_journal_len = fs::metadata(reopened.journal_path()).unwrap().len();
    assert_eq!(reopened.commit(group).unwrap(), outcome);
    assert_eq!(
        fs::metadata(reopened.journal_path()).unwrap().len(),
        compacted_journal_len
    );
    let before = reopened.canonical_json().unwrap();
    let error = reopened
        .commit(Group::new(
            "stable",
            vec![Command::Recolor {
                id: 400,
                fill: 0x1122_33ff,
            }],
        ))
        .unwrap_err();
    assert_eq!(error.code(), "group_id_conflict");
    assert_eq!(reopened.canonical_json().unwrap(), before);
}

#[test]
fn middle_corruption_remains_fatal_and_is_not_truncated() {
    let directory = tempfile::tempdir().unwrap();
    let first_end = {
        let mut baseline = Baseline::open(directory.path(), 2_000).unwrap();
        baseline
            .commit(Group::new(
                "one",
                vec![Command::Create { object: object(1) }],
            ))
            .unwrap();
        let offset = fs::metadata(baseline.journal_path()).unwrap().len();
        baseline
            .commit(Group::new(
                "two",
                vec![Command::Create { object: object(2) }],
            ))
            .unwrap();
        baseline
            .commit(Group::new(
                "three",
                vec![Command::Create { object: object(3) }],
            ))
            .unwrap();
        offset
    };
    let journal_path = directory.path().join("history.jsonl");
    let mut bytes = fs::read(&journal_path).unwrap();
    let middle_byte = usize::try_from(first_end).unwrap() + 10;
    bytes[middle_byte] ^= 1;
    fs::write(&journal_path, &bytes).unwrap();
    let corrupt_len = fs::metadata(&journal_path).unwrap().len();

    let Err(error) = Baseline::open(directory.path(), 2_000) else {
        panic!("middle corruption must remain fatal");
    };
    assert_eq!(error.code(), "middle_journal_corruption");
    assert_eq!(fs::metadata(&journal_path).unwrap().len(), corrupt_len);
}
