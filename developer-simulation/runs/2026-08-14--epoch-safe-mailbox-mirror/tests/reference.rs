use mailbox_mirror_lab::{ApplyOutcome, Batch, Event, Message, ReferenceMirror};

fn message(uid: u64, seen: bool, fingerprint: &str) -> Message {
    Message {
        uid,
        fingerprint: fingerprint.to_owned(),
        internal_date: 1_700_000_000,
        size: 512,
        flags: u8::from(seen),
    }
}

#[test]
fn duplicate_complete_batch_is_idempotent() {
    // Break caught: forgetting durable batch admission would double-apply an add.
    let mut mirror = ReferenceMirror::default();
    let create = Batch::new(
        "b-1",
        "mailbox-a",
        1,
        vec![Event::CreateMailbox {
            display_name: "Inbox".to_owned(),
            uid_validity: 7,
        }],
    );
    let add = Batch::new(
        "b-2",
        "mailbox-a",
        2,
        vec![Event::Add {
            uid_validity: 7,
            message: message(42, false, "00000000000000000000000000000001"),
            cursor: 2,
        }],
    );

    assert_eq!(mirror.apply(&create).unwrap(), ApplyOutcome::Applied);
    assert_eq!(mirror.apply(&add).unwrap(), ApplyOutcome::Applied);
    let once = mirror.checkpoint("mailbox-a", &[42]).unwrap();
    assert_eq!(mirror.apply(&add).unwrap(), ApplyOutcome::Duplicate);
    assert_eq!(mirror.checkpoint("mailbox-a", &[42]).unwrap(), once);
    assert_eq!(once.total, 1);
    assert_eq!(once.unread, 1);
}

#[test]
fn replacement_epoch_stays_hidden_until_snapshot_commit() {
    // Break caught: publishing staged rows early would mix epochs and reuse old flags for UID 42.
    let mut mirror = ReferenceMirror::default();
    mirror
        .apply(&Batch::new(
            "create",
            "mailbox-a",
            1,
            vec![Event::CreateMailbox {
                display_name: "Inbox".to_owned(),
                uid_validity: 7,
            }],
        ))
        .unwrap();
    mirror
        .apply(&Batch::new(
            "old-message",
            "mailbox-a",
            2,
            vec![Event::Add {
                uid_validity: 7,
                message: message(42, true, "00000000000000000000000000000001"),
                cursor: 2,
            }],
        ))
        .unwrap();
    mirror
        .apply(&Batch::new(
            "snapshot-begin",
            "mailbox-a",
            3,
            vec![Event::SnapshotBegin { uid_validity: 8 }],
        ))
        .unwrap();

    let during = mirror.checkpoint("mailbox-a", &[42]).unwrap();
    assert_eq!(during.uid_validity, 7);
    assert_eq!(during.status, mailbox_mirror_lab::MirrorStatus::NeedsRepair);
    assert_eq!(during.selected[0].flags, 1);

    mirror
        .apply(&Batch::new(
            "snapshot-message",
            "mailbox-a",
            4,
            vec![Event::SnapshotMessage {
                uid_validity: 8,
                message: message(42, false, "00000000000000000000000000000002"),
            }],
        ))
        .unwrap();
    assert_eq!(
        mirror.checkpoint("mailbox-a", &[42]).unwrap().selected[0].flags,
        1
    );

    mirror
        .apply(&Batch::new(
            "snapshot-commit",
            "mailbox-a",
            5,
            vec![Event::SnapshotCommit {
                uid_validity: 8,
                cursor: 5,
            }],
        ))
        .unwrap();
    let after = mirror.checkpoint("mailbox-a", &[42]).unwrap();
    assert_eq!(after.uid_validity, 8);
    assert_eq!(after.status, mailbox_mirror_lab::MirrorStatus::Synced);
    assert_eq!(after.selected[0].flags, 0);
    assert_eq!(
        after.selected[0].fingerprint,
        "00000000000000000000000000000002"
    );
}

