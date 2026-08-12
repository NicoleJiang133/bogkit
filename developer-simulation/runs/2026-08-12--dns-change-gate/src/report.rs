use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::{Limits, RecordKey, ZoneData, ZonePolicy};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum Verdict {
    Allow,
    Review,
    Block,
}

impl Verdict {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Review => "review",
            Self::Block => "block",
        }
    }
}

#[derive(Debug)]
struct Change {
    kind: &'static str,
    key: RecordKey,
    old_ttl: Option<u32>,
    new_ttl: Option<u32>,
}

#[derive(Debug)]
struct Finding {
    code: &'static str,
    verdict: Verdict,
    owner: String,
    record_types: Vec<String>,
    detail: String,
}

#[derive(Debug)]
pub struct ZoneReport {
    zone: String,
    old_baseline: String,
    new_baseline: String,
    verdict: Verdict,
    changes: Vec<Change>,
    findings: Vec<Finding>,
}

#[derive(Debug)]
pub struct Report {
    overall: Verdict,
    zones: Vec<ZoneReport>,
}

impl Report {
    pub fn new(zones: Vec<ZoneReport>) -> Self {
        let overall = zones
            .iter()
            .map(|zone| zone.verdict)
            .max()
            .unwrap_or(Verdict::Allow);
        Self { overall, zones }
    }

    pub const fn is_blocked(&self) -> bool {
        matches!(self.overall, Verdict::Block)
    }

    pub fn to_json(&self) -> String {
        let mut output = String::new();
        write!(
            output,
            "{{\"schema_version\":1,\"overall_verdict\":\"{}\",\"zones\":[",
            self.overall.as_str()
        )
        .expect("write to string");
        for (zone_index, zone) in self.zones.iter().enumerate() {
            if zone_index != 0 {
                output.push(',');
            }
            write!(
                output,
                "{{\"zone\":\"{}\",\"verdict\":\"{}\",\"baseline\":{{\"old\":\"{}\",\"new\":\"{}\"}},\"changes\":[",
                json(&zone.zone),
                zone.verdict.as_str(),
                json(&zone.old_baseline),
                json(&zone.new_baseline)
            )
            .expect("write to string");
            for (change_index, change) in zone.changes.iter().enumerate() {
                if change_index != 0 {
                    output.push(',');
                }
                write!(
                    output,
                    "{{\"kind\":\"{}\",\"owner\":\"{}\",\"type\":\"{}\",\"rdata\":\"{}\",\"old_ttl\":{},\"new_ttl\":{}}}",
                    change.kind,
                    json(&change.key.owner),
                    json(&change.key.record_type),
                    json(&change.key.rdata),
                    optional_number(change.old_ttl),
                    optional_number(change.new_ttl)
                )
                .expect("write to string");
            }
            output.push_str("],\"findings\":[");
            for (finding_index, finding) in zone.findings.iter().enumerate() {
                if finding_index != 0 {
                    output.push(',');
                }
                write!(
                    output,
                    "{{\"code\":\"{}\",\"verdict\":\"{}\",\"owner\":\"{}\",\"record_types\":[",
                    finding.code,
                    finding.verdict.as_str(),
                    json(&finding.owner)
                )
                .expect("write to string");
                for (type_index, record_type) in finding.record_types.iter().enumerate() {
                    if type_index != 0 {
                        output.push(',');
                    }
                    write!(output, "\"{}\"", json(record_type)).expect("write to string");
                }
                write!(output, "],\"detail\":\"{}\"}}", json(&finding.detail))
                    .expect("write to string");
            }
            output.push_str("]}");
        }
        output.push_str("]}\n");
        output
    }
}

