use std::sync::{Arc, Barrier};

use mailbox_mirror_lab::{Batch, Event, Message, SqliteBaseline};

fn epoch_message(epoch: u64, uid: u64) -> Message {
    Message {
        uid,
        fingerprint: format!("{epoch:016x}{uid:016x}"),
        internal_date: 1_700_000_000,
        size: 512,
        flags: u8::from(uid.is_multiple_of(2)),
    }
}

#[test]
fn eight_readers_never_observe_a_mixed_epoch_during_publication() {
    // Break caught: composing one checkpoint from separate read snapshots around a writer commit.
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("mirror.sqlite");
    let mut writer = SqliteBaseline::create(&path).unwrap();
    writer
        .apply(&Batch::new(
            "create",
            "mailbox-a",
            1,
            vec![Event::CreateMailbox {
                display_name: "Inbox".to_owned(),
                uid_validity: 1,
            }],
        ))
        .unwrap();
    writer
        .apply(&Batch::new(
            "initial",
            "mailbox-a",
            2,
            (1..=20)
                .map(|uid| Event::Add {
                    uid_validity: 1,
                    message: epoch_message(1, uid),
                    cursor: 2,
                })
                .collect(),
        ))
        .unwrap();

    let barrier = Arc::new(Barrier::new(9));
    let selected: Vec<u64> = (1..=20).collect();
    std::thread::scope(|scope| {
        let mut readers = Vec::new();
        for _ in 0..8 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            let selected = selected.clone();
            readers.push(scope.spawn(move || {
                let reader = SqliteBaseline::open(&path).unwrap();
                barrier.wait();
                for _ in 0..80 {
                    let checkpoint = reader.checkpoint("mailbox-a", &selected).unwrap();
                    assert_eq!(checkpoint.total, 20);
                    assert_eq!(checkpoint.selected.len(), 20);
                    assert!(checkpoint.selected.iter().all(|message| {
                        message
                            .fingerprint
                            .starts_with(&format!("{:016x}", checkpoint.uid_validity))
                    }));
                }
            }));
        }
        barrier.wait();
        for epoch in 2..=20_u64 {
            let sequence = epoch * 3;
            writer
                .apply(&Batch::new(
                    format!("begin-{epoch}"),
                    "mailbox-a",
                    sequence,
                    vec![Event::SnapshotBegin {
                        uid_validity: epoch,
                    }],
                ))
                .unwrap();
            writer
                .apply(&Batch::new(
                    format!("stage-{epoch}"),
                    "mailbox-a",
                    sequence + 1,
                    (1..=20)
                        .map(|uid| Event::SnapshotMessage {
                            uid_validity: epoch,
                            message: epoch_message(epoch, uid),
                        })
                        .collect(),
                ))
                .unwrap();
            writer
                .apply(&Batch::new(
                    format!("commit-{epoch}"),
                    "mailbox-a",
                    sequence + 2,
                    vec![Event::SnapshotCommit {
                        uid_validity: epoch,
                        cursor: sequence + 2,
                    }],
                ))
                .unwrap();
        }
        for reader in readers {
            reader.join().unwrap();
        }
    });
}
