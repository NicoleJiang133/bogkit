//! Deliberately straightforward reference calculation.
//!
//! This module does not call the candidate reconciler. It repeatedly scans
//! sorted vectors and derives literal groups, favoring auditability over speed.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Event, EventAccounting, LinePlan, PaymentStatus, Report, ReturnAuthorization, ReturnPlan,
    SCHEMA_VERSION, Snapshot, Summary, validate_snapshot,
};

#[derive(Clone)]
struct ReferenceScan {
    event_id: String,
    sku: String,
    quantity: u32,
    provenance: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReferenceBlock {
    Successful,
    Pending,
}

/// Calculate the same report with an intentionally separate, scan-oriented algorithm.
///
/// # Errors
///
/// Returns an error if snapshot validation or canonical serialization fails.
pub fn reference_reconcile(snapshot: &Snapshot) -> Result<Report, String> {
    validate_snapshot(snapshot)?;
    let (accepted, accounting, duplicate_count, conflict_count, conflict_returns) =
        reference_canonicalize(&snapshot.events)?;
    let mut events_by_return: BTreeMap<String, Vec<Event>> = BTreeMap::new();
    for event in &accepted {
        events_by_return
            .entry(event.return_id().to_string())
            .or_default()
            .push(event.clone());
    }

    let mut authorizations = snapshot.returns.clone();
    authorizations.sort_by(|left, right| left.return_id.cmp(&right.return_id));
    let mut returns = Vec::with_capacity(authorizations.len());
    for authorization in authorizations {
        let return_id = authorization.return_id.clone();
        returns.push(reference_return(
            authorization,
            events_by_return.get(&return_id).map_or(&[], Vec::as_slice),
            &conflict_returns,
        ));
    }

    let proposed_cents = returns
        .iter()
        .flat_map(|plan| &plan.lines)
        .try_fold(0_u64, |total, line| total.checked_add(line.proposed_cents))
        .ok_or_else(|| "report proposed cents overflow".to_string())?;
    Ok(Report {
        schema_version: SCHEMA_VERSION,
        snapshot_id: snapshot.snapshot_id.clone(),
        advisory_only: true,
        summary: Summary {
            return_count: returns.len(),
            input_event_count: snapshot.events.len(),
            accepted_unique_events: accepted.len(),
            exact_duplicate_events: duplicate_count,
            conflicting_event_ids: conflict_count,
            review_returns: returns
                .iter()
                .filter(|plan| plan.disposition == "review")
                .count(),
            proposed_quantity: returns
                .iter()
                .flat_map(|plan| &plan.lines)
                .map(|line| u64::from(line.proposed_quantity))
                .sum(),
            proposed_cents,
        },
        event_accounting: accounting,
        returns,
    })
}

type CanonicalReference = (
    Vec<Event>,
    Vec<EventAccounting>,
    usize,
    usize,
    BTreeSet<String>,
);

fn reference_canonicalize(events: &[Event]) -> Result<CanonicalReference, String> {
    let mut rows = Vec::with_capacity(events.len());
    for event in events {
        rows.push((
            event.event_id().to_string(),
            serde_json::to_vec(event).map_err(|error| error.to_string())?,
            event,
        ));
    }
    rows.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));

    let mut accepted = Vec::new();
    let mut accounting = Vec::new();
    let mut exact_duplicates = 0_usize;
    let mut conflicts = 0_usize;
    let mut conflict_returns = BTreeSet::new();
    let mut start = 0_usize;
    while start < rows.len() {
        let mut end = start + 1;
        while end < rows.len() && rows[end].0 == rows[start].0 {
            end += 1;
        }
        let first_payload = &rows[start].1;
        let conflicting = rows[start + 1..end]
            .iter()
            .any(|(_, payload, _)| payload != first_payload);
        if conflicting {
            conflicts += 1;
            for (_, _, event) in &rows[start..end] {
                conflict_returns.insert(event.return_id().to_string());
            }
            accounting.push(EventAccounting {
                event_id: rows[start].0.clone(),
                input_occurrences: end - start,
                disposition: "quarantined_conflict".to_string(),
            });
        } else {
            exact_duplicates += end - start - 1;
            accepted.push(rows[start].2.clone());
            accounting.push(EventAccounting {
                event_id: rows[start].0.clone(),
                input_occurrences: end - start,
                disposition: if end - start == 1 {
                    "accepted".to_string()
                } else {
                    "accepted_with_exact_retries".to_string()
                },
            });
        }
        start = end;
    }
    Ok((
        accepted,
        accounting,
        exact_duplicates,
        conflicts,
        conflict_returns,
    ))
}