#[test]
fn flag_replacement_and_expunge_update_materialized_counts() {
    // Break caught: treating replacement flags as additive would double unread counts.
    let mut mirror = ReferenceMirror::default();
    mirror
        .apply(&Batch::new(
            "create",
            "mailbox-a",
            1,
            vec![Event::CreateMailbox {
                display_name: "Inbox".to_owned(),
                uid_validity: 7,
            }],
        ))
        .unwrap();
    mirror
        .apply(&Batch::new(
            "add",
            "mailbox-a",
            2,
            vec![
                Event::Add {
                    uid_validity: 7,
                    message: message(1, false, "00000000000000000000000000000001"),
                    cursor: 2,
                },
                Event::Add {
                    uid_validity: 7,
                    message: message(2, false, "00000000000000000000000000000002"),
                    cursor: 2,
                },
            ],
        ))
        .unwrap();
    mirror
        .apply(&Batch::new(
            "flags",
            "mailbox-a",
            3,
            vec![Event::ReplaceFlags {
                uid_validity: 7,
                uid: 1,
                flags: 1,
                cursor: 3,
            }],
        ))
        .unwrap();
    let flagged = mirror.checkpoint("mailbox-a", &[1, 2]).unwrap();
    assert_eq!((flagged.total, flagged.unread), (2, 1));
    assert_eq!(flagged.selected[0].flags, 1);

    mirror
        .apply(&Batch::new(
            "expunge",
            "mailbox-a",
            4,
            vec![Event::Expunge {
                uid_validity: 7,
                uid: 2,
                cursor: 4,
            }],
        ))
        .unwrap();
    let expunged = mirror.checkpoint("mailbox-a", &[1, 2]).unwrap();
    assert_eq!((expunged.total, expunged.unread), (1, 0));
    assert_eq!(expunged.selected.len(), 1);
}

#[test]
fn rename_preserves_stable_id_but_recreated_name_does_not_resurrect_state() {
    // Break caught: keying by display name would attach old messages to the recreated mailbox.
    let mut mirror = ReferenceMirror::default();
    mirror
        .apply(&Batch::new(
            "create-old",
            "stable-old",
            1,
            vec![Event::CreateMailbox {
                display_name: "Inbox".to_owned(),
                uid_validity: 7,
            }],
        ))
        .unwrap();
    mirror
        .apply(&Batch::new(
            "add-old",
            "stable-old",
            2,
            vec![Event::Add {
                uid_validity: 7,
                message: message(9, false, "00000000000000000000000000000009"),
                cursor: 2,
            }],
        ))
        .unwrap();
    mirror
        .apply(&Batch::new(
            "rename",
            "stable-old",
            3,
            vec![Event::Rename {
                display_name: "Archive".to_owned(),
            }],
        ))
        .unwrap();
    let renamed = mirror.checkpoint("stable-old", &[9]).unwrap();
    assert_eq!(renamed.display_name, "Archive");
    assert_eq!(renamed.total, 1);

    mirror
        .apply(&Batch::new("delete", "stable-old", 4, vec![Event::Delete]))
        .unwrap();
    assert!(mirror.checkpoint("stable-old", &[9]).is_err());

    mirror
        .apply(&Batch::new(
            "create-new",
            "stable-new",
            5,
            vec![Event::CreateMailbox {
                display_name: "Archive".to_owned(),
                uid_validity: 1,
            }],
        ))
        .unwrap();
    let recreated = mirror.checkpoint("stable-new", &[9]).unwrap();
    assert_eq!(recreated.total, 0);
    assert!(recreated.selected.is_empty());
}

#[test]
fn rejected_batch_has_no_partial_mutation() {
    // Break caught: applying responses before validating the complete batch leaks partial state.
    let mut mirror = ReferenceMirror::default();
    mirror
        .apply(&Batch::new(
            "create",
            "mailbox-a",
            1,
            vec![Event::CreateMailbox {
                display_name: "Inbox".to_owned(),
                uid_validity: 7,
            }],
        ))
        .unwrap();
    let invalid = Batch::new(
        "invalid",
        "mailbox-a",
        2,
        vec![
            Event::Add {
                uid_validity: 7,
                message: message(1, false, "00000000000000000000000000000001"),
                cursor: 2,
            },
            Event::ReplaceFlags {
                uid_validity: 7,
                uid: 999,
                flags: 1,
                cursor: 2,
            },
        ],
    );

    assert!(mirror.apply(&invalid).is_err());
    let checkpoint = mirror.checkpoint("mailbox-a", &[1]).unwrap();
    assert_eq!(checkpoint.total, 0);
    assert_eq!(checkpoint.cursor, 0);
    assert!(checkpoint.selected.is_empty());
}
