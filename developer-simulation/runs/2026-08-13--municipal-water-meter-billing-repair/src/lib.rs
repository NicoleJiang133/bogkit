use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod reference;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installation {
    pub service_point_id: u32,
    pub installation_id: u64,
    pub meter_id: u32,
    pub installed_at: i64,
    pub removed_at: Option<i64>,
    pub register_width: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingKind {
    Actual,
    Estimated,
    CorrectedActual,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    pub service_point_id: u32,
    pub meter_id: u32,
    pub source_id: u64,
    pub at: i64,
    pub value_litres: u32,
    pub kind: ReadingKind,
    pub supersedes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BilledInterval {
    pub service_point_id: u32,
    pub interval_id: u64,
    pub start_at: i64,
    pub end_at: i64,
    pub issued_usage_litres: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorAdjustment {
    pub service_point_id: u32,
    pub interval_id: u64,
    pub adjustment_id: u64,
    pub adjustment_litres: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Export {
    pub installations: Vec<Installation>,
    pub readings: Vec<Reading>,
    pub billed_intervals: Vec<BilledInterval>,
    pub prior_adjustments: Vec<PriorAdjustment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repair {
    pub service_point_id: u32,
    pub interval_id: u64,
    pub old_usage_litres: i64,
    pub recomputed_usage_litres: i64,
    pub adjustment_litres: i64,
    pub source_reading_ids: Vec<u64>,
    pub installation_ids: Vec<u64>,
    pub installation_meter_ids: Vec<u32>,
    pub reason_code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCase {
    pub service_point_id: u32,
    pub reason_code: String,
    pub detail_ids: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairPlan {
    pub schema_version: u8,
    pub repairs: Vec<Repair>,
    pub review_cases: Vec<ReviewCase>,
}

impl Default for RepairPlan {
    fn default() -> Self {
        Self {
            schema_version: 1,
            repairs: Vec::new(),
            review_cases: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationFailure {
    None,
    BeforeTemporary,
    BeforeFinalRename,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationCrash {
    None,
    BeforeStaging,
    BeforeFinalRename,
}

#[derive(Debug)]
struct ServiceError {
    code: &'static str,
    ids: Vec<u64>,
}

impl ServiceError {
    fn new(code: &'static str, mut ids: Vec<u64>) -> Self {
        ids.sort_unstable();
        ids.dedup();
        Self { code, ids }
    }
}

struct ComputedUsage {
    litres: i64,
    source_ids: Vec<u64>,
    installation_ids: Vec<u64>,
    meter_ids: Vec<u32>,
    saw_rollover: bool,
    saw_correction: bool,
}

/// Evaluate an export with the candidate's sort-then-reduce implementation.
///
/// `batch_size` controls only how the immutable input is traversed. The
/// canonical sort makes results independent of both batch boundaries and input
/// order.
///
/// # Errors
///
/// Returns an error for an empty batch size, a globally conflicting source ID,
/// or conflicting global adjustment-ID reuse. Exact repeated adjustment rows
/// are treated as idempotent delivery retries. Service-local ambiguity is
/// represented as a review case instead.
pub fn evaluate_candidate(export: &Export, batch_size: usize) -> Result<RepairPlan, String> {
    if batch_size == 0 {
        return Err("batch size must be greater than zero".to_string());
    }
    let adjustments = normalize_adjustments(export)?;
    let prepared = prepare_candidate(export, batch_size, adjustments)?;
    Ok(reduce_candidate(prepared))
}

fn normalize_adjustments(export: &Export) -> Result<Vec<&PriorAdjustment>, String> {
    let mut adjustments: Vec<_> = export.prior_adjustments.iter().collect();
    adjustments.sort_unstable_by_key(|adjustment| adjustment.adjustment_id);
    let mut unique: Vec<&PriorAdjustment> = Vec::with_capacity(adjustments.len());
    for adjustment in adjustments {
        if let Some(previous) = unique.last()
            && previous.adjustment_id == adjustment.adjustment_id
        {
            if *previous != adjustment {
                return Err(format!(
                    "CONFLICTING_ADJUSTMENT_ID:{}",
                    adjustment.adjustment_id
                ));
            }
            continue;
        }
        unique.push(adjustment);
    }
    Ok(unique)
}

struct CandidateInputs<'a> {
    readings: Vec<&'a Reading>,
    installations: Vec<&'a Installation>,
    bills: Vec<&'a BilledInterval>,
    adjustments: Vec<&'a PriorAdjustment>,
    service_ids: Vec<u32>,
}

fn prepare_candidate<'a>(
    export: &'a Export,
    batch_size: usize,
    mut adjustments: Vec<&'a PriorAdjustment>,
) -> Result<CandidateInputs<'a>, String> {
    let mut readings = Vec::with_capacity(export.readings.len());
    for batch in export.readings.chunks(batch_size) {
        readings.extend(batch);
    }
    readings.sort_unstable_by_key(|reading| reading.source_id);
    let mut unique_readings: Vec<&Reading> = Vec::with_capacity(readings.len());
    for reading in readings {
        if let Some(previous) = unique_readings.last()
            && previous.source_id == reading.source_id
        {
            if *previous != reading {
                return Err(format!("CONFLICTING_SOURCE_ID:{}", reading.source_id));
            }
            continue;
        }
        unique_readings.push(reading);
    }
    unique_readings.sort_unstable_by_key(|reading| {
        (
            reading.service_point_id,
            reading.meter_id,
            reading.at,
            reading.source_id,
        )
    });

    let mut installations: Vec<_> = export.installations.iter().collect();
    installations.sort_unstable_by_key(|installation| {
        (
            installation.service_point_id,
            installation.installed_at,
            installation.meter_id,
            installation.installation_id,
        )
    });
    let mut bills: Vec<_> = export.billed_intervals.iter().collect();
    bills.sort_unstable_by_key(|bill| {
        (
            bill.service_point_id,
            bill.start_at,
            bill.end_at,
            bill.interval_id,
        )
    });
    adjustments.sort_unstable_by_key(|adjustment| {
        (
            adjustment.service_point_id,
            adjustment.interval_id,
            adjustment.adjustment_id,
        )
    });

    let mut service_ids = Vec::with_capacity(
        unique_readings.len() + installations.len() + bills.len() + adjustments.len(),
    );
    service_ids.extend(
        unique_readings
            .iter()
            .map(|reading| reading.service_point_id),
    );
    service_ids.extend(
        installations
            .iter()
            .map(|installation| installation.service_point_id),
    );
    service_ids.extend(bills.iter().map(|bill| bill.service_point_id));
    service_ids.extend(
        adjustments
            .iter()
            .map(|adjustment| adjustment.service_point_id),
    );
    service_ids.sort_unstable();
    service_ids.dedup();

    Ok(CandidateInputs {
        readings: unique_readings,
        installations,
        bills,
        adjustments,
        service_ids,
    })
}

fn reduce_candidate(prepared: CandidateInputs<'_>) -> RepairPlan {
    let mut plan = RepairPlan::default();
    let mut reading_offset = 0;
    let mut installation_offset = 0;
    let mut bill_offset = 0;
    let mut adjustment_offset = 0;

    for service_point_id in prepared.service_ids {
        let reading_end =
            advance_service(&prepared.readings, reading_offset, service_point_id, |r| {
                r.service_point_id
            });
        let installation_end = advance_service(
            &prepared.installations,
            installation_offset,
            service_point_id,
            |i| i.service_point_id,
        );
        let bill_end = advance_service(&prepared.bills, bill_offset, service_point_id, |b| {
            b.service_point_id
        });
        let adjustment_end = advance_service(
            &prepared.adjustments,
            adjustment_offset,
            service_point_id,
            |a| a.service_point_id,
        );

        match evaluate_service(
            service_point_id,
            &prepared.installations[installation_offset..installation_end],
            &prepared.readings[reading_offset..reading_end],
            &prepared.bills[bill_offset..bill_end],
            &prepared.adjustments[adjustment_offset..adjustment_end],
        ) {
            Ok(mut repairs) => plan.repairs.append(&mut repairs),
            Err(error) => plan.review_cases.push(ReviewCase {
                service_point_id,
                reason_code: error.code.to_string(),
                detail_ids: error.ids,
            }),
        }

        reading_offset = reading_end;
        installation_offset = installation_end;
        bill_offset = bill_end;
        adjustment_offset = adjustment_end;
    }

    canonicalize(&mut plan);
    plan
}

fn advance_service<T, F>(items: &[T], start: usize, service_id: u32, service: F) -> usize
where
    F: Fn(&T) -> u32,
{
    let mut end = start;
    while end < items.len() && service(&items[end]) == service_id {
        end += 1;
    }
    end
}

fn evaluate_service(
    service_id: u32,
    installations: &[&Installation],
    readings: &[&Reading],
    bills: &[&BilledInterval],
    adjustments: &[&PriorAdjustment],
) -> Result<Vec<Repair>, ServiceError> {
    if installations.is_empty() {
        return Err(ServiceError::new("MISSING_INSTALLATION", Vec::new()));
    }
    validate_installations(installations)?;
    validate_bills(bills)?;

    let active = resolve_active_readings(service_id, installations, readings)?;
    let adjustment_totals = adjustment_totals(adjustments)?;
    let mut repairs = Vec::new();

    for bill in bills {
        let usage = compute_usage(bill, installations, &active)?;
        let prior = adjustment_totals
            .get(&bill.interval_id)
            .copied()
            .unwrap_or(0);
        let old_usage = bill
            .issued_usage_litres
            .checked_add(prior)
            .ok_or_else(|| ServiceError::new("INTEGER_OVERFLOW", vec![bill.interval_id]))?;
        if old_usage == usage.litres {
            continue;
        }
        let adjustment = usage
            .litres
            .checked_sub(old_usage)
            .ok_or_else(|| ServiceError::new("INTEGER_OVERFLOW", vec![bill.interval_id]))?;
        let reason = if usage.saw_correction {
            "CORRECTION_REPAIR"
        } else if usage.installation_ids.len() > 1 {
            "METER_REPLACEMENT_REPAIR"
        } else if usage.saw_rollover {
            "ROLLOVER_REPAIR"
        } else {
            "RECOMPUTED_USAGE"
        };
        repairs.push(Repair {
            service_point_id: service_id,
            interval_id: bill.interval_id,
            old_usage_litres: old_usage,
            recomputed_usage_litres: usage.litres,
            adjustment_litres: adjustment,
            source_reading_ids: usage.source_ids,
            installation_ids: usage.installation_ids,
            installation_meter_ids: usage.meter_ids,
            reason_code: reason.to_string(),
        });
    }
    Ok(repairs)
}

fn validate_installations(installations: &[&Installation]) -> Result<(), ServiceError> {
    let mut installation_ids = BTreeSet::new();
    let mut meter_ids = BTreeSet::new();
    for installation in installations {
        if !installation_ids.insert(installation.installation_id) {
            return Err(ServiceError::new(
                "DUPLICATE_INSTALLATION_ID",
                vec![installation.installation_id],
            ));
        }
        if !meter_ids.insert(installation.meter_id) {
            return Err(ServiceError::new(
                "METER_REINSTALLED_UNSUPPORTED",
                vec![u64::from(installation.meter_id)],
            ));
        }
        if !matches!(installation.register_width, 6 | 8) {
            return Err(ServiceError::new(
                "MALFORMED_REGISTER_WIDTH",
                vec![installation.installation_id],
            ));
        }
        if installation
            .removed_at
            .is_some_and(|removed| removed <= installation.installed_at)
        {
            return Err(ServiceError::new(
                "INVALID_INSTALLATION_RANGE",
                vec![installation.installation_id],
            ));
        }
    }
    for pair in installations.windows(2) {
        let left = pair[0];
        let right = pair[1];
        match left.removed_at {
            None => {
                return Err(ServiceError::new(
                    "OVERLAPPING_INSTALLATIONS",
                    vec![left.installation_id, right.installation_id],
                ));
            }
            Some(removed) if removed < right.installed_at => {
                return Err(ServiceError::new(
                    "INSTALLATION_GAP_NO_POLICY",
                    vec![left.installation_id, right.installation_id],
                ));
            }
            Some(removed) if removed > right.installed_at => {
                return Err(ServiceError::new(
                    "OVERLAPPING_INSTALLATIONS",
                    vec![left.installation_id, right.installation_id],
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

fn validate_bills(bills: &[&BilledInterval]) -> Result<(), ServiceError> {
    let mut ids = BTreeSet::new();
    for bill in bills {
        if !ids.insert(bill.interval_id) {
            return Err(ServiceError::new(
                "DUPLICATE_INTERVAL_ID",
                vec![bill.interval_id],
            ));
        }
        if bill.end_at <= bill.start_at || bill.issued_usage_litres < 0 {
            return Err(ServiceError::new(
                "INVALID_BILLED_INTERVAL",
                vec![bill.interval_id],
            ));
        }
    }
    Ok(())
}

fn resolve_active_readings<'a>(
    service_id: u32,
    installations: &[&Installation],
    readings: &[&'a Reading],
) -> Result<Vec<&'a Reading>, ServiceError> {
    let by_source: BTreeMap<_, _> = readings
        .iter()
        .map(|reading| (reading.source_id, *reading))
        .collect();
    let mut superseded = BTreeSet::new();
    for reading in readings {
        let matching: Vec<_> = installations
            .iter()
            .filter(|installation| {
                installation.meter_id == reading.meter_id
                    && reading.at >= installation.installed_at
                    && installation
                        .removed_at
                        .is_none_or(|removed| reading.at <= removed)
            })
            .collect();
        if matching.len() != 1 {
            return Err(ServiceError::new(
                "READING_FOR_INACTIVE_METER",
                vec![reading.source_id],
            ));
        }
        let modulus = register_modulus(matching[0].register_width);
        if u64::from(reading.value_litres) >= modulus {
            return Err(ServiceError::new(
                "REGISTER_VALUE_OUT_OF_RANGE",
                vec![reading.source_id],
            ));
        }
        if reading.kind == ReadingKind::Estimated && reading.supersedes.is_some() {
            return Err(ServiceError::new(
                "INVALID_SUPERSESSION_KIND",
                vec![reading.source_id],
            ));
        }
        if reading.kind == ReadingKind::CorrectedActual && reading.supersedes.is_none() {
            return Err(ServiceError::new(
                "CORRECTION_MISSING_TARGET",
                vec![reading.source_id],
            ));
        }
        if let Some(target_id) = reading.supersedes {
            let target = by_source.get(&target_id).ok_or_else(|| {
                ServiceError::new(
                    "SUPERSESSION_TARGET_MISSING",
                    vec![reading.source_id, target_id],
                )
            })?;
            if target.service_point_id != service_id
                || target.meter_id != reading.meter_id
                || target.at != reading.at
                || target.source_id == reading.source_id
            {
                return Err(ServiceError::new(
                    "INVALID_SUPERSESSION_TARGET",
                    vec![reading.source_id, target_id],
                ));
            }
            if !superseded.insert(target_id) {
                return Err(ServiceError::new("MULTIPLE_SUPERSEDERS", vec![target_id]));
            }
        }
    }

    let mut active: Vec<_> = readings
        .iter()
        .copied()
        .filter(|reading| !superseded.contains(&reading.source_id))
        .collect();
    active.sort_unstable_by_key(|reading| (reading.meter_id, reading.at, reading.source_id));
    for pair in active.windows(2) {
        if pair[0].meter_id == pair[1].meter_id && pair[0].at == pair[1].at {
            return Err(ServiceError::new(
                "AMBIGUOUS_EQUAL_TIMESTAMP",
                vec![pair[0].source_id, pair[1].source_id],
            ));
        }
    }
    Ok(active)
}

fn adjustment_totals(adjustments: &[&PriorAdjustment]) -> Result<BTreeMap<u64, i64>, ServiceError> {
    let mut totals = BTreeMap::<u64, i64>::new();
    for adjustment in adjustments {
        let total = totals.entry(adjustment.interval_id).or_default();
        *total = total
            .checked_add(adjustment.adjustment_litres)
            .ok_or_else(|| ServiceError::new("INTEGER_OVERFLOW", vec![adjustment.adjustment_id]))?;
    }
    Ok(totals)
}

fn compute_usage(
    bill: &BilledInterval,
    installations: &[&Installation],
    active: &[&Reading],
) -> Result<ComputedUsage, ServiceError> {
    let mut total = 0_i64;
    let mut sources = Vec::new();
    let mut used_installations = Vec::new();
    let mut meters = Vec::new();
    let mut saw_rollover = false;
    let mut saw_correction = false;
    let mut cursor = bill.start_at;

    for installation in installations {
        let start = bill.start_at.max(installation.installed_at);
        let end = bill.end_at.min(installation.removed_at.unwrap_or(i64::MAX));
        if start >= end {
            continue;
        }
        if start != cursor {
            return Err(ServiceError::new(
                "BILLING_INTERVAL_NOT_COVERED",
                vec![bill.interval_id],
            ));
        }
        cursor = end;
        let segment: Vec<_> = active
            .iter()
            .copied()
            .filter(|reading| {
                reading.meter_id == installation.meter_id
                    && reading.at >= start
                    && reading.at <= end
            })
            .collect();
        if segment.first().is_none_or(|reading| reading.at != start)
            || segment.last().is_none_or(|reading| reading.at != end)
        {
            return Err(ServiceError::new(
                "MISSING_BOUNDARY_READING",
                vec![bill.interval_id, installation.installation_id],
            ));
        }
        for pair in segment.windows(2) {
            let (delta, rollover) = meter_delta(pair[0], pair[1], installation)?;
            total = total
                .checked_add(delta)
                .ok_or_else(|| ServiceError::new("INTEGER_OVERFLOW", vec![bill.interval_id]))?;
            saw_rollover |= rollover;
        }
        sources.extend(segment.iter().map(|reading| reading.source_id));
        saw_correction |= segment.iter().any(|reading| reading.supersedes.is_some());
        used_installations.push(installation.installation_id);
        meters.push(installation.meter_id);
    }
    if cursor != bill.end_at {
        return Err(ServiceError::new(
            "BILLING_INTERVAL_NOT_COVERED",
            vec![bill.interval_id],
        ));
    }
    sources.sort_unstable();
    sources.dedup();
    used_installations.sort_unstable();
    used_installations.dedup();
    meters.sort_unstable();
    meters.dedup();
    Ok(ComputedUsage {
        litres: total,
        source_ids: sources,
        installation_ids: used_installations,
        meter_ids: meters,
        saw_rollover,
        saw_correction,
    })
}

fn meter_delta(
    previous: &Reading,
    current: &Reading,
    installation: &Installation,
) -> Result<(i64, bool), ServiceError> {
    if current.value_litres >= previous.value_litres {
        return Ok((
            i64::from(current.value_litres - previous.value_litres),
            false,
        ));
    }
    let modulus = register_modulus(installation.register_width);
    let previous_value = u64::from(previous.value_litres);
    let current_value = u64::from(current.value_litres);
    let regression = previous_value - current_value;
    if regression > modulus / 2
        && previous_value >= modulus.saturating_mul(3) / 4
        && current_value <= modulus / 4
    {
        let delta = modulus - previous_value + current_value;
        return Ok((
            i64::try_from(delta).map_err(|_| {
                ServiceError::new(
                    "INTEGER_OVERFLOW",
                    vec![previous.source_id, current.source_id],
                )
            })?,
            true,
        ));
    }
    Err(ServiceError::new(
        "AMBIGUOUS_REGISTER_REGRESSION",
        vec![previous.source_id, current.source_id],
    ))
}

const fn register_modulus(width: u8) -> u64 {
    match width {
        6 => 1_000_000,
        8 => 100_000_000,
        _ => 0,
    }
}

fn canonicalize(plan: &mut RepairPlan) {
    plan.repairs.sort_unstable_by_key(|repair| {
        (
            repair.service_point_id,
            repair.interval_id,
            repair.reason_code.clone(),
        )
    });
    plan.review_cases.sort_unstable_by_key(|review| {
        (
            review.service_point_id,
            review.reason_code.clone(),
            review.detail_ids.clone(),
        )
    });
}

/// Serialize a plan in the canonical byte representation used for publication.
///
/// # Errors
///
/// Returns a serialization error if the plan cannot be encoded.
pub fn canonical_plan_bytes(plan: &RepairPlan) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(plan).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Publish a complete canonical plan using a same-directory temporary file and
/// atomic rename.
///
/// # Errors
///
/// Returns an error for input/output identity, injected failure, filesystem
/// failure, or serialization failure. The prior output is preserved on every
/// error before the final rename.
pub fn publish_plan(
    input_path: Option<&Path>,
    output_path: &Path,
    plan: &RepairPlan,
    failure: PublicationFailure,
) -> Result<(), String> {
    publish_plan_internal(
        input_path,
        output_path,
        plan,
        failure,
        PublicationCrash::None,
    )
}

/// Publish a plan while optionally terminating the process at a precise crash
/// boundary. This exists to exercise abrupt lifecycle behavior from a child
/// process; ordinary callers should use [`publish_plan`].
///
/// `BeforeStaging` aborts after path validation and stale-stage cleanup but
/// before creating a temporary file. `BeforeFinalRename` aborts after the
/// complete temporary file has been written and synced, immediately before the
/// atomic rename.
///
/// # Errors
///
/// Returns a path, serialization, or filesystem error before reaching the
/// requested crash point.
pub fn publish_plan_with_crash(
    input_path: Option<&Path>,
    output_path: &Path,
    plan: &RepairPlan,
    crash: PublicationCrash,
) -> Result<(), String> {
    publish_plan_internal(
        input_path,
        output_path,
        plan,
        PublicationFailure::None,
        crash,
    )
}

fn publish_plan_internal(
    input_path: Option<&Path>,
    output_path: &Path,
    plan: &RepairPlan,
    failure: PublicationFailure,
    crash: PublicationCrash,
) -> Result<(), String> {
    if let Some(input) = input_path {
        reject_path_identity(input, output_path)?;
    }
    if failure == PublicationFailure::BeforeTemporary {
        return Err("injected failure before temporary publication".to_string());
    }
    let parent = output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    cleanup_stale_temporaries(output_path)?;
    if crash == PublicationCrash::BeforeStaging {
        std::process::abort();
    }
    let bytes = canonical_plan_bytes(plan)?;
    let (temporary_path, mut temporary) = create_temporary(output_path)?;
    let staged = (|| -> Result<(), String> {
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.sync_all())
            .map_err(|error| error.to_string())?;
        if failure == PublicationFailure::BeforeFinalRename {
            return Err("injected failure before final publication".to_string());
        }
        if crash == PublicationCrash::BeforeFinalRename {
            std::process::abort();
        }
        fs::rename(&temporary_path, output_path).map_err(|error| error.to_string())?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if staged.is_err() {
        drop(temporary);
        let _ignored = fs::remove_file(&temporary_path);
    }
    staged
}

fn cleanup_stale_temporaries(output: &Path) -> Result<(), String> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = output
        .file_name()
        .ok_or_else(|| "output path has no file name".to_string())?
        .to_string_lossy();
    let prefix = format!(".{file_name}.");
    for entry in fs::read_dir(parent).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".tmp") {
            fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn create_temporary(output: &Path) -> Result<(PathBuf, File), String> {
    let file_name = output
        .file_name()
        .ok_or_else(|| "output path has no file name".to_string())?
        .to_string_lossy();
    for attempt in 0_u8..100 {
        let candidate = output.with_file_name(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("could not allocate a unique temporary output".to_string())
}

fn reject_path_identity(input: &Path, output: &Path) -> Result<(), String> {
    let canonical_input = fs::canonicalize(input).map_err(|error| error.to_string())?;
    let canonical_output = if output.exists() {
        fs::canonicalize(output).map_err(|error| error.to_string())?
    } else {
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        match fs::canonicalize(parent) {
            Ok(canonical_parent) => canonical_parent.join(
                output
                    .file_name()
                    .ok_or_else(|| "output path has no file name".to_string())?,
            ),
            Err(_) => std::env::current_dir()
                .map_err(|error| error.to_string())?
                .join(output),
        }
    };
    if canonical_input == canonical_output {
        return Err("INPUT_OUTPUT_PATH_IDENTITY".to_string());
    }
    Ok(())
}

/// A deliberately straightforward chronological oracle. It reconstructs a
/// sorted export one service point at a time and delegates no ordering or
/// publication work to the candidate's global traversal.
///
/// # Errors
///
/// Returns a global source-ID or adjustment-ID conflict, matching the
/// candidate's fail-before-derivation contract. Exact repeated adjustment rows
/// are idempotent. Service-local ambiguity becomes a review case.
pub fn evaluate_reference(export: &Export) -> Result<RepairPlan, String> {
    let adjustments = normalize_adjustments(export)?;
    reference::evaluate(export, &adjustments)
}

/// Build a small deterministic synthetic case for oracle comparison.
#[must_use]
pub fn generated_case(seed: u64, service_points: u32) -> Export {
    let mut export = Export {
        installations: Vec::with_capacity(service_points as usize),
        readings: Vec::with_capacity(service_points as usize * 4),
        billed_intervals: Vec::with_capacity(service_points as usize * 2),
        prior_adjustments: Vec::new(),
    };
    for service in 0..service_points {
        let base_id = seed
            .wrapping_mul(1_000_003)
            .wrapping_add(u64::from(service) * 10);
        let width = if (seed + u64::from(service)).is_multiple_of(2) {
            6
        } else {
            8
        };
        let meter_id = service + 10_000;
        export.installations.push(Installation {
            service_point_id: service,
            installation_id: base_id,
            meter_id,
            installed_at: 0,
            removed_at: None,
            register_width: width,
        });
        let mode = (seed + u64::from(service)) % 4;
        let modulus = register_modulus(width);
        let (first, second, third) = if mode == 1 {
            (u32::try_from(modulus - 5).unwrap_or(0), 3, 11)
        } else if mode == 3 {
            (500, 400, 600)
        } else {
            let start = u32::try_from(100 + seed % 100).unwrap_or(100);
            (start, start + 10, start + 25)
        };
        export.readings.extend([
            Reading {
                service_point_id: service,
                meter_id,
                source_id: base_id + 1,
                at: 0,
                value_litres: first,
                kind: ReadingKind::Actual,
                supersedes: None,
            },
            Reading {
                service_point_id: service,
                meter_id,
                source_id: base_id + 2,
                at: 1,
                value_litres: second,
                kind: if mode == 2 {
                    ReadingKind::Estimated
                } else {
                    ReadingKind::Actual
                },
                supersedes: None,
            },
            Reading {
                service_point_id: service,
                meter_id,
                source_id: base_id + 3,
                at: 2,
                value_litres: third,
                kind: ReadingKind::Actual,
                supersedes: None,
            },
        ]);
        if mode == 2 {
            export.readings.push(Reading {
                service_point_id: service,
                meter_id,
                source_id: base_id + 4,
                at: 1,
                value_litres: second + 2,
                kind: ReadingKind::CorrectedActual,
                supersedes: Some(base_id + 2),
            });
        }
        export.billed_intervals.extend([
            BilledInterval {
                service_point_id: service,
                interval_id: base_id + 5,
                start_at: 0,
                end_at: 1,
                issued_usage_litres: if mode == 1 { 7 } else { 10 },
            },
            BilledInterval {
                service_point_id: service,
                interval_id: base_id + 6,
                start_at: 1,
                end_at: 2,
                issued_usage_litres: 15,
            },
        ]);
    }
    export
}

/// Generate the best-case requested counts used by the release demonstration.
/// Records are pre-sorted; every service point has 20 readings and only two
/// issued intervals; there are no corrections or prior adjustments; the first
/// 20,000 points have a replacement, yielding 120,000 physical meters.
#[must_use]
pub fn generate_representative() -> Export {
    const SERVICES: u32 = 100_000;
    const REPLACEMENTS: u32 = 20_000;
    let mut export = Export {
        installations: Vec::with_capacity((SERVICES + REPLACEMENTS) as usize),
        readings: Vec::with_capacity(SERVICES as usize * 20),
        billed_intervals: Vec::with_capacity(SERVICES as usize * 2),
        prior_adjustments: Vec::new(),
    };
    let mut source_id = 1_u64;
    for service in 0..SERVICES {
        if service < REPLACEMENTS {
            push_replacement_service(&mut export, service, SERVICES, &mut source_id);
        } else {
            push_single_meter_service(&mut export, service, &mut source_id);
        }
    }
    export
}

fn push_replacement_service(export: &mut Export, service: u32, services: u32, source_id: &mut u64) {
    let first_meter = service + 1;
    let second_meter = services + service + 1;
    let install_base = u64::from(service) * 2 + 1;
    export.installations.extend([
        Installation {
            service_point_id: service,
            installation_id: install_base,
            meter_id: first_meter,
            installed_at: 0,
            removed_at: Some(12),
            register_width: 8,
        },
        Installation {
            service_point_id: service,
            installation_id: install_base + 1,
            meter_id: second_meter,
            installed_at: 12,
            removed_at: None,
            register_width: 8,
        },
    ]);
    push_reading_range(export, service, first_meter, 0..9, 0, source_id);
    push_reading(export, service, first_meter, 12, 1_200, source_id);
    push_reading_range(export, service, second_meter, 12..21, 12, source_id);
    push_reading(export, service, second_meter, 24, 1_200, source_id);
    export.billed_intervals.extend([
        BilledInterval {
            service_point_id: service,
            interval_id: u64::from(service) * 2 + 1,
            start_at: 8,
            end_at: 12,
            issued_usage_litres: 400,
        },
        BilledInterval {
            service_point_id: service,
            interval_id: u64::from(service) * 2 + 2,
            start_at: 12,
            end_at: 13,
            issued_usage_litres: 100,
        },
    ]);
}

fn push_single_meter_service(export: &mut Export, service: u32, source_id: &mut u64) {
    let meter = service + 1;
    export.installations.push(Installation {
        service_point_id: service,
        installation_id: u64::from(service) * 2 + 1,
        meter_id: meter,
        installed_at: 0,
        removed_at: None,
        register_width: if service.is_multiple_of(2) { 6 } else { 8 },
    });
    push_reading_range(export, service, meter, 0..19, 0, source_id);
    push_reading(export, service, meter, 24, 2_400, source_id);
    export.billed_intervals.extend([
        BilledInterval {
            service_point_id: service,
            interval_id: u64::from(service) * 2 + 1,
            start_at: 0,
            end_at: 1,
            issued_usage_litres: 100,
        },
        BilledInterval {
            service_point_id: service,
            interval_id: u64::from(service) * 2 + 2,
            start_at: 1,
            end_at: 2,
            issued_usage_litres: 100,
        },
    ]);
}

fn push_reading_range(
    export: &mut Export,
    service: u32,
    meter: u32,
    range: std::ops::Range<i64>,
    origin: i64,
    source_id: &mut u64,
) {
    for at in range {
        let value = u32::try_from((at - origin) * 100).unwrap_or(0);
        push_reading(export, service, meter, at, value, source_id);
    }
}

fn push_reading(
    export: &mut Export,
    service: u32,
    meter: u32,
    at: i64,
    value_litres: u32,
    source_id: &mut u64,
) {
    export.readings.push(Reading {
        service_point_id: service,
        meter_id: meter,
        source_id: *source_id,
        at,
        value_litres,
        kind: ReadingKind::Actual,
        supersedes: None,
    });
    *source_id += 1;
}
