use causal_canvas_compaction::{Limits, ValidationError, validate_ndjson_line};

#[test]
fn raw_input_is_size_bounded_before_utf8_or_json_parsing() {
    // Catches parsing/allocation before the raw byte cap and accidental panic on
    // invalid UTF-8. The error priority is part of the bounded-input contract.
    let limits = Limits {
        max_line_bytes: 8,
        max_payload_bytes: 4,
        max_pending_operations: 2,
    };
    assert_eq!(
        validate_ndjson_line(&[0xff; 9], limits),
        Err(ValidationError::InputTooLarge)
    );
    assert_eq!(
        validate_ndjson_line(&[0xff; 2], limits),
        Err(ValidationError::InvalidUtf8)
    );
}

#[test]
fn truncated_json_and_unknown_operation_kind_have_stable_errors() {
    // Catches treating an unknown variant as malformed JSON or accepting a
    // partially decoded line.
    let limits = Limits::default();
    assert_eq!(
        validate_ndjson_line(br#"{"id":"cut"#, limits),
        Err(ValidationError::InvalidJson)
    );
    let unknown = br#"{"id":"x-1","document_id":"canvas","actor_id":"x","actor_sequence":1,"dependency_clock":{},"accepted_minute":1,"payload":{"kind":"teleport","object_id":"shape"}}"#;
    assert_eq!(
        validate_ndjson_line(unknown, limits),
        Err(ValidationError::UnknownOperationKind)
    );
}

#[test]
fn payload_and_sequence_limits_reject_valid_json() {
    // Catches checking only the line envelope while allowing an oversized
    // payload, zero sequences, or the boundary that cannot be incremented.
    let limits = Limits {
        max_line_bytes: 1_024,
        max_payload_bytes: 32,
        max_pending_operations: 2,
    };
    let oversized = br#"{"id":"x-1","document_id":"canvas","actor_id":"x","actor_sequence":1,"dependency_clock":{},"accepted_minute":1,"payload":{"kind":"set_property","object_id":"shape","key":"text","value":"012345678901234567890123456789012345678901234567890123456789"}}"#;
    assert_eq!(
        validate_ndjson_line(oversized, limits),
        Err(ValidationError::PayloadTooLarge)
    );

    let zero = br#"{"id":"x-0","document_id":"canvas","actor_id":"x","actor_sequence":0,"dependency_clock":{},"accepted_minute":1,"payload":{"kind":"create","object_id":"shape","object_kind":"rect"}}"#;
    assert_eq!(
        validate_ndjson_line(zero, Limits::default()),
        Err(ValidationError::InvalidSequence)
    );

    let overflow = br#"{"id":"x-max","document_id":"canvas","actor_id":"x","actor_sequence":18446744073709551615,"dependency_clock":{},"accepted_minute":1,"payload":{"kind":"create","object_id":"shape","object_kind":"rect"}}"#;
    assert_eq!(
        validate_ndjson_line(overflow, Limits::default()),
        Err(ValidationError::SequenceOverflow)
    );
}
