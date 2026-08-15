use std::collections::{HashMap, HashSet};

use fold::pipeline::Scored;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Applicability {
    Known(bool),
    UnknownAttribute(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Evidence {
    pub manual_id: String,
    pub stable_section_id: String,
    pub revision_id: String,
    pub effective_from: i64,
    pub effective_until: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevisionMeta {
    pub doc_id: u64,
    pub evidence: Evidence,
    pub applicability: Applicability,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EligibleHit {
    pub doc_id: u64,
    pub score: f64,
    pub evidence: Evidence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SafetyError {
    DuplicateDocumentId { doc_id: u64 },
    DuplicateRankedDocumentId { doc_id: u64 },
    DuplicateRevisionId { revision_id: String },
    MissingMetadata { doc_id: u64 },
    TwoActiveRevisions { stable_section_id: String },
    UnknownAttribute { attribute: String },
}

/// Validates canonical metadata, filters a complete lexical ranking to active
/// and applicable revisions, applies the required deterministic tie-break,
/// and returns exact source evidence.
///
/// # Errors
///
/// Returns [`SafetyError`] when canonical or ranked document IDs repeat, the
/// input contains duplicate revision IDs or overlapping active revisions,
/// applicability data is unknown, or a ranked document lacks canonical
/// metadata.
pub fn select_eligible_from_complete_ranking(
    ranked: &[Scored<f64, u64>],
    revisions: &[RevisionMeta],
    effective_time: i64,
    limit: usize,
) -> Result<Vec<EligibleHit>, SafetyError> {
    let mut document_ids = HashSet::with_capacity(revisions.len());
    for revision in revisions {
        if !document_ids.insert(revision.doc_id) {
            return Err(SafetyError::DuplicateDocumentId {
                doc_id: revision.doc_id,
            });
        }
    }

    let mut ranked_document_ids = HashSet::with_capacity(ranked.len());
    for ranked_hit in ranked {
        if !ranked_document_ids.insert(ranked_hit.val) {
            return Err(SafetyError::DuplicateRankedDocumentId {
                doc_id: ranked_hit.val,
            });
        }
    }

    let mut revision_ids = HashSet::with_capacity(revisions.len());
    let mut active_sections = HashSet::with_capacity(revisions.len());
    let mut by_doc_id = HashMap::with_capacity(revisions.len());

    for revision in revisions {
        if !revision_ids.insert(&revision.evidence.revision_id) {
            return Err(SafetyError::DuplicateRevisionId {
                revision_id: revision.evidence.revision_id.clone(),
            });
        }

        if let Applicability::UnknownAttribute(attribute) = &revision.applicability {
            return Err(SafetyError::UnknownAttribute {
                attribute: attribute.clone(),
            });
        }

        if revision.evidence.effective_from <= effective_time
            && effective_time < revision.evidence.effective_until
            && !active_sections.insert((
                &revision.evidence.manual_id,
                &revision.evidence.stable_section_id,
            ))
        {
            return Err(SafetyError::TwoActiveRevisions {
                stable_section_id: revision.evidence.stable_section_id.clone(),
            });
        }

        by_doc_id.insert(revision.doc_id, revision);
    }

    let mut eligible = Vec::new();
    for ranked_hit in ranked {
        let revision = by_doc_id
            .get(&ranked_hit.val)
            .ok_or(SafetyError::MissingMetadata {
                doc_id: ranked_hit.val,
            })?;
        let is_active = revision.evidence.effective_from <= effective_time
            && effective_time < revision.evidence.effective_until;
        if is_active && revision.applicability == Applicability::Known(true) {
            eligible.push(EligibleHit {
                doc_id: ranked_hit.val,
                score: ranked_hit.score,
                evidence: revision.evidence.clone(),
            });
        }
    }

    eligible.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| {
                left.evidence
                    .stable_section_id
                    .cmp(&right.evidence.stable_section_id)
            })
            .then_with(|| left.evidence.revision_id.cmp(&right.evidence.revision_id))
    });
    eligible.truncate(limit);
    Ok(eligible)
}