#[allow(clippy::too_many_lines)]
fn reference_return(
    mut authorization: ReturnAuthorization,
    accepted: &[Event],
    conflict_returns: &BTreeSet<String>,
) -> ReturnPlan {
    authorization
        .lines
        .sort_by(|left, right| left.line_id.cmp(&right.line_id));
    let mut reviews = BTreeSet::new();
    if conflict_returns.contains(&authorization.return_id) {
        reviews.insert("conflicting_event_id".to_string());
    }

    for event in accepted {
        if let Event::Correction {
            return_id,
            target_event_id,
            ..
        } = event
            && return_id == &authorization.return_id
            && !accepted.iter().any(|candidate| {
                    matches!(candidate, Event::WarehouseScan { event_id, .. } if event_id == target_event_id)
                })
        {
            reviews.insert("orphan_correction".to_string());
        }
    }

    let mut scans = Vec::new();
    for event in accepted {
        let Event::WarehouseScan {
            event_id,
            source_timestamp,
            ingestion_id,
            return_id,
            sku,
            quantity,
            ..
        } = event
        else {
            continue;
        };
        if return_id != &authorization.return_id {
            continue;
        }
        let mut corrected_sku = sku.clone();
        let mut corrected_quantity = *quantity;
        let mut provenance = vec![event_id.clone()];
        let mut eligible = Vec::new();
        for correction in accepted {
            let Event::Correction {
                event_id: correction_id,
                source_timestamp: correction_timestamp,
                ingestion_id: correction_ingestion,
                return_id: correction_return,
                target_event_id,
                replacement_sku,
                replacement_quantity,
                ..
            } = correction
            else {
                continue;
            };
            if target_event_id != event_id {
                continue;
            }
            if correction_return != return_id {
                reviews.insert("correction_return_mismatch".to_string());
            } else if (
                *correction_timestamp,
                *correction_ingestion,
                correction_id.as_str(),
            ) <= (*source_timestamp, *ingestion_id, event_id.as_str())
            {
                reviews.insert("non_later_correction".to_string());
            } else {
                eligible.push((
                    (
                        *correction_timestamp,
                        *correction_ingestion,
                        correction_id.as_str(),
                    ),
                    correction_id,
                    replacement_sku,
                    *replacement_quantity,
                ));
            }
        }
        eligible.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some((_, correction_id, replacement_sku, replacement_quantity)) = eligible.last() {
            corrected_sku.clone_from(replacement_sku);
            corrected_quantity = *replacement_quantity;
            provenance.push((*correction_id).clone());
            provenance.sort();
        }
        scans.push(ReferenceScan {
            event_id: event_id.clone(),
            sku: corrected_sku,
            quantity: corrected_quantity,
            provenance,
        });
    }
    scans.sort_by(|left, right| (&left.sku, &left.event_id).cmp(&(&right.sku, &right.event_id)));

    let mut blocks: BTreeMap<String, ReferenceBlock> = BTreeMap::new();
    let mut attempts: BTreeMap<(&str, &str), Vec<(PaymentStatus, u64)>> = BTreeMap::new();
    for event in accepted {
        if let Event::PaymentResult {
            return_id,
            line_id,
            refund_id,
            amount_cents,
            status,
            ..
        } = event
            && return_id == &authorization.return_id
        {
            attempts
                .entry((line_id, refund_id))
                .or_default()
                .push((*status, *amount_cents));
        }
    }
    for ((line_id, _), results) in attempts {
        let different_amounts = results
            .iter()
            .map(|(_, amount)| *amount)
            .collect::<BTreeSet<_>>()
            .len()
            > 1;
        if different_amounts {
            reviews.insert("payment_amount_conflict".to_string());
            blocks.insert(line_id.to_string(), ReferenceBlock::Pending);
        } else if results
            .iter()
            .any(|(status, _)| *status == PaymentStatus::Successful)
        {
            blocks.insert(line_id.to_string(), ReferenceBlock::Successful);
        } else if results
            .iter()
            .any(|(status, _)| *status == PaymentStatus::Pending)
        {
            blocks
                .entry(line_id.to_string())
                .or_insert(ReferenceBlock::Pending);
        }
    }

    let authorized_skus: BTreeSet<String> = authorization
        .lines
        .iter()
        .map(|line| line.sku.clone())
        .collect();
    let line_skus: BTreeMap<String, String> = authorization
        .lines
        .iter()
        .map(|line| (line.line_id.clone(), line.sku.clone()))
        .collect();
    let unmatched_scan_quantity: u64 = scans
        .iter()
        .filter(|scan| !authorized_skus.contains(&scan.sku))
        .map(|scan| u64::from(scan.quantity))
        .sum();
    if unmatched_scan_quantity > 0 {
        reviews.insert("unauthorized_sku".to_string());
    }

    let total_prior: u64 = authorization
        .lines
        .iter()
        .map(|line| line.prior_successful_refund_cents)
        .sum();
    let mut return_cap = authorization.captured_cents - total_prior;
    let mut remaining_scan_quantity: Vec<u32> = scans.iter().map(|scan| scan.quantity).collect();
    let mut lines = Vec::with_capacity(authorization.lines.len());
    for line in authorization.lines {
        let mut accepted_quantity = 0_u32;
        let mut provenance = BTreeSet::new();
        for (scan_index, scan) in scans.iter().enumerate() {
            if scan.sku != line.sku || accepted_quantity == line.authorized_qty {
                continue;
            }
            let wanted = line.authorized_qty - accepted_quantity;
            let taken = remaining_scan_quantity[scan_index].min(wanted);
            if taken > 0 {
                remaining_scan_quantity[scan_index] -= taken;
                accepted_quantity += taken;
                provenance.extend(scan.provenance.iter().cloned());
            }
        }
        let net = line.paid_subtotal_cents + line.tax_cents - line.discount_cents;
        let maximum_refundable_cents = net - line.prior_successful_refund_cents;
        let blocked = blocks.get(&line.line_id).copied();
        if blocked == Some(ReferenceBlock::Successful) {
            reviews.insert("payment_already_successful".to_string());
        } else if blocked == Some(ReferenceBlock::Pending) {
            reviews.insert("payment_inconclusive".to_string());
        }
        let eligible_quantity = if blocked.is_none() {
            accepted_quantity
        } else {
            0
        };
        let unit_base = net / u64::from(line.authorized_qty);
        let remainder = net % u64::from(line.authorized_qty);
        let cent_capacity = maximum_refundable_cents.min(return_cap);
        let mut proposed_quantity = 0;
        for quantity in 1..=eligible_quantity {
            let cost = unit_base * u64::from(quantity) + remainder.min(u64::from(quantity));
            if cost <= cent_capacity {
                proposed_quantity = quantity;
            } else {
                break;
            }
        }
        let proposed_cents =
            unit_base * u64::from(proposed_quantity) + remainder.min(u64::from(proposed_quantity));
        if blocked.is_none() && proposed_quantity < accepted_quantity {
            reviews.insert("insufficient_whole_unit_capacity".to_string());
        }
        return_cap -= proposed_cents;
        lines.push(LinePlan {
            line_id: line.line_id,
            authorized_quantity: line.authorized_qty,
            accepted_scan_quantity: accepted_quantity,
            review_scan_quantity: 0,
            prior_successful_refund_cents: line.prior_successful_refund_cents,
            maximum_refundable_cents,
            proposed_quantity,
            proposed_cents,
            provenance_event_ids: provenance.into_iter().collect(),
        });
    }

    for sku in authorized_skus {
        let excess: u64 = scans
            .iter()
            .enumerate()
            .filter(|(_, scan)| scan.sku == sku)
            .map(|(index, _)| u64::from(remaining_scan_quantity[index]))
            .sum();
        if excess > 0 {
            reviews.insert("excess_quantity".to_string());
            if let Some(line) = lines
                .iter_mut()
                .rev()
                .find(|line| line_skus.get(&line.line_id) == Some(&sku))
            {
                line.review_scan_quantity += excess;
            }
        }
    }

    let review_codes: Vec<String> = reviews.into_iter().collect();
    let has_proposal = lines.iter().any(|line| line.proposed_cents > 0);
    ReturnPlan {
        return_id: authorization.return_id,
        disposition: if review_codes.is_empty() {
            if has_proposal {
                "proposed".to_string()
            } else {
                "no_refund".to_string()
            }
        } else {
            "review".to_string()
        },
        review_codes,
        unmatched_scan_quantity,
        lines,
    }
}
