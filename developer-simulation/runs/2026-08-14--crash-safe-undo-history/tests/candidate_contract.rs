use undo_history_lab::candidate::{Candidate, compact_directory};
use undo_history_lab::{Command, Group, Object};

fn directory_bytes(path: &std::path::Path) -> u64 {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directory_bytes(&path)
            } else {
                std::fs::metadata(path).unwrap().len()
            }
        })
        .sum()
}

fn object(id: u128) -> Object {
    Object {
        id,
        z: 7,
        transform: [1_000, 0, 0, 1_000, 0, 0],
        visible: true,
        fill: 0xaabb_ccff,
        points: vec![],
    }
}

#[test]
fn acknowledged_group_and_duplicate_outcome_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let group = Group::new("stable-7", vec![Command::Create { object: object(7) }]);
    let first = {
        let mut store = Candidate::open(dir.path()).unwrap();
        store.commit(group.clone()).unwrap()
    };

    let mut reopened = Candidate::open(dir.path()).unwrap();
    let duplicate = reopened.commit(group).unwrap();

    assert_eq!(duplicate, first);
    assert_eq!(reopened.status().unwrap().undo_count, 1);
    assert_eq!(reopened.model().document().objects().len(), 1);
}

#[test]
fn reused_group_id_with_different_content_does_not_change_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Candidate::open(dir.path()).unwrap();
    store
        .commit(Group::new(
            "stable-8",
            vec![Command::Create { object: object(8) }],
        ))
        .unwrap();
    let before = store.canonical_json().unwrap();

    let error = store
        .commit(Group::new(
            "stable-8",
            vec![Command::Create { object: object(9) }],
        ))
        .unwrap_err();

    assert_eq!(error.code(), "group_id_conflict");
    assert_eq!(store.canonical_json().unwrap(), before);
}

#[test]
fn compaction_retains_recent_undo_window_and_all_duplicate_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let oldest = Group::new(
        "oldest",
        vec![Command::Create {
            object: object(100),
        }],
    );
    let original = {
        let mut store = Candidate::open(dir.path()).unwrap();
        let original = store.commit(oldest.clone()).unwrap();
        for id in 101..105 {
            store
                .commit(Group::new(
                    format!("g-{id}"),
                    vec![Command::Create { object: object(id) }],
                ))
                .unwrap();
        }
        original
    };

    compact_directory(dir.path(), 3).unwrap();
    let mut reopened = Candidate::open(dir.path()).unwrap();

    assert_eq!(reopened.status().unwrap().undo_count, 3);
    assert_eq!(reopened.commit(oldest).unwrap(), original);
    assert_eq!(reopened.status().unwrap().undo_count, 3);
}

#[test]
fn exact_retry_does_not_rewrite_candidate_before_or_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let group = Group::new(
        "stable",
        vec![Command::Create {
            object: object(200),
        }],
    );
    let outcome = {
        let mut store = Candidate::open(dir.path()).unwrap();
        let outcome = store.commit(group.clone()).unwrap();
        let bytes = directory_bytes(dir.path());
        assert_eq!(store.commit(group.clone()).unwrap(), outcome);
        assert_eq!(directory_bytes(dir.path()), bytes);
        outcome
    };

    let mut reopened = Candidate::open(dir.path()).unwrap();
    let bytes = directory_bytes(dir.path());
    assert_eq!(reopened.commit(group).unwrap(), outcome);
    assert_eq!(directory_bytes(dir.path()), bytes);
}

#[test]
fn exact_dedup_content_survives_candidate_restart_and_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let group = Group::new(
        "stable",
        vec![Command::Create {
            object: object(300),
        }],
    );
    let outcome = {
        let mut store = Candidate::open(dir.path()).unwrap();
        store.commit(group.clone()).unwrap()
    };
    compact_directory(dir.path(), 1).unwrap();

    let mut reopened = Candidate::open(dir.path()).unwrap();
    let compacted_bytes = directory_bytes(dir.path());
    assert_eq!(reopened.commit(group).unwrap(), outcome);
    assert_eq!(directory_bytes(dir.path()), compacted_bytes);
    let before = reopened.canonical_json().unwrap();
    let error = reopened
        .commit(Group::new(
            "stable",
            vec![Command::Recolor {
                id: 300,
                fill: 0x1122_33ff,
            }],
        ))
        .unwrap_err();
    assert_eq!(error.code(), "group_id_conflict");
    assert_eq!(reopened.canonical_json().unwrap(), before);
}
