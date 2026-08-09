mod reference;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::io::{ErrorKind, Write as IoWrite};
use std::net::IpAddr;
use std::path::Path;

pub use reference::{evaluate as evaluate_linear, parse_packet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IpFamily {
    Ipv4,
    Ipv6,
}

impl IpFamily {
    fn bits(self) -> u8 {
        match self {
            Self::Ipv4 => 32,
            Self::Ipv6 => 128,
        }
    }

    fn maximum(self) -> u128 {
        match self {
            Self::Ipv4 => u32::MAX as u128,
            Self::Ipv6 => u128::MAX,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyFile {
    pub rules: Vec<RuleInput>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuleInput {
    pub id: String,
    pub action: Action,
    pub ip_family: IpFamily,
    pub source_cidr: String,
    pub protocol: Protocol,
    pub port_start: u16,
    pub port_end: u16,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub action: Action,
    pub family: IpFamily,
    pub source_start: u128,
    pub source_end: u128,
    pub protocol: Protocol,
    pub port_start: u16,
    pub port_end: u16,
}

#[derive(Debug, Clone)]
pub struct ValidatedPolicy {
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    pub family: IpFamily,
    pub source: u128,
    pub protocol: Protocol,
    pub destination_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Decision {
    pub action: Action,
    pub rule_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeKind {
    NewlyAllowed,
    NewlyDenied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Witness {
    pub source_ip: String,
    pub protocol: Protocol,
    pub destination_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeRegion {
    pub kind: ChangeKind,
    pub ip_family: IpFamily,
    pub source_start: String,
    pub source_end: String,
    pub protocol: Protocol,
    pub port_start: u16,
    pub port_end: u16,
    pub old_decision: Action,
    pub old_rule_id: String,
    pub new_decision: Action,
    pub new_rule_id: String,
    pub witness: Witness,
    #[serde(skip)]
    source_start_num: u128,
    #[serde(skip)]
    source_end_num: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleReachability {
    pub rule_id: String,
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub witness: Option<Witness>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportSummary {
    pub outcome: String,
    pub newly_allowed_regions: usize,
    pub newly_denied_regions: usize,
    pub proposed_reachable_rules: usize,
    pub proposed_unreachable_rules: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub schema_version: u32,
    pub analysis: String,
    pub summary: ReportSummary,
    pub changes: Vec<ChangeRegion>,
    pub proposed_rule_reachability: Vec<RuleReachability>,
}

pub fn parse_and_validate(bytes: &[u8], label: &str) -> Result<ValidatedPolicy, Vec<String>> {
    let input: PolicyFile = match serde_json::from_slice(bytes) {
        Ok(input) => input,
        Err(error) => return Err(vec![format!("{label}: invalid JSON or schema: {error}")]),
    };

    let mut errors = Vec::new();
    let mut ids = HashSet::with_capacity(input.rules.len());
    let mut rules = Vec::with_capacity(input.rules.len());

    for (index, input_rule) in input.rules.into_iter().enumerate() {
        let location = format!("{label}: rule[{index}] id={:?}", input_rule.id);
        if input_rule.id.is_empty() {
            errors.push(format!("{location}: id must not be empty"));
        } else if input_rule.id == "default-deny" {
            errors.push(format!(
                "{location}: id is reserved for the implicit default-deny decision"
            ));
        } else if !ids.insert(input_rule.id.clone()) {
            errors.push(format!("{location}: duplicate rule id"));
        }
        if input_rule.port_start > input_rule.port_end {
            errors.push(format!(
                "{location}: reversed destination-port range {}..{}",
                input_rule.port_start, input_rule.port_end
            ));
        }

        match parse_canonical_cidr(&input_rule.source_cidr, input_rule.ip_family) {
            Ok((source_start, source_end)) => rules.push(Rule {
                id: input_rule.id,
                action: input_rule.action,
                family: input_rule.ip_family,
                source_start,
                source_end,
                protocol: input_rule.protocol,
                port_start: input_rule.port_start,
                port_end: input_rule.port_end,
            }),
            Err(error) => errors.push(format!("{location}: {error}")),
        }
    }

    if errors.is_empty() {
        Ok(ValidatedPolicy { rules })
    } else {
        Err(errors)
    }
}

pub fn read_and_validate_pair(
    old_path: &Path,
    proposed_path: &Path,
) -> Result<(ValidatedPolicy, ValidatedPolicy), String> {
    let mut diagnostics = Vec::new();
    let old = match std::fs::read(old_path) {
        Ok(bytes) => match parse_and_validate(&bytes, "old policy") {
            Ok(policy) => Some(policy),
            Err(errors) => {
                diagnostics.extend(errors);
                None
            }
        },
        Err(error) => {
            diagnostics.push(format!(
                "old policy: cannot read {}: {error}",
                old_path.display()
            ));
            None
        }
    };
    let proposed = match std::fs::read(proposed_path) {
        Ok(bytes) => match parse_and_validate(&bytes, "proposed policy") {
            Ok(policy) => Some(policy),
            Err(errors) => {
                diagnostics.extend(errors);
                None
            }
        },
        Err(error) => {
            diagnostics.push(format!(
                "proposed policy: cannot read {}: {error}",
                proposed_path.display()
            ));
            None
        }
    };

    match (old, proposed) {
        (Some(old), Some(proposed)) => Ok((old, proposed)),
        _ => Err(format!(
            "input validation failed; no semantic verdict was produced:\n{}",
            diagnostics.join("\n")
        )),
    }
}

/// Clear a prior report before reading either policy, then keep the requested
/// path absent if any subsequent step returns an error.
pub fn analyze_files_to_report(
    old_path: &Path,
    proposed_path: &Path,
    report_path: &Path,
) -> Result<Report, String> {
    reject_input_alias(report_path, old_path, "old policy")?;
    reject_input_alias(report_path, proposed_path, "proposed policy")?;
    remove_requested_report(report_path)?;

    let result = (|| {
        let (old, proposed) = read_and_validate_pair(old_path, proposed_path)?;
        let report = analyze(&old, &proposed);
        write_report(report_path, &report)?;
        Ok(report)
    })();

    match result {
        Ok(report) => Ok(report),
        Err(error) => match remove_requested_report(report_path) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(format!(
                "{error}\nadditionally failed to keep requested report path absent: {cleanup_error}"
            )),
        },
    }
}

fn reject_input_alias(report_path: &Path, input_path: &Path, label: &str) -> Result<(), String> {
    if report_path == input_path {
        return Err(format!(
            "requested report path aliases the {label}; refusing to unlink or overwrite an input policy"
        ));
    }

    if let (Ok(report_canonical), Ok(input_canonical)) = (
        std::fs::canonicalize(report_path),
        std::fs::canonicalize(input_path),
    ) && report_canonical == input_canonical
    {
        return Err(format!(
            "requested report path aliases the {label}; refusing to unlink or overwrite an input policy"
        ));
    }

    #[cfg(unix)]
    if let (Ok(report_metadata), Ok(input_metadata)) = (
        std::fs::metadata(report_path),
        std::fs::metadata(input_path),
    ) {
        use std::os::unix::fs::MetadataExt;
        if report_metadata.dev() == input_metadata.dev()
            && report_metadata.ino() == input_metadata.ino()
        {
            return Err(format!(
                "requested report path aliases the {label}; refusing to unlink or overwrite an input policy"
            ));
        }
    }

    Ok(())
}

fn remove_requested_report(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            std::fs::remove_file(path)
                .map_err(|error| format!("cannot remove prior report {}: {error}", path.display()))
        }
        Ok(_) => Err(format!(
            "requested report path {} exists but is not a regular file or symbolic link",
            path.display()
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect requested report path {}: {error}",
            path.display()
        )),
    }
}

fn parse_canonical_cidr(cidr: &str, family: IpFamily) -> Result<(u128, u128), String> {
    let (address_text, prefix_text) = cidr
        .split_once('/')
        .ok_or_else(|| format!("malformed CIDR {cidr:?}: missing '/'"))?;
    if prefix_text.contains('/') {
        return Err(format!("malformed CIDR {cidr:?}: too many '/' separators"));
    }
    let address: IpAddr = address_text
        .parse()
        .map_err(|_| format!("malformed CIDR {cidr:?}: invalid address"))?;
    let prefix: u8 = prefix_text
        .parse()
        .map_err(|_| format!("malformed CIDR {cidr:?}: invalid prefix length"))?;

    let (actual_family, raw) = match address {
        IpAddr::V4(address) => (IpFamily::Ipv4, u32::from(address) as u128),
        IpAddr::V6(address) => (IpFamily::Ipv6, u128::from(address)),
    };
    if actual_family != family {
        return Err(format!(
            "CIDR family {:?} does not match declared family {:?}",
            actual_family, family
        ));
    }
    if prefix > family.bits() {
        return Err(format!(
            "CIDR prefix /{prefix} exceeds {}-bit {:?} width",
            family.bits(),
            family
        ));
    }

    let host_mask = if prefix == family.bits() {
        0
    } else {
        family.maximum() >> prefix
    };
    if raw & host_mask != 0 {
        return Err(format!("CIDR {cidr:?} is not canonical"));
    }
    Ok((raw, raw | host_mask))
}

pub fn analyze(old: &ValidatedPolicy, proposed: &ValidatedPolicy) -> Report {
    let mut changes = Vec::new();
    let mut reachable_witnesses = vec![None; proposed.rules.len()];

    for family in [IpFamily::Ipv4, IpFamily::Ipv6] {
        for protocol in [Protocol::Tcp, Protocol::Udp] {
            analyze_dimension(
                old,
                proposed,
                family,
                protocol,
                &mut changes,
                &mut reachable_witnesses,
            );
        }
    }

    changes = normalize_regions(changes);
    let newly_allowed_regions = changes
        .iter()
        .filter(|region| region.kind == ChangeKind::NewlyAllowed)
        .count();
    let newly_denied_regions = changes.len() - newly_allowed_regions;
    let proposed_reachable_rules = reachable_witnesses.iter().filter(|w| w.is_some()).count();
    let proposed_unreachable_rules = proposed.rules.len() - proposed_reachable_rules;
    let outcome = match (newly_allowed_regions > 0, newly_denied_regions > 0) {
        (false, false) => "exact-no-semantic-change",
        (true, false) => "newly-allowed",
        (false, true) => "newly-denied",
        (true, true) => "newly-allowed-and-newly-denied",
    };
    let proposed_rule_reachability = proposed
        .rules
        .iter()
        .zip(reachable_witnesses)
        .map(|(rule, witness)| RuleReachability {
            rule_id: rule.id.clone(),
            reachable: witness.is_some(),
            witness,
        })
        .collect();

    Report {
        schema_version: 1,
        analysis: "exact ordered first-match over source CIDR, TCP/UDP, and destination-port interval; implicit default deny".to_string(),
        summary: ReportSummary {
            outcome: outcome.to_string(),
            newly_allowed_regions,
            newly_denied_regions,
            proposed_reachable_rules,
            proposed_unreachable_rules,
        },
        changes,
        proposed_rule_reachability,
    }
}

#[derive(Debug, Clone, Copy)]
struct Event {
    at: u128,
    rule_index: usize,
    add: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PortRun {
    start: u16,
    end: u16,
    winner: Option<usize>,
}

fn analyze_dimension(
    old: &ValidatedPolicy,
    proposed: &ValidatedPolicy,
    family: IpFamily,
    protocol: Protocol,
    changes: &mut Vec<ChangeRegion>,
    reachable_witnesses: &mut [Option<Witness>],
) {
    let old_indexes = matching_indexes(old, family, protocol);
    let new_indexes = matching_indexes(proposed, family, protocol);
    if old_indexes.is_empty() && new_indexes.is_empty() {
        return;
    }

    let mut port_coordinates = Vec::with_capacity((old_indexes.len() + new_indexes.len()) * 2);
    for (policy, indexes) in [(old, &old_indexes), (proposed, &new_indexes)] {
        for &index in indexes {
            let rule = &policy.rules[index];
            port_coordinates.push(rule.port_start as u32);
            if rule.port_end < u16::MAX {
                port_coordinates.push(rule.port_end as u32 + 1);
            }
        }
    }
    port_coordinates.sort_unstable();
    port_coordinates.dedup();

    let mut old_tree = PortTree::new(port_coordinates.clone());
    let mut new_tree = PortTree::new(port_coordinates);
    let mut old_events = make_events(old, &old_indexes, family.maximum());
    let mut new_events = make_events(proposed, &new_indexes, family.maximum());
    let mut addresses: Vec<u128> = old_events
        .iter()
        .chain(&new_events)
        .map(|event| event.at)
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    old_events.sort_by_key(|event| (event.at, event.add, event.rule_index));
    new_events.sort_by_key(|event| (event.at, event.add, event.rule_index));

    let mut old_cursor = 0;
    let mut new_cursor = 0;
    for (address_position, &address_start) in addresses.iter().enumerate() {
        apply_events_at(
            &old_events,
            &mut old_cursor,
            address_start,
            old,
            &mut old_tree,
        );
        apply_events_at(
            &new_events,
            &mut new_cursor,
            address_start,
            proposed,
            &mut new_tree,
        );
        if old_tree.is_empty() && new_tree.is_empty() {
            continue;
        }

        let address_end = addresses
            .get(address_position + 1)
            .map_or(family.maximum(), |next| next - 1);
        let old_runs = old_tree.runs();
        let new_runs = new_tree.runs();

        for run in &new_runs {
            if let Some(rule_index) = run.winner
                && reachable_witnesses[rule_index].is_none()
            {
                reachable_witnesses[rule_index] = Some(Witness {
                    source_ip: format_address(family, address_start),
                    protocol,
                    destination_port: run.start,
                });
            }
        }

        compare_runs(
            old,
            proposed,
            family,
            protocol,
            address_start,
            address_end,
            &old_runs,
            &new_runs,
            changes,
        );
    }
}

fn matching_indexes(policy: &ValidatedPolicy, family: IpFamily, protocol: Protocol) -> Vec<usize> {
    policy
        .rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            (rule.family == family && rule.protocol == protocol).then_some(index)
        })
        .collect()
}

fn make_events(policy: &ValidatedPolicy, indexes: &[usize], maximum: u128) -> Vec<Event> {
    let mut events = Vec::with_capacity(indexes.len() * 2);
    for &rule_index in indexes {
        let rule = &policy.rules[rule_index];
        events.push(Event {
            at: rule.source_start,
            rule_index,
            add: true,
        });
        if rule.source_end < maximum {
            events.push(Event {
                at: rule.source_end + 1,
                rule_index,
                add: false,
            });
        }
    }
    events
}

fn apply_events_at(
    events: &[Event],
    cursor: &mut usize,
    address: u128,
    policy: &ValidatedPolicy,
    tree: &mut PortTree,
) {
    while *cursor < events.len() && events[*cursor].at == address {
        let event = events[*cursor];
        let rule = &policy.rules[event.rule_index];
        tree.update(rule.port_start, rule.port_end, event.rule_index, event.add);
        *cursor += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn compare_runs(
    old: &ValidatedPolicy,
    proposed: &ValidatedPolicy,
    family: IpFamily,
    protocol: Protocol,
    address_start: u128,
    address_end: u128,
    old_runs: &[PortRun],
    new_runs: &[PortRun],
    changes: &mut Vec<ChangeRegion>,
) {
    let mut old_index = 0;
    let mut new_index = 0;
    while old_index < old_runs.len() && new_index < new_runs.len() {
        let old_run = old_runs[old_index];
        let new_run = new_runs[new_index];
        let start = old_run.start.max(new_run.start);
        let end = old_run.end.min(new_run.end);
        let old_decision = decision_for_winner(old, old_run.winner);
        let new_decision = decision_for_winner(proposed, new_run.winner);

        if old_decision.action != new_decision.action {
            let kind = if new_decision.action == Action::Allow {
                ChangeKind::NewlyAllowed
            } else {
                ChangeKind::NewlyDenied
            };
            changes.push(ChangeRegion {
                kind,
                ip_family: family,
                source_start: format_address(family, address_start),
                source_end: format_address(family, address_end),
                protocol,
                port_start: start,
                port_end: end,
                old_decision: old_decision.action,
                old_rule_id: old_decision.rule_id,
                new_decision: new_decision.action,
                new_rule_id: new_decision.rule_id,
                witness: Witness {
                    source_ip: format_address(family, address_start),
                    protocol,
                    destination_port: start,
                },
                source_start_num: address_start,
                source_end_num: address_end,
            });
        }

        if old_run.end == end {
            old_index += 1;
        }
        if new_run.end == end {
            new_index += 1;
        }
    }
}

fn decision_for_winner(policy: &ValidatedPolicy, winner: Option<usize>) -> Decision {
    winner.map_or_else(
        || Decision {
            action: Action::Deny,
            rule_id: "default-deny".to_string(),
        },
        |index| Decision {
            action: policy.rules[index].action,
            rule_id: policy.rules[index].id.clone(),
        },
    )
}

fn normalize_regions(mut regions: Vec<ChangeRegion>) -> Vec<ChangeRegion> {
    regions.sort_by(|a, b| vertical_key(a).cmp(&vertical_key(b)));
    let mut vertically_merged: Vec<ChangeRegion> = Vec::with_capacity(regions.len());
    for region in regions {
        if let Some(last) = vertically_merged.last_mut()
            && same_except_address(last, &region)
            && last.source_end_num.checked_add(1) == Some(region.source_start_num)
        {
            last.source_end_num = region.source_end_num;
            last.source_end = region.source_end;
            continue;
        }
        vertically_merged.push(region);
    }

    vertically_merged.sort_by(compare_region_order);
    let mut normalized: Vec<ChangeRegion> = Vec::with_capacity(vertically_merged.len());
    for region in vertically_merged {
        if let Some(last) = normalized.last_mut()
            && same_except_port(last, &region)
            && last.port_end.checked_add(1) == Some(region.port_start)
        {
            last.port_end = region.port_end;
            continue;
        }
        normalized.push(region);
    }
    normalized.sort_by(compare_region_order);
    normalized
}

fn vertical_key(region: &ChangeRegion) -> impl Ord + '_ {
    (
        region.ip_family,
        region.protocol,
        region.port_start,
        region.port_end,
        region.kind.clone(),
        region.old_decision,
        region.old_rule_id.as_str(),
        region.new_decision,
        region.new_rule_id.as_str(),
        region.source_start_num,
        region.source_end_num,
    )
}

fn compare_region_order(a: &ChangeRegion, b: &ChangeRegion) -> std::cmp::Ordering {
    (
        a.ip_family,
        a.source_start_num,
        a.source_end_num,
        a.protocol,
        a.port_start,
        a.port_end,
        &a.kind,
        a.old_decision,
        a.old_rule_id.as_str(),
        a.new_decision,
        a.new_rule_id.as_str(),
    )
        .cmp(&(
            b.ip_family,
            b.source_start_num,
            b.source_end_num,
            b.protocol,
            b.port_start,
            b.port_end,
            &b.kind,
            b.old_decision,
            b.old_rule_id.as_str(),
            b.new_decision,
            b.new_rule_id.as_str(),
        ))
}

fn same_except_address(a: &ChangeRegion, b: &ChangeRegion) -> bool {
    a.ip_family == b.ip_family
        && a.protocol == b.protocol
        && a.port_start == b.port_start
        && a.port_end == b.port_end
        && a.kind == b.kind
        && a.old_decision == b.old_decision
        && a.old_rule_id == b.old_rule_id
        && a.new_decision == b.new_decision
        && a.new_rule_id == b.new_rule_id
}

fn same_except_port(a: &ChangeRegion, b: &ChangeRegion) -> bool {
    a.ip_family == b.ip_family
        && a.source_start_num == b.source_start_num
        && a.source_end_num == b.source_end_num
        && a.protocol == b.protocol
        && a.kind == b.kind
        && a.old_decision == b.old_decision
        && a.old_rule_id == b.old_rule_id
        && a.new_decision == b.new_decision
        && a.new_rule_id == b.new_rule_id
}

fn format_address(family: IpFamily, raw: u128) -> String {
    match family {
        IpFamily::Ipv4 => std::net::Ipv4Addr::from(raw as u32).to_string(),
        IpFamily::Ipv6 => std::net::Ipv6Addr::from(raw).to_string(),
    }
}

struct PortTree {
    coordinates: Vec<u32>,
    size: usize,
    own: Vec<BTreeSet<usize>>,
    subtree_minimum: Vec<Option<usize>>,
}

impl PortTree {
    fn new(coordinates: Vec<u32>) -> Self {
        let size = coordinates.len().next_power_of_two();
        Self {
            coordinates,
            size,
            own: vec![BTreeSet::new(); size * 2],
            subtree_minimum: vec![None; size * 2],
        }
    }

    fn is_empty(&self) -> bool {
        self.subtree_minimum[1].is_none()
    }

    fn update(&mut self, start: u16, end: u16, rule_index: usize, add: bool) {
        let left = self.coordinates.binary_search(&(start as u32)).unwrap();
        let right = if end == u16::MAX {
            self.coordinates.len() - 1
        } else {
            self.coordinates.binary_search(&(end as u32 + 1)).unwrap() - 1
        };
        self.update_node(1, 0, self.size - 1, left, right, rule_index, add);
    }

    #[allow(clippy::too_many_arguments)]
    fn update_node(
        &mut self,
        node: usize,
        left: usize,
        right: usize,
        query_left: usize,
        query_right: usize,
        rule_index: usize,
        add: bool,
    ) {
        if query_left <= left && right <= query_right {
            if add {
                self.own[node].insert(rule_index);
            } else {
                let removed = self.own[node].remove(&rule_index);
                debug_assert!(removed);
            }
            self.pull(node);
            return;
        }
        let middle = (left + right) / 2;
        if query_left <= middle {
            self.update_node(
                node * 2,
                left,
                middle,
                query_left,
                query_right,
                rule_index,
                add,
            );
        }
        if query_right > middle {
            self.update_node(
                node * 2 + 1,
                middle + 1,
                right,
                query_left,
                query_right,
                rule_index,
                add,
            );
        }
        self.pull(node);
    }

    fn pull(&mut self, node: usize) {
        let mut minimum = self.own[node].first().copied();
        if node < self.size {
            minimum = option_min(minimum, self.subtree_minimum[node * 2]);
            minimum = option_min(minimum, self.subtree_minimum[node * 2 + 1]);
        }
        self.subtree_minimum[node] = minimum;
    }

    fn runs(&self) -> Vec<PortRun> {
        let mut runs = Vec::new();
        self.collect_runs(1, 0, self.size - 1, None, &mut runs);
        runs
    }

    fn collect_runs(
        &self,
        node: usize,
        left: usize,
        right: usize,
        inherited: Option<usize>,
        runs: &mut Vec<PortRun>,
    ) {
        if left >= self.coordinates.len() {
            return;
        }
        let own_minimum = self.own[node].first().copied();
        let base = option_min(inherited, own_minimum);
        let descendant_minimum = if node < self.size {
            option_min(
                self.subtree_minimum[node * 2],
                self.subtree_minimum[node * 2 + 1],
            )
        } else {
            None
        };
        let uniform = match (base, descendant_minimum) {
            (_, None) => true,
            (Some(base), Some(descendant)) => base <= descendant,
            (None, Some(_)) => false,
        };

        if left == right || uniform {
            let capped_right = right.min(self.coordinates.len() - 1);
            let start = self.coordinates[left] as u16;
            let end = self
                .coordinates
                .get(capped_right + 1)
                .map_or(u16::MAX, |next| (*next - 1) as u16);
            push_port_run(
                runs,
                PortRun {
                    start,
                    end,
                    winner: base,
                },
            );
            return;
        }

        let middle = (left + right) / 2;
        self.collect_runs(node * 2, left, middle, base, runs);
        self.collect_runs(node * 2 + 1, middle + 1, right, base, runs);
    }
}

fn option_min(left: Option<usize>, right: Option<usize>) -> Option<usize> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn push_port_run(runs: &mut Vec<PortRun>, run: PortRun) {
    if let Some(last) = runs.last_mut()
        && last.winner == run.winner
        && last.end.checked_add(1) == Some(run.start)
    {
        last.end = run.end;
        return;
    }
    runs.push(run);
}

pub fn report_json(report: &Report) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn write_report(path: &Path, report: &Report) -> Result<(), String> {
    let bytes = report_json(report)?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("report path {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("report path {} has no valid file name", path.display()))?;
    const MAX_TEMPORARY_ATTEMPTS: usize = 256;
    for attempt in 0..MAX_TEMPORARY_ATTEMPTS {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!("-{attempt}")
        };
        let temporary = parent.join(format!(".{name}.tmp-{}{suffix}", std::process::id()));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create temporary report {}: {error}",
                    temporary.display()
                ));
            }
        };

        let write_result = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = write_result {
            let cleanup_suffix = cleanup_owned_temporary(&temporary);
            return Err(format!(
                "cannot write temporary report {}: {error}{cleanup_suffix}",
                temporary.display()
            ));
        }
        if let Err(error) = std::fs::rename(&temporary, path) {
            let cleanup_suffix = cleanup_owned_temporary(&temporary);
            return Err(format!(
                "cannot publish report {}: {error}{cleanup_suffix}",
                path.display()
            ));
        }
        return Ok(());
    }

    Err(format!(
        "cannot create a fresh temporary report after {MAX_TEMPORARY_ATTEMPTS} collisions for {}",
        path.display()
    ))
}

fn cleanup_owned_temporary(path: &Path) -> String {
    std::fs::remove_file(path)
        .err()
        .map_or_else(String::new, |cleanup_error| {
            format!(
                "; additionally cannot remove owned temporary report {}: {cleanup_error}",
                path.display()
            )
        })
}

pub fn verify_report(
    old: &ValidatedPolicy,
    proposed: &ValidatedPolicy,
    report: &Report,
) -> Result<(usize, usize), String> {
    let fresh = analyze(old, proposed);
    if report_json(&fresh)? != report_json(report)? {
        return Err("report does not byte-semantically match a fresh exact analysis".to_string());
    }

    let mut change_witnesses = 0;
    for region in &report.changes {
        let packet = parse_packet(
            &region.witness.source_ip,
            region.witness.protocol,
            region.witness.destination_port,
        )?;
        let source_start = parse_address_for_family(&region.source_start, region.ip_family)?;
        let source_end = parse_address_for_family(&region.source_end, region.ip_family)?;
        if packet.family != region.ip_family
            || packet.source < source_start
            || packet.source > source_end
            || packet.destination_port < region.port_start
            || packet.destination_port > region.port_end
        {
            return Err(format!(
                "change witness is outside region: {:?}",
                region.witness
            ));
        }
        let old_decision = evaluate_linear(old, packet);
        let new_decision = evaluate_linear(proposed, packet);
        if old_decision.action != region.old_decision
            || old_decision.rule_id != region.old_rule_id
            || new_decision.action != region.new_decision
            || new_decision.rule_id != region.new_rule_id
        {
            return Err(format!(
                "change witness did not reproduce decisions: {:?}",
                region.witness
            ));
        }
        change_witnesses += 1;
    }

    let mut reachability_witnesses = 0;
    for reachability in &report.proposed_rule_reachability {
        if let Some(witness) = &reachability.witness {
            let packet = parse_packet(
                &witness.source_ip,
                witness.protocol,
                witness.destination_port,
            )?;
            let decision = evaluate_linear(proposed, packet);
            if decision.rule_id != reachability.rule_id {
                return Err(format!(
                    "reachability witness for {:?} reached {:?}",
                    reachability.rule_id, decision.rule_id
                ));
            }
            reachability_witnesses += 1;
        }
    }
    Ok((change_witnesses, reachability_witnesses))
}

fn parse_address_for_family(text: &str, family: IpFamily) -> Result<u128, String> {
    let address: IpAddr = text
        .parse()
        .map_err(|_| format!("invalid {:?} address {text:?} in report", family))?;
    match (family, address) {
        (IpFamily::Ipv4, IpAddr::V4(address)) => Ok(u32::from(address) as u128),
        (IpFamily::Ipv6, IpAddr::V6(address)) => Ok(u128::from(address)),
        _ => Err(format!("address {text:?} does not match {:?}", family)),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplePacket {
    pub source_ip: String,
    pub protocol: Protocol,
    pub destination_port: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct SampleReplayResult {
    pub sample_count: usize,
    pub changed_sample_count: usize,
    pub changed_samples: Vec<SampleReplayChange>,
    pub warning: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SampleReplayChange {
    pub sample_index: usize,
    pub packet: Witness,
    pub old: Decision,
    pub proposed: Decision,
}

pub fn replay_samples(
    old: &ValidatedPolicy,
    proposed: &ValidatedPolicy,
    bytes: &[u8],
) -> Result<SampleReplayResult, String> {
    let samples: Vec<SamplePacket> = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid sample packet JSON or schema: {error}"))?;
    let mut changed_samples = Vec::new();
    for (sample_index, sample) in samples.iter().enumerate() {
        let packet = parse_packet(&sample.source_ip, sample.protocol, sample.destination_port)?;
        let old_decision = evaluate_linear(old, packet);
        let new_decision = evaluate_linear(proposed, packet);
        if old_decision.action != new_decision.action {
            changed_samples.push(SampleReplayChange {
                sample_index,
                packet: Witness {
                    source_ip: sample.source_ip.clone(),
                    protocol: sample.protocol,
                    destination_port: sample.destination_port,
                },
                old: old_decision,
                proposed: new_decision,
            });
        }
    }
    Ok(SampleReplayResult {
        sample_count: samples.len(),
        changed_sample_count: changed_samples.len(),
        changed_samples,
        warning: "sample replay is incomplete evidence and never establishes no semantic change"
            .to_string(),
    })
}

pub fn generate_benchmark_pair(rule_count: usize, seed: u64) -> (PolicyFile, PolicyFile) {
    assert!(rule_count > 0);
    let mut rules = Vec::with_capacity(rule_count);
    let mut random = SplitMix64::new(seed);
    let mut pair_index = 0usize;
    while rules.len() < rule_count {
        let family = if pair_index.is_multiple_of(2) {
            IpFamily::Ipv4
        } else {
            IpFamily::Ipv6
        };
        let protocol = if random.next_u64() & 1 == 0 {
            Protocol::Tcp
        } else {
            Protocol::Udp
        };
        let action = if pair_index == 0 || random.next_u64() & 1 == 0 {
            Action::Deny
        } else {
            Action::Allow
        };
        let (cidr, prefix) = if family == IpFamily::Ipv4 {
            let raw = 0x0a00_0000u32 + (pair_index / 2) as u32;
            (std::net::Ipv4Addr::from(raw).to_string(), 32)
        } else {
            let raw = 0x2001_0db8_0000_0000_0000_0000_0000_0000u128 + (pair_index / 2) as u128;
            (std::net::Ipv6Addr::from(raw).to_string(), 128)
        };
        let (first, end) = if pair_index == 0 {
            (0, u16::MAX)
        } else {
            let first = random.next_u64() as u16;
            let width = (random.next_u64() % 1024) as u16;
            (first, first.saturating_add(width))
        };
        let primary = benchmark_rule(
            &format!("primary-{pair_index:05}"),
            action,
            family,
            &format!("{cidr}/{prefix}"),
            protocol,
            first,
            end,
        );
        rules.push(primary.clone());
        if rules.len() < rule_count {
            let mut shadow = primary;
            shadow.id = format!("shadow-{pair_index:05}");
            shadow.action = match action {
                Action::Allow => Action::Deny,
                Action::Deny => Action::Allow,
            };
            rules.push(shadow);
        }
        pair_index += 1;
    }
    let old = PolicyFile { rules };
    let mut proposed = old.clone();
    proposed.rules[0].action = Action::Allow;
    (old, proposed)
}

fn benchmark_rule(
    id: &str,
    action: Action,
    ip_family: IpFamily,
    source_cidr: &str,
    protocol: Protocol,
    port_start: u16,
    port_end: u16,
) -> RuleInput {
    RuleInput {
        id: id.to_string(),
        action,
        ip_family,
        source_cidr: source_cidr.to_string(),
        protocol,
        port_start,
        port_end,
    }
}

fn canonical_start(raw: u128, family: IpFamily, prefix: u8) -> u128 {
    let maximum = family.maximum();
    let host_mask = if prefix == family.bits() {
        0
    } else {
        maximum >> prefix
    };
    (raw & maximum) & !host_mask
}

pub fn write_benchmark_fixture(
    directory: &Path,
    rule_count: usize,
    seed: u64,
) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let (old, proposed) = generate_benchmark_pair(rule_count, seed);
    let mut old_bytes = serde_json::to_vec(&old).map_err(|error| error.to_string())?;
    old_bytes.push(b'\n');
    let mut proposed_bytes = serde_json::to_vec(&proposed).map_err(|error| error.to_string())?;
    proposed_bytes.push(b'\n');
    std::fs::write(directory.join("old.json"), old_bytes)
        .map_err(|error| format!("cannot write benchmark old policy: {error}"))?;
    std::fs::write(directory.join("proposed.json"), proposed_bytes)
        .map_err(|error| format!("cannot write benchmark proposed policy: {error}"))?;
    Ok(())
}

pub fn validation_suite(
    small_pair_count: usize,
    full_pair_count: usize,
    total_probe_count: usize,
    seed: u64,
) -> Result<String, String> {
    let mut random = SplitMix64::new(seed);
    let mut exhaustive_packets = 0usize;
    for pair_index in 0..small_pair_count {
        let (old, proposed) = generated_small_pair(&mut random, pair_index);
        let report = analyze(&old, &proposed);
        exhaustive_packets += verify_reduced_pair(&old, &proposed, &report)?;
    }

    let base_probes = total_probe_count / full_pair_count;
    let extra_probes = total_probe_count % full_pair_count;
    let mut probes = 0usize;
    for pair_index in 0..full_pair_count {
        let (old, proposed) = generated_full_pair(&mut random, pair_index);
        let report = analyze(&old, &proposed);
        let pair_probes = base_probes + usize::from(pair_index < extra_probes);
        verify_boundary_probes(&old, &proposed, &report, pair_probes, &mut random)?;
        probes += pair_probes;
    }

    Ok(format!(
        "small_pairs={small_pair_count} exhaustive_packets={exhaustive_packets} full_width_pairs={full_pair_count} boundary_probes={probes} disagreements=0"
    ))
}

fn generated_small_pair(
    random: &mut SplitMix64,
    pair_index: usize,
) -> (ValidatedPolicy, ValidatedPolicy) {
    let count = 1 + (random.next_u64() % 7) as usize;
    let mut old_inputs = Vec::with_capacity(count);
    for rule_index in 0..count {
        old_inputs.push(random_small_rule(
            random,
            format!("r{pair_index}-{rule_index}"),
        ));
    }
    let mut proposed_inputs = old_inputs.clone();
    match random.next_u64() % 4 {
        0 => {
            let position = (random.next_u64() as usize) % (proposed_inputs.len() + 1);
            proposed_inputs.insert(
                position,
                random_small_rule(random, format!("inserted-{pair_index}")),
            );
        }
        1 if proposed_inputs.len() > 1 => {
            let position = (random.next_u64() as usize) % proposed_inputs.len();
            proposed_inputs.remove(position);
        }
        2 if proposed_inputs.len() > 1 => {
            let from = (random.next_u64() as usize) % proposed_inputs.len();
            let rule = proposed_inputs.remove(from);
            let to = (random.next_u64() as usize) % (proposed_inputs.len() + 1);
            proposed_inputs.insert(to, rule);
        }
        _ => {
            let position = (random.next_u64() as usize) % proposed_inputs.len();
            proposed_inputs[position].action = match proposed_inputs[position].action {
                Action::Allow => Action::Deny,
                Action::Deny => Action::Allow,
            };
        }
    }
    (
        validated_from_inputs(old_inputs),
        validated_from_inputs(proposed_inputs),
    )
}

fn random_small_rule(random: &mut SplitMix64, id: String) -> RuleInput {
    let prefix = 28 + (random.next_u64() % 5) as u8;
    let address = (random.next_u64() % 16) as u8;
    let block = 1u8 << (32 - prefix);
    let canonical = address / block * block;
    let first = (random.next_u64() % 8) as u16;
    let second = (random.next_u64() % 8) as u16;
    RuleInput {
        id,
        action: if random.next_u64() & 1 == 0 {
            Action::Allow
        } else {
            Action::Deny
        },
        ip_family: IpFamily::Ipv4,
        source_cidr: format!("0.0.0.{canonical}/{prefix}"),
        protocol: if random.next_u64() & 1 == 0 {
            Protocol::Tcp
        } else {
            Protocol::Udp
        },
        port_start: first.min(second),
        port_end: first.max(second),
    }
}

fn validated_from_inputs(rules: Vec<RuleInput>) -> ValidatedPolicy {
    let bytes = serde_json::to_vec(&PolicyFile { rules }).unwrap();
    parse_and_validate(&bytes, "generated policy").unwrap()
}

fn verify_reduced_pair(
    old: &ValidatedPolicy,
    proposed: &ValidatedPolicy,
    report: &Report,
) -> Result<usize, String> {
    let mut expected_reachable = vec![false; proposed.rules.len()];
    let mut packet_count = 0;
    for address in 0..16u128 {
        for protocol in [Protocol::Tcp, Protocol::Udp] {
            for port in 0..8u16 {
                packet_count += 1;
                let packet = Packet {
                    family: IpFamily::Ipv4,
                    source: address,
                    protocol,
                    destination_port: port,
                };
                let old_decision = evaluate_linear(old, packet);
                let new_decision = evaluate_linear(proposed, packet);
                if let Some(index) = proposed
                    .rules
                    .iter()
                    .position(|rule| rule.id == new_decision.rule_id)
                {
                    expected_reachable[index] = true;
                }
                let containing: Vec<_> = report
                    .changes
                    .iter()
                    .filter(|region| region_contains(region, packet))
                    .collect();
                let expected_change = old_decision.action != new_decision.action;
                if containing.len() != usize::from(expected_change) {
                    return Err(format!(
                        "reduced exhaustive mismatch for packet {:?}: expected_change={expected_change} containing_regions={}",
                        packet,
                        containing.len()
                    ));
                }
                if let Some(region) = containing.first()
                    && (region.old_decision != old_decision.action
                        || region.old_rule_id != old_decision.rule_id
                        || region.new_decision != new_decision.action
                        || region.new_rule_id != new_decision.rule_id)
                {
                    return Err(format!("reduced decision mismatch for packet {packet:?}"));
                }
            }
        }
    }
    for (index, reachability) in report.proposed_rule_reachability.iter().enumerate() {
        if reachability.reachable != expected_reachable[index] {
            return Err(format!(
                "reduced reachability mismatch for rule {:?}",
                reachability.rule_id
            ));
        }
    }
    verify_report(old, proposed, report)?;
    Ok(packet_count)
}

fn generated_full_pair(
    random: &mut SplitMix64,
    pair_index: usize,
) -> (ValidatedPolicy, ValidatedPolicy) {
    let count = 2 + (random.next_u64() % 10) as usize;
    let mut old_inputs = Vec::with_capacity(count);
    for rule_index in 0..count {
        old_inputs.push(random_full_rule(
            random,
            format!("f{pair_index}-{rule_index}"),
        ));
    }
    let mut proposed_inputs = old_inputs.clone();
    let position = (random.next_u64() as usize) % proposed_inputs.len();
    proposed_inputs[position].action = match proposed_inputs[position].action {
        Action::Allow => Action::Deny,
        Action::Deny => Action::Allow,
    };
    (
        validated_from_inputs(old_inputs),
        validated_from_inputs(proposed_inputs),
    )
}

fn random_full_rule(random: &mut SplitMix64, id: String) -> RuleInput {
    let family = if random.next_u64() & 1 == 0 {
        IpFamily::Ipv4
    } else {
        IpFamily::Ipv6
    };
    let prefix = if family == IpFamily::Ipv4 {
        (random.next_u64() % 33) as u8
    } else {
        (random.next_u64() % 129) as u8
    };
    let start = canonical_start(random.next_u128(), family, prefix);
    let first = random.next_u64() as u16;
    let second = random.next_u64() as u16;
    RuleInput {
        id,
        action: if random.next_u64() & 1 == 0 {
            Action::Allow
        } else {
            Action::Deny
        },
        ip_family: family,
        source_cidr: format!("{}/{}", format_address(family, start), prefix),
        protocol: if random.next_u64() & 1 == 0 {
            Protocol::Tcp
        } else {
            Protocol::Udp
        },
        port_start: first.min(second),
        port_end: first.max(second),
    }
}

fn verify_boundary_probes(
    old: &ValidatedPolicy,
    proposed: &ValidatedPolicy,
    report: &Report,
    probe_count: usize,
    random: &mut SplitMix64,
) -> Result<(), String> {
    let mut address_candidates = vec![
        (IpFamily::Ipv4, 0),
        (IpFamily::Ipv4, u32::MAX as u128),
        (IpFamily::Ipv6, 0),
        (IpFamily::Ipv6, u128::MAX),
    ];
    let mut port_candidates = vec![0, u16::MAX];
    for rule in old.rules.iter().chain(&proposed.rules) {
        for address in [rule.source_start, rule.source_end] {
            address_candidates.push((rule.family, address));
            if address > 0 {
                address_candidates.push((rule.family, address - 1));
            }
            if address < rule.family.maximum() {
                address_candidates.push((rule.family, address + 1));
            }
        }
        for port in [rule.port_start, rule.port_end] {
            port_candidates.push(port);
            if port > 0 {
                port_candidates.push(port - 1);
            }
            if port < u16::MAX {
                port_candidates.push(port + 1);
            }
        }
    }
    for probe_index in 0..probe_count {
        let (family, source) = if probe_index % 3 == 0 {
            address_candidates[(random.next_u64() as usize) % address_candidates.len()]
        } else if random.next_u64() & 1 == 0 {
            (IpFamily::Ipv4, random.next_u64() as u32 as u128)
        } else {
            (IpFamily::Ipv6, random.next_u128())
        };
        let destination_port = if probe_index % 3 == 0 {
            port_candidates[(random.next_u64() as usize) % port_candidates.len()]
        } else {
            random.next_u64() as u16
        };
        let protocol = if random.next_u64() & 1 == 0 {
            Protocol::Tcp
        } else {
            Protocol::Udp
        };
        let packet = Packet {
            family,
            source,
            protocol,
            destination_port,
        };
        let old_decision = evaluate_linear(old, packet);
        let new_decision = evaluate_linear(proposed, packet);
        let containing: Vec<_> = report
            .changes
            .iter()
            .filter(|region| region_contains(region, packet))
            .collect();
        let expected_change = old_decision.action != new_decision.action;
        if containing.len() != usize::from(expected_change) {
            return Err(format!(
                "full-width probe mismatch for packet {:?}: expected_change={expected_change} containing_regions={}",
                packet,
                containing.len()
            ));
        }
        if let Some(region) = containing.first()
            && (region.old_decision != old_decision.action
                || region.old_rule_id != old_decision.rule_id
                || region.new_decision != new_decision.action
                || region.new_rule_id != new_decision.rule_id)
        {
            return Err(format!(
                "full-width decision mismatch for packet {packet:?}"
            ));
        }
    }
    verify_report(old, proposed, report)?;
    Ok(())
}

pub fn region_contains(region: &ChangeRegion, packet: Packet) -> bool {
    packet.family == region.ip_family
        && packet.protocol == region.protocol
        && region.source_start_num <= packet.source
        && packet.source <= region.source_end_num
        && region.port_start <= packet.destination_port
        && packet.destination_port <= region.port_end
}

#[derive(Clone)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }

    fn next_u128(&mut self) -> u128 {
        ((self.next_u64() as u128) << 64) | self.next_u64() as u128
    }
}

pub fn diagnostics_for_errors(errors: &[String]) -> String {
    let mut result = String::new();
    for error in errors {
        let _ = writeln!(result, "{error}");
    }
    result
}
