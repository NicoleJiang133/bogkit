use mailbox_mirror_lab::{Batch, Event, Message, ReferenceMirror, SqliteBaseline};

fn message(uid: u64, flags: u8, fingerprint: &str) -> Message {
    Message {
        uid,
        fingerprint: fingerprint.to_owned(),
        internal_date: 1_700_000_000,
        size: 2048,
        flags,
    }
}

#[test]
fn sqlite_baseline_matches_reference_across_epoch_publication_and_replay() {
    // Break caught: a schema or SQL transaction that exposes staging or admits a duplicate twice.
    let temporary = tempfile::tempdir().unwrap();
    let mut sqlite = SqliteBaseline::create(&temporary.path().join("mirror.sqlite")).unwrap();
    let mut reference = ReferenceMirror::default();
    let batches = vec![
        Batch::new(
            "create",
            "mailbox-a",
            1,
            vec![Event::CreateMailbox {
                display_name: "Inbox".to_owned(),
                uid_validity: 7,
            }],
        ),
        Batch::new(
            "old",
            "mailbox-a",
            2,
            vec![Event::Add {
                uid_validity: 7,
                message: message(5, 1, "00000000000000000000000000000005"),
                cursor: 2,
            }],
        ),
        Batch::new(
            "begin",
            "mailbox-a",
            3,
            vec![Event::SnapshotBegin { uid_validity: 8 }],
        ),
        Batch::new(
            "stage",
            "mailbox-a",
            4,
            vec![Event::SnapshotMessage {
                uid_validity: 8,
                message: message(5, 0, "ffffffffffffffffffffffffffffffff"),
            }],
        ),
    ];
    for batch in &batches {
        reference.apply(batch).unwrap();
        sqlite.apply(batch).unwrap();
        assert_eq!(
            sqlite.checkpoint("mailbox-a", &[5]).unwrap(),
            reference.checkpoint("mailbox-a", &[5]).unwrap()
        );
    }
    let commit = Batch::new(
        "commit",
        "mailbox-a",
        5,
        vec![Event::SnapshotCommit {
            uid_validity: 8,
            cursor: 5,
        }],
    );
    reference.apply(&commit).unwrap();
    sqlite.apply(&commit).unwrap();
    sqlite.apply(&commit).unwrap();
    assert_eq!(
        sqlite.checkpoint("mailbox-a", &[5]).unwrap(),
        reference.checkpoint("mailbox-a", &[5]).unwrap()
    );
}

#[test]
fn reopening_retains_old_epoch_and_durable_staging_until_commit() {
    // Break caught: process-local staging would lose repair progress or expose the new UID early.
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("mirror.sqlite");
    let mut sqlite = SqliteBaseline::create(&path).unwrap();
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
            "old",
            "mailbox-a",
            2,
            vec![Event::Add {
                uid_validity: 1,
                message: message(10, 1, "11111111111111111111111111111111"),
                cursor: 2,
            }],
        ),
        Batch::new(
            "begin",
            "mailbox-a",
            3,
            vec![Event::SnapshotBegin { uid_validity: 2 }],
        ),
        Batch::new(
            "stage",
            "mailbox-a",
            4,
            vec![Event::SnapshotMessage {
                uid_validity: 2,
                message: message(10, 0, "22222222222222222222222222222222"),
            }],
        ),
    ] {
        sqlite.apply(&batch).unwrap();
    }
    drop(sqlite);

    let mut reopened = SqliteBaseline::open(&path).unwrap();
    let before = reopened.checkpoint("mailbox-a", &[10]).unwrap();
    assert_eq!(before.uid_validity, 1);
    assert_eq!(
        before.selected[0].fingerprint,
        "11111111111111111111111111111111"
    );
    reopened
        .apply(&Batch::new(
            "commit",
            "mailbox-a",
            5,
            vec![Event::SnapshotCommit {
                uid_validity: 2,
                cursor: 5,
            }],
        ))
        .unwrap();
    let after = reopened.checkpoint("mailbox-a", &[10]).unwrap();
    assert_eq!(after.uid_validity, 2);
    assert_eq!(
        after.selected[0].fingerprint,
        "22222222222222222222222222222222"
    );
}

