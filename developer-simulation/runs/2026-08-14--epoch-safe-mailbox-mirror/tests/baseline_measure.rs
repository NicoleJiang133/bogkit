use std::time::Instant;

use mailbox_mirror_lab::{Batch, Event, Message, SqliteBaseline};

#[test]
#[ignore = "explicit local measurement"]
fn measure_sqlite_baseline_10k_messages() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("mirror.sqlite");
    let mut baseline = SqliteBaseline::create(&path).unwrap();
    let started = Instant::now();
    for mailbox in 0..20_u64 {
        baseline
            .apply(&Batch::new(
                format!("create-{mailbox}"),
                format!("mailbox-{mailbox:02}"),
                1,
                vec![Event::CreateMailbox {
                    display_name: format!("Folder {mailbox:02}"),
                    uid_validity: 1,
                }],
            ))
            .unwrap();
    }
    for (sequence, batch_number) in (2_u64..).zip(0..100_u64) {
        let mailbox = batch_number % 20;
        let events = (0..100_u64)
            .map(|offset| {
                let uid = (batch_number / 20) * 100 + offset + 1;
                Event::Add {
                    uid_validity: 1,
                    message: Message {
                        uid,
                        fingerprint: format!("{mailbox:016x}{uid:016x}"),
                        internal_date: 1_700_000_000 + i64::try_from(uid).unwrap(),
                        size: 2048,
                        flags: u8::from(uid % 3 == 0),
                    },
                    cursor: sequence,
                }
            })
            .collect();
        baseline
            .apply(&Batch::new(
                format!("add-{batch_number}"),
                format!("mailbox-{mailbox:02}"),
                sequence,
                events,
            ))
            .unwrap();
    }
    let apply_ms = started.elapsed().as_secs_f64() * 1000.0;

    let mut query_us = Vec::with_capacity(200);
    for index in 0..200_u64 {
        let query_started = Instant::now();
        let checkpoint = baseline
            .checkpoint(&format!("mailbox-{:02}", index % 20), &[])
            .unwrap();
        assert_eq!(checkpoint.total, 500);
        query_us.push(query_started.elapsed().as_micros());
    }
    query_us.sort_unstable();
    let p50_us = query_us[query_us.len() / 2];
    let p95_us = query_us[query_us.len() * 95 / 100];

    drop(baseline);
    let reopen_started = Instant::now();
    let reopened = SqliteBaseline::open(&path).unwrap();
    assert_eq!(reopened.checkpoint("mailbox-00", &[]).unwrap().total, 500);
    let reopen_ms = reopen_started.elapsed().as_secs_f64() * 1000.0;
    let disk_bytes = [
        path.clone(),
        path.with_extension("sqlite-wal"),
        path.with_extension("sqlite-shm"),
    ]
    .iter()
    .filter_map(|entry| std::fs::metadata(entry).ok())
    .map(|metadata| metadata.len())
    .sum::<u64>();
    println!(
        "{{\"mailboxes\":20,\"messages\":10000,\"batches\":120,\"apply_ms\":{apply_ms:.3},\"responses_per_second\":{:.1},\"count_p50_us\":{p50_us},\"count_p95_us\":{p95_us},\"reopen_ms\":{reopen_ms:.3},\"disk_bytes\":{disk_bytes}}}",
        10_020.0 / (apply_ms / 1000.0)
    );
}