pub fn compare_zone(
    selected: &ZonePolicy,
    old: &ZoneData,
    new: &ZoneData,
    limits: &Limits,
) -> ZoneReport {
    let mut changes = Vec::new();
    for (key, ttl) in &old.records {
        match new.records.get(key) {
            None => changes.push(Change {
                kind: "removal",
                key: key.clone(),
                old_ttl: Some(*ttl),
                new_ttl: None,
            }),
            Some(new_ttl) if new_ttl != ttl => changes.push(Change {
                kind: "ttl",
                key: key.clone(),
                old_ttl: Some(*ttl),
                new_ttl: Some(*new_ttl),
            }),
            Some(_) => {}
        }
    }
    for (key, ttl) in &new.records {
        if !old.records.contains_key(key) {
            changes.push(Change {
                kind: "addition",
                key: key.clone(),
                old_ttl: None,
                new_ttl: Some(*ttl),
            });
        }
    }
    changes.sort_by(|left, right| {
        (
            &left.key.owner,
            &left.key.record_type,
            &left.key.rdata,
            left.kind,
        )
            .cmp(&(
                &right.key.owner,
                &right.key.record_type,
                &right.key.rdata,
                right.kind,
            ))
    });

    let mut findings = Vec::new();
    if selected.old_baseline != "PASS" || selected.new_baseline != "PASS" {
        findings.push(Finding {
            code: "BASELINE_FAILURE",
            verdict: Verdict::Block,
            owner: selected.zone.clone(),
            record_types: Vec::new(),
            detail: format!(
                "named-checkzone old={} new={}; a baseline failure cannot be allowed",
                selected.old_baseline, selected.new_baseline
            ),
        });
    }
    check_apex(selected, new, &mut findings);
    check_aliases(new, &mut findings);
    check_ttls(&changes, limits, &mut findings);
    if !changes.is_empty() {
        check_soa(selected, old, new, &mut findings);
    }
    check_delegations(selected, old, new, &mut findings);
    findings.sort_by(|left, right| {
        (
            left.owner.as_str(),
            left.code,
            &left.record_types,
            left.detail.as_str(),
        )
            .cmp(&(
                right.owner.as_str(),
                right.code,
                &right.record_types,
                right.detail.as_str(),
            ))
    });
    let verdict = findings
        .iter()
        .map(|finding| finding.verdict)
        .max()
        .unwrap_or(Verdict::Allow);
    ZoneReport {
        zone: selected.zone.clone(),
        old_baseline: selected.old_baseline.clone(),
        new_baseline: selected.new_baseline.clone(),
        verdict,
        changes,
        findings,
    }
}

fn check_apex(selected: &ZonePolicy, new: &ZoneData, findings: &mut Vec<Finding>) {
    let soa_count = new
        .records
        .keys()
        .filter(|key| key.owner == selected.zone && key.record_type == "SOA")
        .count();
    if soa_count != 1 {
        findings.push(Finding {
            code: "APEX_SOA_COUNT",
            verdict: Verdict::Block,
            owner: selected.zone.clone(),
            record_types: vec!["SOA".to_string()],
            detail: format!("apex requires exactly one canonical SOA record; found {soa_count}"),
        });
    }
    let ns_count = new
        .records
        .keys()
        .filter(|key| key.owner == selected.zone && key.record_type == "NS")
        .count();
    if ns_count == 0 {
        findings.push(Finding {
            code: "APEX_NS_MISSING",
            verdict: Verdict::Block,
            owner: selected.zone.clone(),
            record_types: vec!["NS".to_string()],
            detail: "apex requires at least one NS record".to_string(),
        });
    }
}

fn check_aliases(new: &ZoneData, findings: &mut Vec<Finding>) {
    let mut owners: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for key in new.records.keys() {
        owners
            .entry(&key.owner)
            .or_default()
            .insert(&key.record_type);
    }
    for (owner, types) in owners {
        if types.contains("CNAME") && types.len() > 1 {
            findings.push(Finding {
                code: "CNAME_COEXISTENCE",
                verdict: Verdict::Block,
                owner: owner.to_string(),
                record_types: types.into_iter().map(str::to_string).collect(),
                detail: "CNAME owner also has other data".to_string(),
            });
        }
    }
}

fn check_ttls(changes: &[Change], limits: &Limits, findings: &mut Vec<Finding>) {
    for change in changes {
        if let ("ttl", Some(old), Some(new)) = (change.kind, change.old_ttl, change.new_ttl) {
            let permitted_drop = u64::from(old) * u64::from(limits.ttl_decrease_percent) / 100;
            if new < old && u64::from(old - new) > permitted_drop {
                findings.push(Finding {
                    code: "TTL_DECREASE",
                    verdict: Verdict::Block,
                    owner: change.key.owner.clone(),
                    record_types: vec![change.key.record_type.clone()],
                    detail: format!(
                        "TTL decreased from {old} to {new}, beyond {}% policy",
                        limits.ttl_decrease_percent
                    ),
                });
            }
        }
    }
}