fn staging_rows(path: &std::path::Path, mailbox_id: &str) -> u64 {
    let query = format!(
        "SELECT count(*) FROM staging_messages WHERE mailbox_id='{}';",
        mailbox_id.replace('\'', "''")
    );
    let output = std::process::Command::new("/usr/bin/sqlite3")
        .arg(path)
        .arg(query)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn create_mailbox(batch_id: &str, epoch: u64) -> Batch {
    Batch::new(
        batch_id,
        "mailbox-a",
        1,
        vec![Event::CreateMailbox {
            display_name: "Inbox".to_owned(),
            uid_validity: epoch,
        }],
    )
}

#[test]
fn delete_cleans_staged_messages_and_stays_clean_after_reopen() {
    // Break caught: staging_messages lacking a cascade and surviving stable-mailbox deletion.
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("mirror.sqlite");
    let mut sqlite = SqliteBaseline::create(&path).unwrap();
    for batch in [
        create_mailbox("create", 1),
        Batch::new(
            "begin",
            "mailbox-a",
            2,
            vec![Event::SnapshotBegin { uid_validity: 2 }],
        ),
        Batch::new(
            "stage",
            "mailbox-a",
            3,
            vec![Event::SnapshotMessage {
                uid_validity: 2,
                message: message(1, 0, "11111111111111111111111111111111"),
            }],
        ),
    ] {
        sqlite.apply(&batch).unwrap();
    }
    assert_eq!(staging_rows(&path, "mailbox-a"), 1);
    sqlite
        .apply(&Batch::new("delete", "mailbox-a", 4, vec![Event::Delete]))
        .unwrap();
    assert_eq!(staging_rows(&path, "mailbox-a"), 0);
    drop(sqlite);
    let _reopened = SqliteBaseline::open(&path).unwrap();
    assert_eq!(staging_rows(&path, "mailbox-a"), 0);
}

#[test]
fn repeated_same_id_delete_recreate_cleans_incomplete_and_failed_rescans() {
    // Break caught: an abandoned staged row surviving multiple lifecycles under one stable ID.
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("mirror.sqlite");
    let mut sqlite = SqliteBaseline::create(&path).unwrap();
    for round in 0..2_u64 {
        let epoch = round * 2 + 1;
        sqlite
            .apply(&create_mailbox(&format!("create-{round}"), epoch))
            .unwrap();
        sqlite
            .apply(&Batch::new(
                format!("begin-{round}"),
                "mailbox-a",
                2,
                vec![Event::SnapshotBegin {
                    uid_validity: epoch + 1,
                }],
            ))
            .unwrap();
        sqlite
            .apply(&Batch::new(
                format!("stage-{round}"),
                "mailbox-a",
                3,
                vec![Event::SnapshotMessage {
                    uid_validity: epoch + 1,
                    message: message(round + 1, 0, &format!("{:032x}", round + 1)),
                }],
            ))
            .unwrap();
        let failed = Batch::new(
            format!("failed-stage-{round}"),
            "mailbox-a",
            4,
            vec![
                Event::SnapshotMessage {
                    uid_validity: epoch + 1,
                    message: message(98, 0, "98989898989898989898989898989898"),
                },
                Event::SnapshotMessage {
                    uid_validity: epoch + 2,
                    message: message(99, 0, "99999999999999999999999999999999"),
                },
            ],
        );
        assert!(sqlite.apply(&failed).is_err());
        assert_eq!(staging_rows(&path, "mailbox-a"), 1);
        sqlite
            .apply(&Batch::new(
                format!("delete-{round}"),
                "mailbox-a",
                5,
                vec![Event::Delete],
            ))
            .unwrap();
        assert_eq!(staging_rows(&path, "mailbox-a"), 0);
        drop(sqlite);
        sqlite = SqliteBaseline::open(&path).unwrap();
        assert_eq!(staging_rows(&path, "mailbox-a"), 0);
    }
}
