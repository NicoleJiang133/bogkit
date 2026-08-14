use mailbox_mirror_lab::{
    ApplyOutcome, Batch, Event, Message, ReferenceMirror, SQLITE_INTEGER_MAX, SqliteBaseline,
};

fn message(uid: u64, flags: u8) -> Message {
    Message {
        uid,
        fingerprint: format!("{uid:032x}"),
        internal_date: 1_700_000_000,
        size: 1024,
        flags,
    }
}

fn initialized() -> (tempfile::TempDir, ReferenceMirror, SqliteBaseline) {
    let temporary = tempfile::tempdir().unwrap();
    let mut reference = ReferenceMirror::default();
    let mut sqlite = SqliteBaseline::create(&temporary.path().join("mirror.sqlite")).unwrap();
    for batch in [
        Batch::new(
            "create",
            "mailbox-a",
            1,
            vec![Event::CreateMailbox {
                display_name: "Inbox".to_owned(),
                uid_validity: 1,
            }],
        ),
        Batch::new(
            "add",
            "mailbox-a",
            2,
            vec![Event::Add {
                uid_validity: 1,
                message: message(1, 0),
                cursor: 10,
            }],
        ),
    ] {
        reference.apply(&batch).unwrap();
        sqlite.apply(&batch).unwrap();
    }
    (temporary, reference, sqlite)
}

#[test]
fn decreasing_cursor_unknown_mailbox_and_illegal_reordering_do_not_mutate() {
    // Break caught: validation errors leaking earlier events from the same transaction.
    let (_temporary, mut reference, mut sqlite) = initialized();
    let before_reference = reference.manifest();
    let before_sqlite = sqlite.manifest().unwrap();
    let invalid_batches = [
        Batch::new(
            "decreasing",
            "mailbox-a",
            3,
            vec![Event::ReplaceFlags {
                uid_validity: 1,
                uid: 1,
                flags: 1,
                cursor: 9,
            }],
        ),
        Batch::new(
            "unknown",
            "mailbox-missing",
            1,
            vec![Event::Add {
                uid_validity: 1,
                message: message(1, 0),
                cursor: 1,
            }],
        ),
        Batch::new(
            "illegal-order",
            "mailbox-a",
            4,
            vec![
                Event::ReplaceFlags {
                    uid_validity: 1,
                    uid: 99,
                    flags: 1,
                    cursor: 11,
                },
                Event::Add {
                    uid_validity: 1,
                    message: message(99, 0),
                    cursor: 11,
                },
            ],
        ),
    ];
    for batch in invalid_batches {
        assert!(reference.apply(&batch).is_err());
        assert!(sqlite.apply(&batch).is_err());
        assert_eq!(reference.manifest(), before_reference);
        assert_eq!(sqlite.manifest().unwrap(), before_sqlite);
    }
}

#[test]
fn incremental_event_for_uncommitted_epoch_is_rejected_without_hiding_old_epoch() {
    // Break caught: treating staged epoch as active before snapshot publication.
    let (_temporary, mut reference, mut sqlite) = initialized();
    let begin = Batch::new(
        "begin",
        "mailbox-a",
        3,
        vec![Event::SnapshotBegin { uid_validity: 2 }],
    );
    reference.apply(&begin).unwrap();
    sqlite.apply(&begin).unwrap();
    let invalid = Batch::new(
        "premature",
        "mailbox-a",
        4,
        vec![Event::Add {
            uid_validity: 2,
            message: message(2, 0),
            cursor: 11,
        }],
    );
    assert!(reference.apply(&invalid).is_err());
    assert!(sqlite.apply(&invalid).is_err());
    let expected = reference.checkpoint("mailbox-a", &[1, 2]).unwrap();
    let actual = sqlite.checkpoint("mailbox-a", &[1, 2]).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.uid_validity, 1);
    assert_eq!(actual.total, 1);
}

