use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use fold::pipeline::{Keyed, terminal::search::Bm25};
use fold::stream::Stream;
use revision_safe_manual_search_trial::{
    Applicability, Evidence, RevisionMeta, select_eligible_from_complete_ranking,
};

const SELECTED_TIME: i64 = 1_750_000_000;

struct TempStore(PathBuf);

impl Drop for TempStore {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn revision(doc_id: u64, applicable: bool) -> RevisionMeta {
    RevisionMeta {
        doc_id,
        evidence: Evidence {
            manual_id: "AMM-32".to_string(),
            stable_section_id: format!("32-40-{doc_id:02}"),
            revision_id: format!("R-{doc_id:02}"),
            effective_from: 1_700_000_000,
            effective_until: 1_800_000_000,
        },
        applicability: Applicability::Known(applicable),
    }
}

fn main() {
    let store =
        TempStore(std::env::temp_dir().join(format!("bogkit-trial2-demo-{}", std::process::id())));
    let _ = fs::remove_dir_all(&store.0);

    let mut stream = Stream::new(&store.0, Bm25::new("manual_text"));
    stream.wtx(|tx| {
        for doc_id in 1_u64..=20 {
            tx.insert(&Keyed::new(
                doc_id,
                "pump chatter cold soak pump chatter cold soak".to_string(),
            ));
        }
        tx.insert(&Keyed::new(21_u64, "pump".to_string()));
    });

    let mut revisions = (1_u64..=20)
        .map(|doc_id| revision(doc_id, false))
        .collect::<Vec<_>>();
    revisions.push(revision(21, true));
    let applicable = revisions
        .iter()
        .map(|revision| {
            (
                revision.doc_id,
                revision.applicability == Applicability::Known(true),
            )
        })
        .collect::<HashMap<_, _>>();

    let baseline = stream.rtx(|index| index.search("pump chatter cold soak", 20));
    let baseline_eligible = baseline
        .iter()
        .filter(|hit| applicable.get(&hit.val) == Some(&true))
        .count();
    let complete = stream.rtx(|index| index.search("pump chatter cold soak", 21));
    let safe = select_eligible_from_complete_ranking(&complete, &revisions, SELECTED_TIME, 10)
        .expect("the validated fixture must pass safety selection");
    let top = safe.first().expect("the eligible record must be recovered");

    println!("documents={}", revisions.len());
    println!("baseline_candidate_limit={}", baseline.len());
    println!("baseline_eligible_results={baseline_eligible}");
    println!("complete_ranking_candidates={}", complete.len());
    println!("safe_eligible_results={}", safe.len());
    println!(
        "top_evidence={}|{}|{}|{}..{}",
        top.evidence.manual_id,
        top.evidence.stable_section_id,
        top.evidence.revision_id,
        top.evidence.effective_from,
        top.evidence.effective_until
    );
    println!("decision=NO_FIT_GLOBAL_INDEX_HAS_NO_QUERY_ELIGIBILITY_FILTER");
}
