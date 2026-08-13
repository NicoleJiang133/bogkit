//! A chronological oracle intentionally separate from the candidate's
//! sort-and-slice reducer. This favors obvious scans over candidate efficiency.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    BilledInterval, Export, Installation, PriorAdjustment, Reading, ReadingKind, Repair,
    RepairPlan, ReviewCase, ServiceError, canonicalize,
};

pub(super) fn evaluate(
    export: &Export,
    adjustments: &[&PriorAdjustment],
) -> Result<RepairPlan, String> {
    let unique = unique_sources(export)?;
    let services = service_ids(export, &unique, adjustments);
    let mut plan = RepairPlan::default();
    for service in services {
        match evaluate_service(export, &unique, adjustments, service) {
            Ok(mut repairs) => plan.repairs.append(&mut repairs),
            Err(error) => plan.review_cases.push(ReviewCase {
                service_point_id: service,
                reason_code: error.code.to_string(),
                detail_ids: error.ids,
            }),
        }
    }
    canonicalize(&mut plan);
    Ok(plan)
}

fn unique_sources(export: &Export) -> Result<BTreeMap<u64, &Reading>, String> {
    let mut unique = BTreeMap::new();
    for reading in &export.readings {
        match unique.get(&reading.source_id) {
            Some(previous) if *previous != reading => {
                return Err(format!("CONFLICTING_SOURCE_ID:{}", reading.source_id));
            }
            Some(_) => {}
            None => {
                unique.insert(reading.source_id, reading);
            }
        }
    }
    Ok(unique)
}

fn service_ids(
    export: &Export,
    unique: &BTreeMap<u64, &Reading>,
    adjustments: &[&PriorAdjustment],
) -> BTreeSet<u32> {
    let mut services = BTreeSet::new();
    services.extend(
        export
            .installations
            .iter()
            .map(|item| item.service_point_id),
    );
    services.extend(unique.values().map(|item| item.service_point_id));
    services.extend(
        export
            .billed_intervals
            .iter()
            .map(|item| item.service_point_id),
    );
    services.extend(adjustments.iter().map(|item| item.service_point_id));
    services
}

fn evaluate_service(
    export: &Export,
    unique: &BTreeMap<u64, &Reading>,
    adjustments: &[&PriorAdjustment],
    service: u32,
) -> Result<Vec<Repair>, ServiceError> {
    let mut installations: Vec<_> = export
        .installations
        .iter()
        .filter(|item| item.service_point_id == service)
        .collect();
    installations.sort_unstable_by_key(|item| (item.installed_at, item.installation_id));
    validate_installations(&installations)?;
    let mut readings: Vec<_> = unique
        .values()
        .copied()
        .filter(|item| item.service_point_id == service)
        .collect();
    readings.sort_unstable_by_key(|item| (item.at, item.meter_id, item.source_id));
    let active = active_readings(&installations, &readings)?;
    let mut bills: Vec<_> = export
        .billed_intervals
        .iter()
        .filter(|item| item.service_point_id == service)
        .collect();
    bills.sort_unstable_by_key(|item| (item.start_at, item.end_at, item.interval_id));
    validate_bills(&bills)?;
    build_repairs(adjustments, service, &installations, &active, &bills)
}

fn validate_installations(installations: &[&Installation]) -> Result<(), ServiceError> {
    if installations.is_empty() {
        return Err(ServiceError::new("MISSING_INSTALLATION", Vec::new()));
    }
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
    validate_installation_sequence(installations)
}

