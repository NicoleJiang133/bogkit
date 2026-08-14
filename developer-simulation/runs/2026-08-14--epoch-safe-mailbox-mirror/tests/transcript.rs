use std::io::{BufReader, Cursor};

use mailbox_mirror_lab::{
    GeneratorConfig, SqliteBaseline, apply_transcript, generate_transcript, verify_transcript,
};

#[test]
fn generated_transcript_is_deterministic_and_matches_both_models() {
    // Break caught: generator map order, replay, or epoch handling causing model divergence.
    let config = GeneratorConfig {
        seed: 17,
        mailboxes: 30,
        live_messages: 300,
        responses: 1_200,
    };
    let mut first = Vec::new();
    let first_summary = generate_transcript(&config, &mut first).unwrap();
    let mut second = Vec::new();
    let second_summary = generate_transcript(&config, &mut second).unwrap();
    let mut third = Vec::new();
    let third_summary = generate_transcript(&config, &mut third).unwrap();
    assert_eq!(first, second);
    assert_eq!(second, third);
    assert_eq!(first_summary, second_summary);
    assert_eq!(second_summary, third_summary);
    assert_eq!(first_summary.responses, 1_200);
    assert_eq!(first_summary.duplicate_responses, 60);

    let temporary = tempfile::tempdir().unwrap();
    let database = temporary.path().join("mirror.sqlite");
    let verification = verify_transcript(Cursor::new(first), &database).unwrap();
    assert_eq!(verification.responses, 1_200);
    assert_eq!(verification.duplicate_responses, 60);
    assert_eq!(verification.manifest.live_messages(), 300);
    assert!(verification.checkpoints >= 4);
    assert_eq!(verification.disconnects, 1);
}

#[test]
fn unterminated_batch_reports_only_mailbox_and_sequence_and_mutates_nothing() {
    // Break caught: applying decoded responses before their batch terminator.
    let transcript = br#"{"kind":"begin","batch_id":"b1","mailbox_id":"mailbox-a","sequence":9}
{"kind":"response","event":{"type":"create_mailbox","display_name":"Secret display name","uid_validity":1}}
"#;
    let temporary = tempfile::tempdir().unwrap();
    let database = temporary.path().join("mirror.sqlite");
    let diagnostic = verify_transcript(Cursor::new(transcript), &database).unwrap_err();
    assert_eq!(diagnostic.mailbox_id, "mailbox-a");
    assert_eq!(diagnostic.sequence, 9);
    assert_eq!(diagnostic.code, "missing_batch_terminator");
    assert!(!diagnostic.to_string().contains("Secret"));

    let sqlite = SqliteBaseline::open(&database).unwrap();
    assert!(sqlite.checkpoint("mailbox-a", &[]).is_err());
}

#[test]
fn applying_then_resuming_the_same_transcript_is_idempotent() {
    // Break caught: resume forgetting durable batch IDs and applying complete batches twice.
    let config = GeneratorConfig {
        seed: 23,
        mailboxes: 30,
        live_messages: 300,
        responses: 1_200,
    };
    let mut transcript = Vec::new();
    generate_transcript(&config, &mut transcript).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let database = temporary.path().join("mirror.sqlite");

    let first = apply_transcript(Cursor::new(&transcript), &database, false).unwrap();
    let resumed = apply_transcript(Cursor::new(&transcript), &database, true).unwrap();
    assert_eq!(first.manifest, resumed.manifest);
    assert_eq!(first.manifest.live_messages(), 300);
    assert!(resumed.duplicate_responses > first.duplicate_responses);
}

#[test]
fn every_record_position_in_a_small_transcript_is_truncation_detected() {
    // Break caught: accepting a transcript truncated cleanly between complete batches.
    let records = [
        r#"{"kind":"begin","batch_id":"b1","mailbox_id":"mailbox-a","sequence":1}"#,
        r#"{"kind":"response","event":{"type":"create_mailbox","display_name":"Inbox","uid_validity":1}}"#,
        r#"{"kind":"commit","batch_id":"b1"}"#,
        r#"{"kind":"checkpoint","id":"cp","mailbox_id":"mailbox-a","selected_uids":[]}"#,
        r#"{"kind":"end","responses":1,"records":5}"#,
    ];
    let complete = format!("{}\n", records.join("\n"));
    let complete_directory = tempfile::tempdir().unwrap();
    verify_transcript(
        Cursor::new(complete.as_bytes()),
        &complete_directory.path().join("mirror.sqlite"),
    )
    .unwrap();

    for removed in 0..records.len() {
        let truncated = records
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != removed)
            .map(|(_, record)| *record)
            .collect::<Vec<_>>()
            .join("\n");
        let directory = tempfile::tempdir().unwrap();
        assert!(
            verify_transcript(
                Cursor::new(truncated.as_bytes()),
                &directory.path().join("mirror.sqlite"),
            )
            .is_err(),
            "removing record {removed} was accepted"
        );
    }
}

#[test]
fn chunk_sizes_from_one_byte_to_one_mebibyte_produce_the_same_manifest() {
    // Break caught: line framing depending on the underlying reader's chunk size.
    let config = GeneratorConfig {
        seed: 31,
        mailboxes: 30,
        live_messages: 300,
        responses: 1_200,
    };
    let mut transcript = Vec::new();
    generate_transcript(&config, &mut transcript).unwrap();
    let mut canonical = None;
    for capacity in [1, 7, 1_024, 1_048_576] {
        let directory = tempfile::tempdir().unwrap();
        let reader = BufReader::with_capacity(capacity, Cursor::new(&transcript));
        let summary = verify_transcript(reader, &directory.path().join("mirror.sqlite")).unwrap();
        if let Some(expected) = &canonical {
            assert_eq!(&summary.manifest, expected);
        } else {
            canonical = Some(summary.manifest);
        }
    }
}