fn check_soa(selected: &ZonePolicy, old: &ZoneData, new: &ZoneData, findings: &mut Vec<Finding>) {
    let old_serial = soa_serial(&old.records, &selected.zone);
    let new_serial = soa_serial(&new.records, &selected.zone);
    let (Some(old_serial), Some(new_serial)) = (old_serial, new_serial) else {
        return;
    };
    let delta = new_serial.wrapping_sub(old_serial);
    let (code, verdict, detail) = if delta == 0 {
        (
            "SOA_EQUAL",
            Verdict::Block,
            format!("SOA serial did not advance from {old_serial}"),
        )
    } else if delta == 1_u32 << 31 {
        (
            "SOA_HALF_RANGE_AMBIGUOUS",
            Verdict::Review,
            format!("SOA serial {old_serial} to {new_serial} is RFC 1982 half-range ambiguous"),
        )
    } else if delta < 1_u32 << 31 {
        (
            "SOA_ADVANCE",
            Verdict::Allow,
            format!("SOA serial advanced from {old_serial} to {new_serial}"),
        )
    } else {
        (
            "SOA_REGRESSION",
            Verdict::Block,
            format!("SOA serial regressed from {old_serial} to {new_serial}"),
        )
    };
    findings.push(Finding {
        code,
        verdict,
        owner: selected.zone.clone(),
        record_types: vec!["SOA".to_string()],
        detail,
    });
}

fn soa_serial(records: &BTreeMap<RecordKey, u32>, zone: &str) -> Option<u32> {
    let mut matching = records
        .keys()
        .filter(|key| key.owner == zone && key.record_type == "SOA");
    let key = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    key.rdata.split_whitespace().nth(2)?.parse().ok()
}

fn check_delegations(
    selected: &ZonePolicy,
    old: &ZoneData,
    new: &ZoneData,
    findings: &mut Vec<Finding>,
) {
    let old_ns = delegation_records(&old.records, &selected.zone);
    let new_ns = delegation_records(&new.records, &selected.zone);
    for key in old_ns.difference(&new_ns) {
        findings.push(Finding {
            code: "DELEGATION_REMOVAL",
            verdict: Verdict::Review,
            owner: key.owner.clone(),
            record_types: vec!["NS".to_string()],
            detail: format!("delegation name server {} was removed", key.rdata),
        });
    }
    for key in new_ns.difference(&old_ns) {
        findings.push(Finding {
            code: "DELEGATION_ADDITION",
            verdict: Verdict::Review,
            owner: key.owner.clone(),
            record_types: vec!["NS".to_string()],
            detail: format!("delegation name server {} was added", key.rdata),
        });
    }
    for key in &new_ns {
        let target = &key.rdata;
        let in_bailiwick = is_at_or_below(target, &key.owner);
        let old_families = address_families(&old.records, target);
        let new_families = address_families(&new.records, target);
        if in_bailiwick && new_families.is_empty() {
            findings.push(Finding {
                code: if old_families.is_empty() {
                    "MISSING_GLUE"
                } else {
                    "REMOVED_LAST_GLUE"
                },
                verdict: Verdict::Block,
                owner: key.owner.clone(),
                record_types: vec!["NS".to_string(), "A".to_string(), "AAAA".to_string()],
                detail: format!("in-bailiwick name server {target} has no parent glue address"),
            });
        } else if in_bailiwick && !old_families.is_empty() && old_families != new_families {
            findings.push(Finding {
                code: "GLUE_FAMILY_CHANGE",
                verdict: Verdict::Allow,
                owner: key.owner.clone(),
                record_types: new_families.into_iter().map(str::to_string).collect(),
                detail: format!("name server {target} retains at least one glue address family"),
            });
        } else if !in_bailiwick && new_families.is_empty() {
            findings.push(Finding {
                code: "OUT_OF_BAILIWICK_NO_GLUE",
                verdict: Verdict::Allow,
                owner: key.owner.clone(),
                record_types: vec!["NS".to_string()],
                detail: format!("out-of-bailiwick name server {target} requires no parent glue"),
            });
        }
    }
}

fn delegation_records<'a>(
    records: &'a BTreeMap<RecordKey, u32>,
    zone: &str,
) -> BTreeSet<&'a RecordKey> {
    records
        .keys()
        .filter(|key| key.record_type == "NS" && key.owner != zone)
        .collect()
}

fn address_families<'a>(records: &'a BTreeMap<RecordKey, u32>, owner: &str) -> BTreeSet<&'a str> {
    records
        .keys()
        .filter(|key| key.owner == owner && matches!(key.record_type.as_str(), "A" | "AAAA"))
        .map(|key| key.record_type.as_str())
        .collect()
}

fn is_at_or_below(name: &str, parent: &str) -> bool {
    name == parent
        || (name.ends_with(parent)
            && name.as_bytes().get(name.len() - parent.len() - 1) == Some(&b'.'))
}

fn json(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            value if value <= '\u{1f}' => {
                write!(output, "\\u{:04x}", u32::from(value)).expect("write to string");
            }
            value => output.push(value),
        }
    }
    output
}

fn optional_number(value: Option<u32>) -> String {
    value.map_or_else(|| "null".to_string(), |number| number.to_string())
}
