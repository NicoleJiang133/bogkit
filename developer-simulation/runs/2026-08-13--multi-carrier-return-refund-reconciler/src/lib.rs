//! Deterministic, advisory-only refund reconciliation over immutable snapshots.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

mod reference;
pub use reference::reference_reconcile;
mod workload;
pub use workload::{deterministic_shuffle, generate_representative};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub snapshot_id: String,
    pub returns: Vec<ReturnAuthorization>,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnAuthorization {
    pub return_id: String,
    pub captured_cents: u64,
    pub lines: Vec<AuthorizedLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizedLine {
    pub line_id: String,
    pub sku: String,
    pub authorized_qty: u32,
    pub paid_subtotal_cents: u64,
    pub tax_cents: u64,
    pub discount_cents: u64,
    pub prior_successful_refund_cents: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    Successful,
    Failed,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    WarehouseScan {
        event_id: String,
        source_system_id: String,
        source_timestamp: u64,
        ingestion_id: u64,
        return_id: String,
        parcel_id: String,
        sku: String,
        quantity: u32,
    },
    Correction {
        event_id: String,
        source_system_id: String,
        source_timestamp: u64,
        ingestion_id: u64,
        return_id: String,
        target_event_id: String,
        replacement_sku: String,
        replacement_quantity: u32,
    },
    Carrier {
        event_id: String,
        source_system_id: String,
        source_timestamp: u64,
        ingestion_id: u64,
        return_id: String,
        parcel_id: String,
        status: String,
    },
    PaymentResult {
        event_id: String,
        source_system_id: String,
        source_timestamp: u64,
        ingestion_id: u64,
        return_id: String,
        line_id: String,
        refund_id: String,
        amount_cents: u64,
        status: PaymentStatus,
    },
}

impl Event {
    #[must_use]
    pub fn event_id(&self) -> &str {
        match self {
            Self::WarehouseScan { event_id, .. }
            | Self::Correction { event_id, .. }
            | Self::Carrier { event_id, .. }
            | Self::PaymentResult { event_id, .. } => event_id,
        }
    }

    #[must_use]
    pub fn return_id(&self) -> &str {
        match self {
            Self::WarehouseScan { return_id, .. }
            | Self::Correction { return_id, .. }
            | Self::Carrier { return_id, .. }
            | Self::PaymentResult { return_id, .. } => return_id,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub return_count: usize,
    pub input_event_count: usize,
    pub accepted_unique_events: usize,
    pub exact_duplicate_events: usize,
    pub conflicting_event_ids: usize,
    pub review_returns: usize,
    pub proposed_quantity: u64,
    pub proposed_cents: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub advisory_only: bool,
    pub summary: Summary,
    pub event_accounting: Vec<EventAccounting>,
    pub returns: Vec<ReturnPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventAccounting {
    pub event_id: String,
    pub input_occurrences: usize,
    pub disposition: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnPlan {
    pub return_id: String,
    pub disposition: String,
    pub review_codes: Vec<String>,
    pub unmatched_scan_quantity: u64,
    pub lines: Vec<LinePlan>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinePlan {
    pub line_id: String,
    pub authorized_quantity: u32,
    pub accepted_scan_quantity: u32,
    pub review_scan_quantity: u64,
    pub prior_successful_refund_cents: u64,
    pub maximum_refundable_cents: u64,
    pub proposed_quantity: u32,
    pub proposed_cents: u64,
    pub provenance_event_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailPoint {
    None,
    BeforeOutputCreation,
    AfterTemporaryComplete,
    BeforePublish,
    ExitBeforeOutputCreation,
    ExitAfterTemporaryComplete,
    ExitBeforePublish,
}

#[derive(Clone)]
struct Scan {
    event_id: String,
    return_id: String,
    sku: String,
    quantity: u32,
    source_timestamp: u64,
    ingestion_id: u64,
    provenance: Vec<String>,
}

#[derive(Default)]
struct CanonicalEvents {
    accepted: Vec<Event>,
    event_accounting: Vec<EventAccounting>,
    exact_duplicates: usize,
    conflicts: usize,
    conflict_returns: BTreeSet<String>,
}

#[derive(Default)]
struct ReturnWork {
    review_codes: BTreeSet<String>,
    scans: Vec<Scan>,
    blocked_lines: BTreeMap<String, PaymentBlock>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaymentBlock {
    Successful,
    Pending,
}

/// Validate and reconcile one immutable snapshot into a canonical advisory report.
///
/// # Errors
///
/// Returns an error when identifiers, monetary bounds, structural limits, or
/// cross-record references make the snapshot unsafe to interpret.
#[allow(clippy::too_many_lines)]
pub fn reconcile(snapshot: &Snapshot) -> Result<Report, String> {
    validate_snapshot(snapshot)?;
    let canonical = canonicalize_events(&snapshot.events)?;

    let mut work: BTreeMap<String, ReturnWork> = snapshot
        .returns
        .iter()
        .map(|authorization| (authorization.return_id.clone(), ReturnWork::default()))
        .collect();
    for return_id in &canonical.conflict_returns {
        work.get_mut(return_id)
            .ok_or_else(|| format!("event references unknown return {return_id}"))?
            .review_codes
            .insert("conflicting_event_id".to_string());
    }

    let accepted_by_id: BTreeMap<&str, &Event> = canonical
        .accepted
        .iter()
        .map(|event| (event.event_id(), event))
        .collect();
    let mut corrections_by_target: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for event in &canonical.accepted {
        if let Event::Correction {
            target_event_id, ..
        } = event
        {
            corrections_by_target
                .entry(target_event_id)
                .or_default()
                .push(event);
        }
    }

    for event in &canonical.accepted {
        match event {
            Event::WarehouseScan {
                event_id,
                source_timestamp,
                ingestion_id,
                return_id,
                sku,
                quantity,
                ..
            } => {
                let mut scan = Scan {
                    event_id: event_id.clone(),
                    return_id: return_id.clone(),
                    sku: sku.clone(),
                    quantity: *quantity,
                    source_timestamp: *source_timestamp,
                    ingestion_id: *ingestion_id,
                    provenance: vec![event_id.clone()],
                };
                if let Some(corrections) = corrections_by_target.get(event_id.as_str()) {
                    apply_latest_correction(&mut scan, corrections, &mut work)?;
                }
                work.get_mut(return_id)
                    .ok_or_else(|| format!("scan references unknown return {return_id}"))?
                    .scans
                    .push(scan);
            }
            Event::Correction {
                return_id,
                target_event_id,
                ..
            } => {
                if !matches!(
                    accepted_by_id.get(target_event_id.as_str()),
                    Some(Event::WarehouseScan { .. })
                ) {
                    work.get_mut(return_id)
                        .ok_or_else(|| format!("correction references unknown return {return_id}"))?
                        .review_codes
                        .insert("orphan_correction".to_string());
                }
            }
            Event::PaymentResult { .. } | Event::Carrier { .. } => {}
        }
    }
    apply_payment_results(&canonical.accepted, &mut work)?;

    let mut authorizations = snapshot.returns.clone();
    authorizations.sort_by(|left, right| left.return_id.cmp(&right.return_id));
    let mut plans = Vec::with_capacity(authorizations.len());
    for authorization in authorizations {
        let return_work = work
            .remove(&authorization.return_id)
            .ok_or_else(|| "internal return work missing".to_string())?;
        plans.push(plan_return(authorization, return_work)?);
    }

    let proposed_cents = plans
        .iter()
        .flat_map(|plan| &plan.lines)
        .try_fold(0_u64, |total, line| total.checked_add(line.proposed_cents))
        .ok_or_else(|| "report proposed cents overflow".to_string())?;
    let summary = Summary {
        return_count: plans.len(),
        input_event_count: snapshot.events.len(),
        accepted_unique_events: canonical.accepted.len(),
        exact_duplicate_events: canonical.exact_duplicates,
        conflicting_event_ids: canonical.conflicts,
        review_returns: plans
            .iter()
            .filter(|plan| plan.disposition == "review")
            .count(),
        proposed_quantity: plans
            .iter()
            .flat_map(|plan| &plan.lines)
            .map(|line| u64::from(line.proposed_quantity))
            .sum(),
        proposed_cents,
    };

    Ok(Report {
        schema_version: SCHEMA_VERSION,
        snapshot_id: snapshot.snapshot_id.clone(),
        advisory_only: true,
        summary,
        event_accounting: canonical.event_accounting,
        returns: plans,
    })
}

fn canonicalize_events(events: &[Event]) -> Result<CanonicalEvents, String> {
    let mut groups: BTreeMap<&str, Vec<(&Event, Vec<u8>)>> = BTreeMap::new();
    for event in events {
        if event.event_id().is_empty() {
            return Err("event_id must not be empty".to_string());
        }
        groups.entry(event.event_id()).or_default().push((
            event,
            serde_json::to_vec(event).map_err(|error| error.to_string())?,
        ));
    }

    let mut result = CanonicalEvents::default();
    for (event_id, mut occurrences) in groups {
        occurrences.sort_by(|left, right| left.1.cmp(&right.1));
        let distinct_payloads = occurrences
            .iter()
            .map(|(_, bytes)| bytes)
            .collect::<BTreeSet<_>>()
            .len();
        if distinct_payloads > 1 {
            result.conflicts += 1;
            result.conflict_returns.extend(
                occurrences
                    .iter()
                    .map(|(event, _)| event.return_id().to_string()),
            );
            result.event_accounting.push(EventAccounting {
                event_id: event_id.to_string(),
                input_occurrences: occurrences.len(),
                disposition: "quarantined_conflict".to_string(),
            });
        } else {
            result.exact_duplicates += occurrences.len().saturating_sub(1);
            result.accepted.push(occurrences[0].0.clone());
            result.event_accounting.push(EventAccounting {
                event_id: event_id.to_string(),
                input_occurrences: occurrences.len(),
                disposition: if occurrences.len() == 1 {
                    "accepted".to_string()
                } else {
                    "accepted_with_exact_retries".to_string()
                },
            });
        }
    }
    Ok(result)
}

fn apply_latest_correction(
    scan: &mut Scan,
    corrections: &[&Event],
    work: &mut BTreeMap<String, ReturnWork>,
) -> Result<(), String> {
    let mut eligible = Vec::new();
    for correction in corrections {
        let Event::Correction {
            event_id,
            source_timestamp,
            ingestion_id,
            return_id,
            replacement_sku,
            replacement_quantity,
            ..
        } = correction
        else {
            continue;
        };
        if return_id != &scan.return_id {
            work.get_mut(return_id)
                .ok_or_else(|| format!("correction references unknown return {return_id}"))?
                .review_codes
                .insert("correction_return_mismatch".to_string());
            continue;
        }
        if (*source_timestamp, *ingestion_id, event_id.as_str())
            <= (
                scan.source_timestamp,
                scan.ingestion_id,
                scan.event_id.as_str(),
            )
        {
            work.get_mut(return_id)
                .ok_or_else(|| format!("correction references unknown return {return_id}"))?
                .review_codes
                .insert("non_later_correction".to_string());
            continue;
        }
        eligible.push((
            (*source_timestamp, *ingestion_id, event_id.as_str()),
            event_id,
            replacement_sku,
            *replacement_quantity,
        ));
    }
    eligible.sort_by(|left, right| left.0.cmp(&right.0));
    if let Some((_, event_id, replacement_sku, replacement_quantity)) = eligible.last() {
        scan.sku.clone_from(replacement_sku);
        scan.quantity = *replacement_quantity;
        scan.provenance.push((*event_id).clone());
        scan.provenance.sort();
    }
    Ok(())
}

fn apply_payment_results(
    events: &[Event],
    work: &mut BTreeMap<String, ReturnWork>,
) -> Result<(), String> {
    type PaymentKey<'a> = (&'a str, &'a str, &'a str);
    type PaymentObservation = (PaymentStatus, u64);
    let mut attempts: BTreeMap<PaymentKey<'_>, Vec<PaymentObservation>> = BTreeMap::new();
    for event in events {
        if let Event::PaymentResult {
            return_id,
            line_id,
            refund_id,
            amount_cents,
            status,
            ..
        } = event
        {
            attempts
                .entry((return_id, line_id, refund_id))
                .or_default()
                .push((*status, *amount_cents));
        }
    }

    for ((return_id, line_id, _), results) in attempts {
        let state = if results
            .iter()
            .any(|(status, _)| *status == PaymentStatus::Successful)
        {
            Some(PaymentBlock::Successful)
        } else if results
            .iter()
            .any(|(status, _)| *status == PaymentStatus::Pending)
        {
            Some(PaymentBlock::Pending)
        } else {
            None
        };
        let return_work = work
            .get_mut(return_id)
            .ok_or_else(|| format!("payment result references unknown return {return_id}"))?;
        if results
            .iter()
            .map(|(_, amount)| amount)
            .collect::<BTreeSet<_>>()
            .len()
            > 1
        {
            return_work
                .review_codes
                .insert("payment_amount_conflict".to_string());
            return_work
                .blocked_lines
                .insert(line_id.to_string(), PaymentBlock::Pending);
        } else if let Some(state) = state {
            return_work
                .blocked_lines
                .entry(line_id.to_string())
                .and_modify(|old| {
                    if state == PaymentBlock::Successful {
                        *old = state;
                    }
                })
                .or_insert(state);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn plan_return(
    mut authorization: ReturnAuthorization,
    mut work: ReturnWork,
) -> Result<ReturnPlan, String> {
    authorization
        .lines
        .sort_by(|left, right| left.line_id.cmp(&right.line_id));
    work.scans
        .sort_by(|left, right| (&left.sku, &left.event_id).cmp(&(&right.sku, &right.event_id)));

    let total_prior: u64 = authorization
        .lines
        .iter()
        .map(|line| line.prior_successful_refund_cents)
        .sum();
    let mut return_cap = authorization.captured_cents.saturating_sub(total_prior);
    let mut available: BTreeMap<String, Vec<(String, u32, Vec<String>)>> = BTreeMap::new();
    for scan in work.scans {
        available.entry(scan.sku).or_default().push((
            scan.event_id,
            scan.quantity,
            scan.provenance,
        ));
    }

    let line_skus: BTreeMap<String, String> = authorization
        .lines
        .iter()
        .map(|line| (line.line_id.clone(), line.sku.clone()))
        .collect();
    let authorized_skus: BTreeSet<String> = authorization
        .lines
        .iter()
        .map(|line| line.sku.clone())
        .collect();
    let mut unmatched_scan_quantity = 0_u64;
    for (sku, scans) in &available {
        if !authorized_skus.contains(sku) {
            unmatched_scan_quantity += scans
                .iter()
                .map(|(_, quantity, _)| u64::from(*quantity))
                .sum::<u64>();
        }
    }
    if unmatched_scan_quantity > 0 {
        work.review_codes.insert("unauthorized_sku".to_string());
    }

    let mut lines = Vec::with_capacity(authorization.lines.len());
    for line in authorization.lines {
        let scan_chunks = available.entry(line.sku.clone()).or_default();
        let mut needed = line.authorized_qty;
        let mut accepted = 0_u32;
        let mut provenance = BTreeSet::new();
        for (_, quantity, event_provenance) in scan_chunks.iter_mut() {
            if needed == 0 {
                break;
            }
            let taken = (*quantity).min(needed);
            if taken > 0 {
                provenance.extend(event_provenance.iter().cloned());
                *quantity -= taken;
                needed -= taken;
                accepted += taken;
            }
        }

        let net = line
            .paid_subtotal_cents
            .checked_add(line.tax_cents)
            .and_then(|gross| gross.checked_sub(line.discount_cents))
            .ok_or_else(|| format!("invalid monetary values for line {}", line.line_id))?;
        let maximum_refundable_cents = net.saturating_sub(line.prior_successful_refund_cents);
        let payment_block = work.blocked_lines.get(&line.line_id).copied();
        if payment_block == Some(PaymentBlock::Successful) {
            work.review_codes
                .insert("payment_already_successful".to_string());
        } else if payment_block == Some(PaymentBlock::Pending) {
            work.review_codes.insert("payment_inconclusive".to_string());
        }

        let eligible_quantity = if payment_block.is_none() { accepted } else { 0 };
        let cent_capacity = maximum_refundable_cents.min(return_cap);
        let proposed_quantity = (0..=eligible_quantity)
            .rev()
            .find(|quantity| {
                amount_for_first_units(net, line.authorized_qty, *quantity) <= cent_capacity
            })
            .unwrap_or(0);
        let proposed_cents = amount_for_first_units(net, line.authorized_qty, proposed_quantity);
        if payment_block.is_none() && proposed_quantity < accepted {
            work.review_codes
                .insert("insufficient_whole_unit_capacity".to_string());
        }
        return_cap -= proposed_cents;

        lines.push(LinePlan {
            line_id: line.line_id,
            authorized_quantity: line.authorized_qty,
            accepted_scan_quantity: accepted,
            review_scan_quantity: 0,
            prior_successful_refund_cents: line.prior_successful_refund_cents,
            maximum_refundable_cents,
            proposed_quantity,
            proposed_cents,
            provenance_event_ids: provenance.into_iter().collect(),
        });
    }

    for (sku, chunks) in available {
        if authorized_skus.contains(&sku) {
            let excess = chunks
                .iter()
                .map(|(_, quantity, _)| u64::from(*quantity))
                .sum::<u64>();
            if excess > 0 {
                work.review_codes.insert("excess_quantity".to_string());
                if let Some(line) = lines
                    .iter_mut()
                    .rev()
                    .find(|line| line_skus.get(&line.line_id) == Some(&sku))
                {
                    line.review_scan_quantity += excess;
                }
            }
        }
    }

    let review_codes: Vec<String> = work.review_codes.into_iter().collect();
    let proposed_any = lines.iter().any(|line| line.proposed_cents > 0);
    let disposition = if review_codes.is_empty() {
        if proposed_any {
            "proposed"
        } else {
            "no_refund"
        }
    } else {
        "review"
    };
    Ok(ReturnPlan {
        return_id: authorization.return_id,
        disposition: disposition.to_string(),
        review_codes,
        unmatched_scan_quantity,
        lines,
    })
}

fn amount_for_first_units(net: u64, authorized_quantity: u32, quantity: u32) -> u64 {
    if authorized_quantity == 0 {
        return 0;
    }
    let divisor = u64::from(authorized_quantity);
    let base = net / divisor;
    let remainder = net % divisor;
    base * u64::from(quantity) + remainder.min(u64::from(quantity))
}

fn validate_snapshot(snapshot: &Snapshot) -> Result<(), String> {
    if snapshot.snapshot_id.is_empty() {
        return Err("snapshot_id must not be empty".to_string());
    }
    let mut return_ids = BTreeSet::new();
    let mut line_ids = BTreeSet::new();
    let mut line_owners = BTreeMap::new();
    for authorization in &snapshot.returns {
        if !return_ids.insert(authorization.return_id.as_str()) {
            return Err(format!("duplicate return_id {}", authorization.return_id));
        }
        if authorization.lines.len() > 12 {
            return Err(format!(
                "return {} exceeds 12 lines",
                authorization.return_id
            ));
        }
        let mut prior_total = 0_u64;
        for line in &authorization.lines {
            if !line_ids.insert(line.line_id.as_str()) {
                return Err(format!("duplicate line_id {}", line.line_id));
            }
            line_owners.insert(line.line_id.as_str(), authorization.return_id.as_str());
            if line.authorized_qty == 0 {
                return Err(format!(
                    "line {} has zero authorized quantity",
                    line.line_id
                ));
            }
            let net = line
                .paid_subtotal_cents
                .checked_add(line.tax_cents)
                .and_then(|gross| gross.checked_sub(line.discount_cents))
                .ok_or_else(|| format!("invalid monetary values for line {}", line.line_id))?;
            if line.prior_successful_refund_cents > net {
                return Err(format!(
                    "prior refund exceeds line value for {}",
                    line.line_id
                ));
            }
            prior_total = prior_total
                .checked_add(line.prior_successful_refund_cents)
                .ok_or_else(|| "prior refund total overflow".to_string())?;
        }
        if prior_total > authorization.captured_cents {
            return Err(format!(
                "prior refunds exceed captured payment for {}",
                authorization.return_id
            ));
        }
    }
    let known_returns: BTreeSet<&str> = snapshot
        .returns
        .iter()
        .map(|authorization| authorization.return_id.as_str())
        .collect();
    let mut parcels: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for event in &snapshot.events {
        if !known_returns.contains(event.return_id()) {
            return Err(format!(
                "event {} references unknown return {}",
                event.event_id(),
                event.return_id()
            ));
        }
        if let Event::WarehouseScan {
            return_id,
            parcel_id,
            ..
        }
        | Event::Carrier {
            return_id,
            parcel_id,
            ..
        } = event
        {
            parcels
                .entry(return_id)
                .or_default()
                .insert(parcel_id.as_str());
        }
        validate_payment_line(event, &line_owners)?;
    }
    if let Some((return_id, _)) = parcels.iter().find(|(_, parcel_ids)| parcel_ids.len() > 4) {
        return Err(format!("return {return_id} exceeds 4 parcels"));
    }
    Ok(())
}

fn validate_payment_line(event: &Event, line_owners: &BTreeMap<&str, &str>) -> Result<(), String> {
    let Event::PaymentResult {
        return_id, line_id, ..
    } = event
    else {
        return Ok(());
    };
    match line_owners.get(line_id.as_str()) {
        None => Err(format!(
            "payment result references line {line_id} absent from authorizations"
        )),
        Some(owner) if *owner != return_id.as_str() => Err(format!(
            "payment result references line {line_id} owned by return {owner}, not {return_id}"
        )),
        Some(_) => Ok(()),
    }
}

/// Read a snapshot, reconcile it, verify its invariants, and atomically publish it.
///
/// # Errors
///
/// Returns an error for unsafe path aliasing, invalid input, failed verification,
/// I/O failures, or a requested fault injection point.
pub fn reconcile_file(input: &Path, output: &Path, failpoint: FailPoint) -> Result<Report, String> {
    if paths_alias(input, output)? {
        return Err("output path aliases input".to_string());
    }
    if failpoint == FailPoint::ExitBeforeOutputCreation {
        std::process::exit(86);
    }
    if failpoint == FailPoint::BeforeOutputCreation {
        return Err("injected failure before output creation".to_string());
    }
    let input_file = File::open(input).map_err(|error| error.to_string())?;
    let snapshot: Snapshot =
        serde_json::from_reader(BufReader::new(input_file)).map_err(|error| error.to_string())?;
    let report = reconcile(&snapshot)?;
    verify_report(&snapshot, &report)?;

    let temporary = temporary_path(output);
    let publication = (|| -> Result<(), String> {
        let temporary_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let mut writer = BufWriter::new(temporary_file);
        serde_json::to_writer(&mut writer, &report).map_err(|error| error.to_string())?;
        writer.write_all(b"\n").map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| error.to_string())?;
        if failpoint == FailPoint::ExitAfterTemporaryComplete {
            std::process::exit(86);
        }
        if failpoint == FailPoint::AfterTemporaryComplete {
            return Err("injected failure after temporary output is complete".to_string());
        }
        if failpoint == FailPoint::BeforePublish {
            return Err("injected failure immediately before publication".to_string());
        }
        if failpoint == FailPoint::ExitBeforePublish {
            std::process::exit(86);
        }
        fs::rename(&temporary, output).map_err(|error| error.to_string())?;
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if publication.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    publication?;
    Ok(report)
}

/// Independently check report identity, advisory marking, ordering, totals, and caps.
///
/// # Errors
///
/// Returns an error when any report invariant differs from the snapshot.
pub fn verify_report(snapshot: &Snapshot, report: &Report) -> Result<(), String> {
    if report.schema_version != SCHEMA_VERSION
        || report.snapshot_id != snapshot.snapshot_id
        || !report.advisory_only
    {
        return Err("report identity or advisory marker is invalid".to_string());
    }
    if !report
        .returns
        .windows(2)
        .all(|window| window[0].return_id < window[1].return_id)
    {
        return Err("return plans are not in canonical order".to_string());
    }
    let authorization_by_id: BTreeMap<&str, &ReturnAuthorization> = snapshot
        .returns
        .iter()
        .map(|authorization| (authorization.return_id.as_str(), authorization))
        .collect();
    let mut total_quantity = 0_u64;
    let mut total_cents = 0_u64;
    for plan in &report.returns {
        let authorization = authorization_by_id
            .get(plan.return_id.as_str())
            .ok_or_else(|| format!("unknown return plan {}", plan.return_id))?;
        let prior_total: u64 = authorization
            .lines
            .iter()
            .map(|line| line.prior_successful_refund_cents)
            .sum();
        let return_proposed = plan
            .lines
            .iter()
            .try_fold(0_u64, |total, line| total.checked_add(line.proposed_cents))
            .ok_or_else(|| format!("return proposed cents overflow for {}", plan.return_id))?;
        if return_proposed > authorization.captured_cents.saturating_sub(prior_total) {
            return Err(format!("return cap exceeded for {}", plan.return_id));
        }
        for line in &plan.lines {
            let authorized_line = authorization
                .lines
                .iter()
                .find(|authorized| authorized.line_id == line.line_id)
                .ok_or_else(|| format!("unknown line plan {}", line.line_id))?;
            let net = authorized_line
                .paid_subtotal_cents
                .checked_add(authorized_line.tax_cents)
                .and_then(|gross| gross.checked_sub(authorized_line.discount_cents))
                .ok_or_else(|| format!("invalid monetary values for line {}", line.line_id))?;
            if line.accepted_scan_quantity > line.authorized_quantity
                || line.proposed_quantity > line.accepted_scan_quantity
                || line.proposed_cents > line.maximum_refundable_cents
            {
                return Err(format!("line cap exceeded for {}", line.line_id));
            }
            if line.proposed_cents
                != amount_for_first_units(
                    net,
                    authorized_line.authorized_qty,
                    line.proposed_quantity,
                )
            {
                return Err(format!(
                    "quantity and cents are incoherent for {}",
                    line.line_id
                ));
            }
            total_quantity += u64::from(line.proposed_quantity);
            total_cents = total_cents
                .checked_add(line.proposed_cents)
                .ok_or_else(|| "report proposed cents overflow".to_string())?;
        }
    }
    if total_quantity != report.summary.proposed_quantity
        || total_cents != report.summary.proposed_cents
        || report.summary.input_event_count != snapshot.events.len()
        || report.summary.return_count != snapshot.returns.len()
    {
        return Err("summary totals do not match report body".to_string());
    }
    let expected = reference_reconcile(snapshot)?;
    if report != &expected {
        return Err("report differs from independent reference calculation".to_string());
    }
    Ok(())
}

fn temporary_path(output: &Path) -> PathBuf {
    let file_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("report");
    output.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()))
}

fn paths_alias(input: &Path, output: &Path) -> Result<bool, String> {
    if input == output {
        return Ok(true);
    }
    let input_metadata = fs::metadata(input).map_err(|error| error.to_string())?;
    let Ok(output_metadata) = fs::metadata(output) else {
        return Ok(false);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(input_metadata.dev() == output_metadata.dev()
            && input_metadata.ino() == output_metadata.ino())
    }
    #[cfg(not(unix))]
    {
        Ok(fs::canonicalize(input).map_err(|error| error.to_string())?
            == fs::canonicalize(output).map_err(|error| error.to_string())?)
    }
}
