use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use fold::pipeline::{Keyed, Scored, terminal::search::Bm25};
use fold::stream::Stream;
use revision_safe_manual_search_trial::{
    Applicability, Evidence, RevisionMeta, SafetyError, select_eligible_from_complete_ranking,
};

const SELECTED_TIME: i64 = 1_750_000_000;

fn temp_store(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("bogkit-trial2-{label}-{}", std::process::id()))
}

fn revision(doc_id: u64, applicable: Applicability) -> RevisionMeta {
    RevisionMeta {
        doc_id,
        evidence: Evidence {
            manual_id: "AMM-32".to_string(),
            stable_section_id: format!("32-40-{doc_id:02}"),
            revision_id: format!("R-{doc_id:02}"),
            effective_from: 1_700_000_000,
            effective_until: 1_800_000_000,
        },
        applicability: applicable,
    }
}

fn ranked_fixture(limit: usize, label: &str) -> (Vec<Scored<f64, u64>>, Vec<RevisionMeta>) {
    let store = temp_store(label);
    let _ = fs::remove_dir_all(&store);

    let mut stream = Stream::new(&store, Bm25::new("manual_text"));
    stream.wtx(|tx| {
        for doc_id in 1_u64..=20 {
            tx.insert(&Keyed::new(
                doc_id,
                "pump chatter cold soak pump chatter cold soak".to_string(),
            ));
        }
        tx.insert(&Keyed::new(21_u64, "pump".to_string()));
    });

    let ranked = stream.rtx(|index| index.search("pump chatter cold soak", limit));
    drop(stream);
    fs::remove_dir_all(&store).expect("fixture store should be removable");

    let mut revisions = (1_u64..=20)
        .map(|doc_id| revision(doc_id, Applicability::Known(false)))
        .collect::<Vec<_>>();
    revisions.push(revision(21, Applicability::Known(true)));
    (ranked, revisions)
}

#[test]
fn bounded_top_twenty_then_filter_loses_the_lower_eligible_revision() {
    let (ranked, revisions) = ranked_fixture(20, "bounded-baseline");
    let applicability = revisions
        .iter()
        .map(|revision| {
            (
                revision.doc_id,
                matches!(revision.applicability, Applicability::Known(true)),
            )
        })
        .collect::<HashMap<_, _>>();

    let eligible = ranked
        .iter()
        .filter(|hit| applicability.get(&hit.val) == Some(&true))
        .collect::<Vec<_>>();

    assert_eq!(ranked.len(), 20);
    assert!(ranked.iter().all(|hit| hit.val != 21));
    assert!(eligible.is_empty());
}

#[test]
fn complete_ranking_filters_eligibility_before_final_top_ten() {
    let (ranked, revisions) = ranked_fixture(21, "complete-ranking");

    let hits = select_eligible_from_complete_ranking(&ranked, &revisions, SELECTED_TIME, 10)
        .expect("valid fixture should search safely");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].doc_id, 21);
    assert_eq!(hits[0].evidence.manual_id, "AMM-32");
    assert_eq!(hits[0].evidence.stable_section_id, "32-40-21");
    assert_eq!(hits[0].evidence.revision_id, "R-21");
    assert_eq!(hits[0].evidence.effective_from, 1_700_000_000);
    assert_eq!(hits[0].evidence.effective_until, 1_800_000_000);
}

#[test]
fn two_active_revisions_for_one_stable_section_fail_closed() {
    let ranked = vec![Scored::new(1.0, 1_u64), Scored::new(0.9, 2_u64)];
    let mut first = revision(1, Applicability::Known(true));
    let mut second = revision(2, Applicability::Known(true));
    first.evidence.stable_section_id = "32-40-PUMP".to_string();
    second.evidence.stable_section_id = "32-40-PUMP".to_string();

    let error = select_eligible_from_complete_ranking(&ranked, &[first, second], SELECTED_TIME, 10)
        .expect_err("overlapping active revisions must reject the complete input");

    assert_eq!(
        error,
        SafetyError::TwoActiveRevisions {
            stable_section_id: "32-40-PUMP".to_string(),
        }
    );
}

#[test]
fn unknown_applicability_attribute_fails_closed() {
    let ranked = vec![Scored::new(1.0, 1_u64)];
    let revisions = vec![revision(
        1,
        Applicability::UnknownAttribute("winglet_mod".to_string()),
    )];

    let error = select_eligible_from_complete_ranking(&ranked, &revisions, SELECTED_TIME, 10)
        .expect_err("unknown applicability input must reject the complete input");

    assert_eq!(
        error,
        SafetyError::UnknownAttribute {
            attribute: "winglet_mod".to_string(),
        }
    );
}

