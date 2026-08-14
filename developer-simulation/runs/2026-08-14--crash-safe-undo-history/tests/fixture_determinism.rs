use undo_history_lab::fixture::generate_fixture;

#[test]
fn same_seed_is_byte_identical_and_different_seed_changes_output() {
    let first = serde_json::to_vec(&generate_fixture(17, 24, 200).unwrap()).unwrap();
    let second = serde_json::to_vec(&generate_fixture(17, 24, 200).unwrap()).unwrap();
    let different = serde_json::to_vec(&generate_fixture(18, 24, 200).unwrap()).unwrap();

    assert_eq!(first, second);
    assert_ne!(first, different);
}

#[test]
fn generator_records_reference_checkpoints_and_exactly_ten_percent_retries() {
    let fixture = generate_fixture(91, 32, 1_000).unwrap();

    assert_eq!(fixture.actions.len(), 1_000);
    assert_eq!(
        fixture.duplicate_edit_submissions,
        fixture.edit_submissions / 10
    );
    assert_eq!(fixture.checkpoints.len(), 1);
    assert_eq!(fixture.checkpoints[0].at_action, 1_000);
    assert_eq!(fixture.checkpoints[0].status, fixture.expected_final);
}
