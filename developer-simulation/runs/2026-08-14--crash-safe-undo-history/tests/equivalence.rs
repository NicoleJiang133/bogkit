use undo_history_lab::fixture::generate_fixture;
use undo_history_lab::runner::run_fixture;

#[test]
fn candidate_baseline_and_reference_match_on_checkpoints_and_final_state() {
    let fixture = generate_fixture(2026, 16, 160).unwrap();
    let first_dir = tempfile::tempdir().unwrap();
    let second_dir = tempfile::tempdir().unwrap();
    let third_dir = tempfile::tempdir().unwrap();

    let first = run_fixture(&fixture, first_dir.path()).unwrap();
    let second = run_fixture(&fixture, second_dir.path()).unwrap();
    let third = run_fixture(&fixture, third_dir.path()).unwrap();

    assert_eq!(first.reference_status, first.candidate_status);
    assert_eq!(first.reference_status, first.baseline_status);
    assert_eq!(first.checkpoints_verified, 0);
    assert_eq!(first.candidate.sample_count, fixture.edit_submissions);
    assert_eq!(first.baseline.sample_count, fixture.edit_submissions);
    assert_eq!(first.transcript_digest, second.transcript_digest);
    assert_eq!(first.transcript_digest, third.transcript_digest);
    assert_eq!(first.canonical_json, second.canonical_json);
    assert_eq!(first.canonical_json, third.canonical_json);
}