#[test]
fn duplicate_revision_id_fails_closed() {
    let ranked = vec![Scored::new(1.0, 1_u64), Scored::new(0.9, 2_u64)];
    let first = revision(1, Applicability::Known(true));
    let mut second = revision(2, Applicability::Known(true));
    second.evidence.revision_id = first.evidence.revision_id.clone();

    let error = select_eligible_from_complete_ranking(&ranked, &[first, second], SELECTED_TIME, 10)
        .expect_err("duplicate revision IDs must reject the complete input");

    assert_eq!(
        error,
        SafetyError::DuplicateRevisionId {
            revision_id: "R-01".to_string(),
        }
    );
}

#[test]
fn ranked_document_without_metadata_fails_closed() {
    let ranked = vec![Scored::new(1.0, 99_u64)];

    let error = select_eligible_from_complete_ranking(&ranked, &[], SELECTED_TIME, 10)
        .expect_err("a ranked document without canonical metadata must fail closed");

    assert_eq!(error, SafetyError::MissingMetadata { doc_id: 99 });
}

#[test]
fn duplicate_metadata_doc_id_is_rejected_regardless_of_row_order() {
    let ranked = vec![Scored::new(1.0, 41_u64)];
    let eligible = revision(41, Applicability::Known(true));
    let mut ineligible = revision(42, Applicability::Known(false));
    ineligible.doc_id = 41;

    let forward = select_eligible_from_complete_ranking(
        &ranked,
        &[eligible.clone(), ineligible.clone()],
        SELECTED_TIME,
        10,
    );
    let reversed =
        select_eligible_from_complete_ranking(&ranked, &[ineligible, eligible], SELECTED_TIME, 10);
    let expected = Err(SafetyError::DuplicateDocumentId { doc_id: 41 });

    assert_eq!(forward, expected);
    assert_eq!(reversed, expected);
}

#[test]
fn duplicate_ranked_doc_id_fails_closed() {
    let ranked = vec![Scored::new(1.0, 1_u64), Scored::new(0.9, 1_u64)];
    let revisions = vec![revision(1, Applicability::Known(true))];

    let error = select_eligible_from_complete_ranking(&ranked, &revisions, SELECTED_TIME, 10)
        .expect_err("a duplicate ranked key must reject the complete input");

    assert_eq!(error, SafetyError::DuplicateRankedDocumentId { doc_id: 1 });
}

#[test]
fn unique_metadata_row_order_does_not_change_eligibility_or_evidence() {
    let ranked = vec![Scored::new(1.0, 1_u64), Scored::new(0.9, 2_u64)];
    let eligible = revision(1, Applicability::Known(true));
    let ineligible = revision(2, Applicability::Known(false));

    let forward = select_eligible_from_complete_ranking(
        &ranked,
        &[eligible.clone(), ineligible.clone()],
        SELECTED_TIME,
        10,
    )
    .expect("unique metadata should be accepted");
    let reversed =
        select_eligible_from_complete_ranking(&ranked, &[ineligible, eligible], SELECTED_TIME, 10)
            .expect("metadata order must not affect valid selection");

    assert_eq!(forward, reversed);
    assert_eq!(forward.len(), 1);
    assert_eq!(forward[0].doc_id, 1);
    assert_eq!(forward[0].evidence.manual_id, "AMM-32");
    assert_eq!(forward[0].evidence.stable_section_id, "32-40-01");
    assert_eq!(forward[0].evidence.revision_id, "R-01");
}

#[test]
fn equal_scores_tie_break_by_stable_section_then_revision() {
    let ranked = vec![
        Scored::new(1.0, 3_u64),
        Scored::new(1.0, 2_u64),
        Scored::new(1.0, 1_u64),
    ];
    let mut z = revision(1, Applicability::Known(true));
    let mut a2 = revision(2, Applicability::Known(true));
    let mut a1 = revision(3, Applicability::Known(true));
    z.evidence.stable_section_id = "Z".to_string();
    z.evidence.revision_id = "R-Z".to_string();
    a2.evidence.stable_section_id = "A".to_string();
    a2.evidence.revision_id = "R-2".to_string();
    a2.evidence.manual_id = "AMM-B".to_string();
    a1.evidence.stable_section_id = "A".to_string();
    a1.evidence.revision_id = "R-1".to_string();
    a1.evidence.manual_id = "AMM-C".to_string();

    let hits = select_eligible_from_complete_ranking(&ranked, &[z, a2, a1], SELECTED_TIME, 10)
        .expect("valid ties should be ordered deterministically");

    let ordered_ids = hits.iter().map(|hit| hit.doc_id).collect::<Vec<_>>();
    assert_eq!(ordered_ids, vec![3, 2, 1]);
}
