use std::process::Command;

#[test]
fn runnable_demo_reports_the_exact_baseline_failure_and_bounded_safe_result() {
    let output = Command::new(env!("CARGO_BIN_EXE_revision-safe-manual-search-trial"))
        .output()
        .expect("demo binary should run");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("demo output should be UTF-8"),
        concat!(
            "documents=21\n",
            "baseline_candidate_limit=20\n",
            "baseline_eligible_results=0\n",
            "complete_ranking_candidates=21\n",
            "safe_eligible_results=1\n",
            "top_evidence=AMM-32|32-40-21|R-21|1700000000..1800000000\n",
            "decision=NO_FIT_GLOBAL_INDEX_HAS_NO_QUERY_ELIGIBILITY_FILTER\n",
        )
    );
}