fn validate_installation_sequence(installations: &[&Installation]) -> Result<(), ServiceError> {
    for pair in installations.windows(2) {
        let ids = vec![pair[0].installation_id, pair[1].installation_id];
        match pair[0].removed_at {
            None => return Err(ServiceError::new("OVERLAPPING_INSTALLATIONS", ids)),
            Some(removed) if removed < pair[1].installed_at => {
                return Err(ServiceError::new("INSTALLATION_GAP_NO_POLICY", ids));
            }
            Some(removed) if removed > pair[1].installed_at => {
                return Err(ServiceError::new("OVERLAPPING_INSTALLATIONS", ids));
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

fn active_readings<'a>(
    installations: &[&Installation],
    readings: &[&'a Reading],
) -> Result<Vec<&'a Reading>, ServiceError> {
    let by_source: BTreeMap<_, _> = readings
        .iter()
        .map(|reading| (reading.source_id, *reading))
        .collect();
    let mut superseded = BTreeSet::new();
    for reading in readings {
        validate_reading(installations, &by_source, reading, &mut superseded)?;
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

fn validate_reading(
    installations: &[&Installation],
    by_source: &BTreeMap<u64, &Reading>,
    reading: &Reading,
    superseded: &mut BTreeSet<u64>,
) -> Result<(), ServiceError> {
    let installation = installations.iter().find(|installation| {
        installation.meter_id == reading.meter_id
            && reading.at >= installation.installed_at
            && installation
                .removed_at
                .is_none_or(|removed| reading.at <= removed)
    });
    let Some(installation) = installation else {
        return Err(ServiceError::new(
            "READING_FOR_INACTIVE_METER",
            vec![reading.source_id],
        ));
    };
    let modulus = 10_u64.pow(u32::from(installation.register_width));
    if u64::from(reading.value_litres) >= modulus {
        return Err(ServiceError::new(
            "REGISTER_VALUE_OUT_OF_RANGE",
            vec![reading.source_id],
        ));
    }
    validate_supersession(by_source, reading, superseded)
}

fn validate_supersession(
    by_source: &BTreeMap<u64, &Reading>,
    reading: &Reading,
    superseded: &mut BTreeSet<u64>,
) -> Result<(), ServiceError> {
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
    let Some(target_id) = reading.supersedes else {
        return Ok(());
    };
    let target = by_source.get(&target_id).ok_or_else(|| {
        ServiceError::new(
            "SUPERSESSION_TARGET_MISSING",
            vec![reading.source_id, target_id],
        )
    })?;
    if target.service_point_id != reading.service_point_id
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
    Ok(())
}

fn build_repairs(
    adjustments: &[&PriorAdjustment],
    service: u32,
    installations: &[&Installation],
    active: &[&Reading],
    bills: &[&BilledInterval],
) -> Result<Vec<Repair>, ServiceError> {
    let mut repairs = Vec::new();
    for bill in bills {
        let computed = chronological_usage(bill, installations, active)?;
        let old = adjusted_old_usage(adjustments, service, bill)?;
        if old == computed.litres {
            continue;
        }
        let adjustment = computed
            .litres
            .checked_sub(old)
            .ok_or_else(|| ServiceError::new("INTEGER_OVERFLOW", vec![bill.interval_id]))?;
        let reason = reason_code(&computed).to_string();
        repairs.push(Repair {
            service_point_id: service,
            interval_id: bill.interval_id,
            old_usage_litres: old,
            recomputed_usage_litres: computed.litres,
            adjustment_litres: adjustment,
            source_reading_ids: computed.source_ids,
            installation_ids: computed.installation_ids.clone(),
            installation_meter_ids: computed.meter_ids,
            reason_code: reason,
        });
    }
    Ok(repairs)
}

fn adjusted_old_usage(
    adjustments: &[&PriorAdjustment],
    service: u32,
    bill: &BilledInterval,
) -> Result<i64, ServiceError> {
    let mut total = bill.issued_usage_litres;
    for adjustment in adjustments.iter().filter(|adjustment| {
        adjustment.service_point_id == service && adjustment.interval_id == bill.interval_id
    }) {
        total = total
            .checked_add(adjustment.adjustment_litres)
            .ok_or_else(|| ServiceError::new("INTEGER_OVERFLOW", vec![adjustment.adjustment_id]))?;
    }
    Ok(total)
}

struct ReferenceUsage {
    litres: i64,
    source_ids: Vec<u64>,
    installation_ids: Vec<u64>,
    meter_ids: Vec<u32>,
    rollover: bool,
    correction: bool,
}

fn chronological_usage(
    bill: &BilledInterval,
    installations: &[&Installation],
    active: &[&Reading],
) -> Result<ReferenceUsage, ServiceError> {
    let mut result = ReferenceUsage {
        litres: 0,
        source_ids: Vec::new(),
        installation_ids: Vec::new(),
        meter_ids: Vec::new(),
        rollover: false,
        correction: false,
    };
    let mut cursor = bill.start_at;
    for installation in installations {
        let start = bill.start_at.max(installation.installed_at);
        let end = bill.end_at.min(installation.removed_at.unwrap_or(i64::MAX));
        if start >= end {
            continue;
        }
        if cursor != start {
            return Err(ServiceError::new(
                "BILLING_INTERVAL_NOT_COVERED",
                vec![bill.interval_id],
            ));
        }
        let mut segment: Vec<_> = active
            .iter()
            .copied()
            .filter(|reading| {
                reading.meter_id == installation.meter_id && (start..=end).contains(&reading.at)
            })
            .collect();
        segment.sort_unstable_by_key(|reading| (reading.at, reading.source_id));
        add_segment(&mut result, bill, installation, &segment, start, end)?;
        cursor = end;
    }
    if cursor != bill.end_at {
        return Err(ServiceError::new(
            "BILLING_INTERVAL_NOT_COVERED",
            vec![bill.interval_id],
        ));
    }
    result.source_ids.sort_unstable();
    result.source_ids.dedup();
    result.installation_ids.sort_unstable();
    result.installation_ids.dedup();
    result.meter_ids.sort_unstable();
    result.meter_ids.dedup();
    Ok(result)
}

fn add_segment(
    result: &mut ReferenceUsage,
    bill: &BilledInterval,
    installation: &Installation,
    segment: &[&Reading],
    start: i64,
    end: i64,
) -> Result<(), ServiceError> {
    if segment.first().is_none_or(|reading| reading.at != start)
        || segment.last().is_none_or(|reading| reading.at != end)
    {
        return Err(ServiceError::new(
            "MISSING_BOUNDARY_READING",
            vec![bill.interval_id, installation.installation_id],
        ));
    }
    for pair in segment.windows(2) {
        let (delta, rollover) = chronological_delta(pair[0], pair[1], installation)?;
        result.litres = result
            .litres
            .checked_add(delta)
            .ok_or_else(|| ServiceError::new("INTEGER_OVERFLOW", vec![bill.interval_id]))?;
        result.rollover |= rollover;
    }
    result
        .source_ids
        .extend(segment.iter().map(|reading| reading.source_id));
    result.correction |= segment.iter().any(|reading| reading.supersedes.is_some());
    result.installation_ids.push(installation.installation_id);
    result.meter_ids.push(installation.meter_id);
    Ok(())
}

fn chronological_delta(
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
    let modulus = 10_u64.pow(u32::from(installation.register_width));
    let previous_value = u64::from(previous.value_litres);
    let current_value = u64::from(current.value_litres);
    let regression = previous_value - current_value;
    let looks_like_rollover = regression > modulus / 2
        && previous_value >= modulus * 3 / 4
        && current_value <= modulus / 4;
    if !looks_like_rollover {
        return Err(ServiceError::new(
            "AMBIGUOUS_REGISTER_REGRESSION",
            vec![previous.source_id, current.source_id],
        ));
    }
    let litres = i64::try_from(modulus - previous_value + current_value).map_err(|_| {
        ServiceError::new(
            "INTEGER_OVERFLOW",
            vec![previous.source_id, current.source_id],
        )
    })?;
    Ok((litres, true))
}

fn reason_code(usage: &ReferenceUsage) -> &'static str {
    if usage.correction {
        "CORRECTION_REPAIR"
    } else if usage.installation_ids.len() > 1 {
        "METER_REPLACEMENT_REPAIR"
    } else if usage.rollover {
        "ROLLOVER_REPAIR"
    } else {
        "RECOMPUTED_USAGE"
    }
}