#[test]
fn sqlite_integer_max_round_trips_for_every_unsigned_field() {
    // Break caught: narrowing or floating-point coercion at the declared inclusive boundary.
    let temporary = tempfile::tempdir().unwrap();
    let database = temporary.path().join("mirror.sqlite");
    let mut reference = ReferenceMirror::default();
    let mut sqlite = SqliteBaseline::create(&database).unwrap();
    let create = Batch::new(
        "create-max",
        "mailbox-max",
        SQLITE_INTEGER_MAX,
        vec![Event::CreateMailbox {
            display_name: "Maximum".to_owned(),
            uid_validity: SQLITE_INTEGER_MAX,
        }],
    );
    let add = Batch::new(
        "add-max",
        "mailbox-max",
        SQLITE_INTEGER_MAX,
        vec![Event::Add {
            uid_validity: SQLITE_INTEGER_MAX,
            message: Message {
                uid: SQLITE_INTEGER_MAX,
                fingerprint: "ffffffffffffffffffffffffffffffff".to_owned(),
                internal_date: i64::MAX,
                size: SQLITE_INTEGER_MAX,
                flags: 0,
            },
            cursor: SQLITE_INTEGER_MAX,
        }],
    );

    for batch in [&create, &add] {
        assert_eq!(reference.apply(batch).unwrap(), ApplyOutcome::Applied);
        assert_eq!(sqlite.apply(batch).unwrap(), ApplyOutcome::Applied);
    }
    let expected = reference.manifest();
    let actual = sqlite.manifest().unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.mailboxes[0].messages[0].uid, SQLITE_INTEGER_MAX);
    assert_eq!(actual.mailboxes[0].messages[0].size, SQLITE_INTEGER_MAX);
    let stored_sequences = std::process::Command::new("/usr/bin/sqlite3")
        .arg(&database)
        .arg("SELECT sequence FROM accepted_batches ORDER BY batch_id;")
        .output()
        .unwrap();
    assert!(stored_sequences.status.success());
    assert_eq!(
        String::from_utf8(stored_sequences.stdout).unwrap(),
        format!("{SQLITE_INTEGER_MAX}\n{SQLITE_INTEGER_MAX}\n")
    );
    for batch in [&create, &add] {
        assert_eq!(reference.apply(batch).unwrap(), ApplyOutcome::Duplicate);
        assert_eq!(sqlite.apply(batch).unwrap(), ApplyOutcome::Duplicate);
    }
}

#[test]
fn every_unsigned_field_rejects_sqlite_integer_max_plus_one_before_mutation() {
    // Break caught: any u64 above SQLite's signed INTEGER range reaching SQL numeric coercion.
    let over = SQLITE_INTEGER_MAX + 1;
    let invalid_batches = [
        Batch::new(
            "bad-sequence",
            "mailbox-a",
            over,
            vec![Event::Rename {
                display_name: "Should not change".to_owned(),
            }],
        ),
        Batch::new(
            "bad-epoch",
            "mailbox-a",
            3,
            vec![Event::SnapshotBegin { uid_validity: over }],
        ),
        Batch::new(
            "bad-uid",
            "mailbox-a",
            3,
            vec![Event::Add {
                uid_validity: 1,
                message: Message {
                    uid: over,
                    fingerprint: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
                    internal_date: 1_700_000_000,
                    size: 1,
                    flags: 0,
                },
                cursor: 11,
            }],
        ),
        Batch::new(
            "bad-size",
            "mailbox-a",
            3,
            vec![Event::Add {
                uid_validity: 1,
                message: Message {
                    uid: 2,
                    fingerprint: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
                    internal_date: 1_700_000_000,
                    size: over,
                    flags: 0,
                },
                cursor: 11,
            }],
        ),
        Batch::new(
            "bad-cursor",
            "mailbox-a",
            3,
            vec![Event::ReplaceFlags {
                uid_validity: 1,
                uid: 1,
                flags: 1,
                cursor: over,
            }],
        ),
    ];

    for invalid in invalid_batches {
        let (_temporary, mut reference, mut sqlite) = initialized();
        let before_reference = reference.manifest();
        let before_sqlite = sqlite.manifest().unwrap();
        assert_eq!(
            reference.apply(&invalid).unwrap_err().to_string(),
            "integer outside SQLite INTEGER range"
        );
        assert_eq!(
            sqlite.apply(&invalid).unwrap_err().to_string(),
            "integer outside SQLite INTEGER range"
        );
        assert_eq!(reference.manifest(), before_reference);
        assert_eq!(sqlite.manifest().unwrap(), before_sqlite);

        let valid_retry = Batch::new(
            invalid.batch_id,
            "mailbox-a",
            3,
            vec![Event::Rename {
                display_name: "Valid retry".to_owned(),
            }],
        );
        assert_eq!(
            reference.apply(&valid_retry).unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(sqlite.apply(&valid_retry).unwrap(), ApplyOutcome::Applied);
    }
}
