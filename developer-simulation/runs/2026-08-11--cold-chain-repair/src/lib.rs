use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{BufRead, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use fold::pipeline::terminal;
use fold::stream::KeyedStream;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Record {
    Observation(Observation),
    Configuration(Config),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub observation_id: String,
    pub freezer_id: String,
    pub observed_at: i64,
    pub received_at: i64,
    pub temp_tenths: i32,
    pub gateway_sequence: u64,
    #[serde(default)]
    pub replaces_observation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub configuration_id: String,
    pub freezer_id: String,
    pub effective_at: i64,
    pub min_temp_tenths: i32,
    pub max_temp_tenths: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Export {
    pub freezers: Vec<FreezerExport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreezerExport {
    pub freezer_id: String,
    pub current_status: String,
    pub incidents: Vec<Incident>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incident {
    pub opened: Transition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed: Option<Transition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub action: String,
    pub at: i64,
    pub trigger_observation_id: String,
    pub reason: String,
    pub contributing_observation_ids: Vec<String>,
    pub configuration_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairError(String);

impl RepairError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for RepairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RepairError {}

/// Rebuilds canonical state with the independent full-replay reducer.
///
/// # Errors
/// Returns an error when records conflict, timestamps or limits are invalid,
/// corrections cannot be resolved, or an observation has no effective limit.
pub fn reference_from_records(records: &[Record]) -> Result<Export, RepairError> {
    let validated = validate(records)?;
    reference_reduce(&validated)
}

/// Validates an archive, updates the disposable Fold index, and repairs state.
///
/// # Errors
/// Returns an error for invalid input, an index/archive count mismatch, or an
/// observation without an effective configuration.
pub fn candidate_from_records(
    records: &[Record],
    index_path: &Path,
) -> Result<Export, RepairError> {
    let validated = validate(records)?;
    let mut index = KeyedStream::new(index_path, terminal::Table::new("accepted_records"));
    let observations: Vec<_> = validated.observations.iter().collect();
    for chunk in observations.chunks(4_096) {
        index.wtx(|transaction| {
            for (id, observation) in chunk {
                transaction.upsert(
                    &format!("observation:{id}"),
                    &Record::Observation((*observation).clone()),
                );
            }
        });
    }
    let configurations: Vec<_> = validated.configurations.iter().collect();
    for chunk in configurations.chunks(4_096) {
        index.wtx(|transaction| {
            for (id, configuration) in chunk {
                transaction.upsert(
                    &format!("configuration:{id}"),
                    &Record::Configuration((*configuration).clone()),
                );
            }
        });
    }
    index.checkpoint();

    let indexed_count = index.rtx(|table| table.iter().count());
    let expected_count = validated.observations.len() + validated.configurations.len();
    if indexed_count != expected_count {
        return Err(RepairError::new(format!(
            "disposable Fold index has {indexed_count} records; archive has {expected_count}"
        )));
    }

    candidate_reduce(&validated)
}

/// Serializes a canonical export as compact JSON ending in one newline.
///
/// # Errors
/// Returns an error if serialization fails.
pub fn canonical_json(export: &Export) -> Result<Vec<u8>, RepairError> {
    let mut bytes = serde_json::to_vec(export).map_err(|error| RepairError(error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Reads one externally tagged observation or configuration per line.
///
/// # Errors
/// Returns an error with the line number for I/O failures, malformed JSON,
/// non-integer timestamps, or a record that does not match the archive schema.
pub fn read_ndjson(reader: impl BufRead) -> Result<Vec<Record>, RepairError> {
    let mut records = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line =
            line.map_err(|error| RepairError::new(format!("line {line_number}: {error}")))?;
        let value: serde_json::Value = serde_json::from_str(&line)
            .map_err(|error| RepairError::new(format!("line {line_number}: {error}")))?;
        validate_wire_timestamps(&value, line_number)?;
        let record = serde_json::from_value(value)
            .map_err(|error| RepairError::new(format!("line {line_number}: {error}")))?;
        records.push(record);
    }
    Ok(records)
}

/// Writes one compact archive record per line.
///
/// # Errors
/// Returns an error if JSON serialization or writing fails.
pub fn write_ndjson(records: &[Record], mut writer: impl Write) -> Result<(), RepairError> {
    for record in records {
        serde_json::to_writer(&mut writer, record)
            .map_err(|error| RepairError::new(error.to_string()))?;
        writer
            .write_all(b"\n")
            .map_err(|error| RepairError::new(error.to_string()))?;
    }
    Ok(())
}

fn validate_wire_timestamps(
    value: &serde_json::Value,
    line_number: usize,
) -> Result<(), RepairError> {
    let (record, timestamp_fields): (&serde_json::Value, &[&str]) =
        if let Some(observation) = value.get("observation") {
            (observation, &["observed_at", "received_at"])
        } else if let Some(configuration) = value.get("configuration") {
            (configuration, &["effective_at"])
        } else {
            return Ok(());
        };
    for field in timestamp_fields {
        if !record.get(*field).is_some_and(serde_json::Value::is_i64) {
            return Err(RepairError::new(format!(
                "line {line_number}: {field} must be an integer timestamp"
            )));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct Validated {
    observations: BTreeMap<String, Observation>,
    configurations: BTreeMap<String, Config>,
    replaced_ids: BTreeSet<String>,
}

fn validate(records: &[Record]) -> Result<Validated, RepairError> {
    let mut observations = BTreeMap::<String, Observation>::new();
    let mut configurations = BTreeMap::<String, Config>::new();

    for record in records {
        match record {
            Record::Observation(observation) => {
                validate_observation(observation)?;
                match observations.get(&observation.observation_id) {
                    Some(existing) if existing == observation => {}
                    Some(_) => {
                        return Err(RepairError::new(format!(
                            "conflicting duplicate observation id {}",
                            observation.observation_id
                        )));
                    }
                    None => {
                        observations
                            .insert(observation.observation_id.clone(), observation.clone());
                    }
                }
            }
            Record::Configuration(configuration) => {
                validate_configuration(configuration)?;
                match configurations.get(&configuration.configuration_id) {
                    Some(existing) if existing == configuration => {}
                    Some(_) => {
                        return Err(RepairError::new(format!(
                            "conflicting duplicate configuration id {}",
                            configuration.configuration_id
                        )));
                    }
                    None => {
                        configurations.insert(
                            configuration.configuration_id.clone(),
                            configuration.clone(),
                        );
                    }
                }
            }
        }
    }

    let mut replaced_ids = BTreeSet::new();
    for observation in observations.values() {
        let Some(target_id) = &observation.replaces_observation_id else {
            continue;
        };
        let target = observations
            .get(target_id)
            .ok_or_else(|| RepairError::new(format!("unknown correction target {target_id}")))?;
        if target.freezer_id != observation.freezer_id {
            return Err(RepairError::new(format!(
                "correction {} crosses freezer boundary",
                observation.observation_id
            )));
        }
        if !replaced_ids.insert(target_id.clone()) {
            return Err(RepairError::new(format!(
                "multiple corrections replace observation {target_id}"
            )));
        }
    }

    for id in observations.keys() {
        let mut seen = BTreeSet::new();
        let mut cursor = id.as_str();
        loop {
            if !seen.insert(cursor.to_string()) {
                return Err(RepairError::new(format!(
                    "correction cycle includes observation {cursor}"
                )));
            }
            let Some(next) = observations
                .get(cursor)
                .and_then(|observation| observation.replaces_observation_id.as_deref())
            else {
                break;
            };
            cursor = next;
        }
    }

    let mut effective_slots = BTreeSet::new();
    for config in configurations.values() {
        if !effective_slots.insert((config.freezer_id.clone(), config.effective_at)) {
            return Err(RepairError::new(format!(
                "ambiguous configuration interval for freezer {} at {}",
                config.freezer_id, config.effective_at
            )));
        }
    }

    Ok(Validated {
        observations,
        configurations,
        replaced_ids,
    })
}

fn validate_observation(observation: &Observation) -> Result<(), RepairError> {
    if observation.observation_id.is_empty() || observation.freezer_id.is_empty() {
        return Err(RepairError::new(
            "observation identifiers must not be empty",
        ));
    }
    if observation.observed_at < 0
        || observation.received_at < 0
        || observation.received_at < observation.observed_at
    {
        return Err(RepairError::new(format!(
            "malformed timestamp on observation {}",
            observation.observation_id
        )));
    }
    if observation
        .replaces_observation_id
        .as_ref()
        .is_some_and(std::string::String::is_empty)
    {
        return Err(RepairError::new(format!(
            "empty correction target on observation {}",
            observation.observation_id
        )));
    }
    Ok(())
}

fn validate_configuration(config: &Config) -> Result<(), RepairError> {
    if config.configuration_id.is_empty() || config.freezer_id.is_empty() {
        return Err(RepairError::new(
            "configuration identifiers must not be empty",
        ));
    }
    if config.effective_at < 0 {
        return Err(RepairError::new(format!(
            "malformed timestamp on configuration {}",
            config.configuration_id
        )));
    }
    if config.min_temp_tenths > config.max_temp_tenths {
        return Err(RepairError::new(format!(
            "invalid temperature band on configuration {}",
            config.configuration_id
        )));
    }
    Ok(())
}

fn reference_reduce(validated: &Validated) -> Result<Export, RepairError> {
    let mut by_freezer = BTreeMap::<String, Vec<&Observation>>::new();
    for (id, observation) in &validated.observations {
        if !validated.replaced_ids.contains(id) {
            by_freezer
                .entry(observation.freezer_id.clone())
                .or_default()
                .push(observation);
        }
    }

    let mut configurations = BTreeMap::<String, Vec<&Config>>::new();
    for configuration in validated.configurations.values() {
        configurations
            .entry(configuration.freezer_id.clone())
            .or_default()
            .push(configuration);
    }
    for freezer_configs in configurations.values_mut() {
        freezer_configs.sort_by(|left, right| {
            (left.effective_at, &left.configuration_id)
                .cmp(&(right.effective_at, &right.configuration_id))
        });
    }

    let mut freezers = Vec::with_capacity(by_freezer.len());
    for (freezer_id, mut observations) in by_freezer {
        observations.sort_by(|left, right| {
            (left.observed_at, &left.observation_id)
                .cmp(&(right.observed_at, &right.observation_id))
        });
        let freezer_configs = configurations
            .get(&freezer_id)
            .ok_or_else(|| RepairError::new(format!("no valid limit for freezer {freezer_id}")))?;

        let mut evaluated = Vec::with_capacity(observations.len());
        for observation in observations {
            let config = freezer_configs
                .iter()
                .rev()
                .find(|config| config.effective_at <= observation.observed_at)
                .ok_or_else(|| {
                    RepairError::new(format!(
                        "no valid limit for observation {}",
                        observation.observation_id
                    ))
                })?;
            let outside = observation.temp_tenths < config.min_temp_tenths
                || observation.temp_tenths > config.max_temp_tenths;
            evaluated.push(Evaluated {
                observation,
                config,
                outside,
            });
        }

        freezers.push(reference_replay(&freezer_id, &evaluated));
    }

    Ok(Export { freezers })
}

#[derive(Clone, Copy)]
struct Evaluated<'a> {
    observation: &'a Observation,
    config: &'a Config,
    outside: bool,
}

fn reference_replay(freezer_id: &str, readings: &[Evaluated<'_>]) -> FreezerExport {
    let mut incidents: Vec<Incident> = Vec::new();
    let mut excursion_open = false;
    let mut run: Vec<Evaluated<'_>> = Vec::new();

    for reading in readings {
        let desired_outside = if excursion_open {
            !reading.outside
        } else {
            reading.outside
        };
        if desired_outside {
            run.push(*reading);
        } else {
            run.clear();
        }

        let threshold = if excursion_open { 600 } else { 300 };
        let reached_threshold = run.first().is_some_and(|first| {
            reading.observation.observed_at - first.observation.observed_at >= threshold
        });
        if !reached_threshold {
            continue;
        }

        if excursion_open {
            let closed = transition(
                "close",
                &run,
                "ten continuous minutes inside configured band",
            );
            if let Some(incident) = incidents.last_mut() {
                incident.closed = Some(closed);
            }
        } else {
            incidents.push(Incident {
                opened: transition(
                    "open",
                    &run,
                    "five continuous minutes outside configured band",
                ),
                closed: None,
            });
        }
        excursion_open = !excursion_open;
        run.clear();
    }

    FreezerExport {
        freezer_id: freezer_id.to_string(),
        current_status: if excursion_open {
            "excursion".into()
        } else {
            "normal".into()
        },
        incidents,
    }
}

fn transition(action: &str, run: &[Evaluated<'_>], reason: &str) -> Transition {
    let trigger = run.last().expect("transition requires a non-empty run");
    let mut configuration_ids: Vec<String> = run
        .iter()
        .map(|reading| reading.config.configuration_id.clone())
        .collect();
    configuration_ids.sort();
    configuration_ids.dedup();
    Transition {
        action: action.into(),
        at: trigger.observation.observed_at,
        trigger_observation_id: trigger.observation.observation_id.clone(),
        reason: reason.into(),
        contributing_observation_ids: run
            .iter()
            .map(|reading| reading.observation.observation_id.clone())
            .collect(),
        configuration_ids,
    }
}

#[derive(Clone)]
struct OwnedEvaluation {
    observation_id: String,
    observed_at: i64,
    config_id: String,
    outside: bool,
}

type CandidateTimeline = BTreeMap<(i64, String), OwnedEvaluation>;
type CandidateTimelines = BTreeMap<String, CandidateTimeline>;

enum CandidateMode {
    Normal { outside_run: Vec<OwnedEvaluation> },
    Excursion { inside_run: Vec<OwnedEvaluation> },
}

fn candidate_reduce(validated: &Validated) -> Result<Export, RepairError> {
    let timelines = candidate_timelines(validated)?;
    let mut freezers = Vec::with_capacity(timelines.len());
    for (freezer_id, timeline) in timelines {
        freezers.push(candidate_replay(freezer_id, &timeline));
    }
    Ok(Export { freezers })
}

fn candidate_timelines(validated: &Validated) -> Result<CandidateTimelines, RepairError> {
    let mut config_timelines = BTreeMap::<String, BTreeMap<i64, &Config>>::new();
    for config in validated.configurations.values() {
        config_timelines
            .entry(config.freezer_id.clone())
            .or_default()
            .insert(config.effective_at, config);
    }

    let mut timelines = BTreeMap::<String, BTreeMap<(i64, String), OwnedEvaluation>>::new();
    for (id, observation) in &validated.observations {
        if validated.replaced_ids.contains(id) {
            continue;
        }
        let config = config_timelines
            .get(&observation.freezer_id)
            .and_then(|timeline| timeline.range(..=observation.observed_at).next_back())
            .map(|(_, config)| *config)
            .ok_or_else(|| {
                RepairError::new(format!(
                    "no valid limit for observation {}",
                    observation.observation_id
                ))
            })?;
        timelines
            .entry(observation.freezer_id.clone())
            .or_default()
            .insert(
                (observation.observed_at, observation.observation_id.clone()),
                OwnedEvaluation {
                    observation_id: observation.observation_id.clone(),
                    observed_at: observation.observed_at,
                    config_id: config.configuration_id.clone(),
                    outside: observation.temp_tenths < config.min_temp_tenths
                        || observation.temp_tenths > config.max_temp_tenths,
                },
            );
    }

    Ok(timelines)
}

fn candidate_replay(freezer_id: String, timeline: &CandidateTimeline) -> FreezerExport {
    let mut mode = CandidateMode::Normal {
        outside_run: Vec::new(),
    };
    let mut incidents = Vec::<Incident>::new();

    for reading in timeline.values() {
        match &mut mode {
            CandidateMode::Normal { outside_run } => {
                if !reading.outside {
                    outside_run.clear();
                    continue;
                }
                outside_run.push(reading.clone());
                let elapsed = reading.observed_at
                    - outside_run
                        .first()
                        .expect("reading was just pushed")
                        .observed_at;
                if elapsed >= 300 {
                    incidents.push(Incident {
                        opened: owned_transition(
                            "open",
                            outside_run,
                            "five continuous minutes outside configured band",
                        ),
                        closed: None,
                    });
                    mode = CandidateMode::Excursion {
                        inside_run: Vec::new(),
                    };
                }
            }
            CandidateMode::Excursion { inside_run } => {
                if reading.outside {
                    inside_run.clear();
                    continue;
                }
                inside_run.push(reading.clone());
                let elapsed = reading.observed_at
                    - inside_run
                        .first()
                        .expect("reading was just pushed")
                        .observed_at;
                if elapsed >= 600 {
                    if let Some(incident) = incidents.last_mut() {
                        incident.closed = Some(owned_transition(
                            "close",
                            inside_run,
                            "ten continuous minutes inside configured band",
                        ));
                    }
                    mode = CandidateMode::Normal {
                        outside_run: Vec::new(),
                    };
                }
            }
        }
    }

    FreezerExport {
        freezer_id,
        current_status: match mode {
            CandidateMode::Normal { .. } => "normal".into(),
            CandidateMode::Excursion { .. } => "excursion".into(),
        },
        incidents,
    }
}

fn owned_transition(action: &str, run: &[OwnedEvaluation], reason: &str) -> Transition {
    let trigger = run.last().expect("transition requires a non-empty run");
    let mut configuration_ids: Vec<String> = run
        .iter()
        .map(|reading| reading.config_id.clone())
        .collect();
    configuration_ids.sort();
    configuration_ids.dedup();
    Transition {
        action: action.into(),
        at: trigger.observed_at,
        trigger_observation_id: trigger.observation_id.clone(),
        reason: reason.into(),
        contributing_observation_ids: run
            .iter()
            .map(|reading| reading.observation_id.clone())
            .collect(),
        configuration_ids,
    }
}

/// Creates deterministic observations, limits, a late correction, and a backdated limit.
///
/// # Panics
/// Panics only if the requested fixture dimensions cannot fit the timestamp
/// and sequence integer types on the current platform.
#[must_use]
pub fn generated_fixture(
    seed: u64,
    freezer_count: usize,
    readings_per_freezer: usize,
) -> Vec<Record> {
    let mut rng = Lcg::new(seed);
    let mut records = Vec::with_capacity(freezer_count * (readings_per_freezer + 1) + 8);

    for freezer_number in 0..freezer_count {
        records.push(Record::Configuration(Config {
            configuration_id: format!("cfg-{freezer_number}-base"),
            freezer_id: format!("freezer-{freezer_number:03}"),
            effective_at: 0,
            min_temp_tenths: -220,
            max_temp_tenths: -180,
        }));
    }

    for step in 0..readings_per_freezer {
        for freezer_number in 0..freezer_count {
            let phase = (step + freezer_number * 7) % 64;
            let base_temp = if (8..=25).contains(&phase) {
                -170
            } else {
                -200
            };
            let jitter = i32::try_from(rng.next() % 5).expect("jitter is in range") - 2;
            let observed_at = i64::try_from(step).expect("fixture step fits i64") * 30;
            records.push(Record::Observation(Observation {
                observation_id: format!("o-{freezer_number}-{step}"),
                freezer_id: format!("freezer-{freezer_number:03}"),
                observed_at,
                received_at: observed_at
                    + i64::try_from(rng.next() % 21_601).expect("lateness fits i64"),
                temp_tenths: base_temp + jitter,
                gateway_sequence: u64::try_from(step).expect("fixture step fits u64"),
                replaces_observation_id: None,
            }));
        }
    }

    if freezer_count > 0 && readings_per_freezer > 20 {
        records.push(Record::Configuration(Config {
            configuration_id: "cfg-0-backdated-maintenance".into(),
            freezer_id: "freezer-000".into(),
            effective_at: 20 * 30,
            min_temp_tenths: -220,
            max_temp_tenths: -160,
        }));
    }

    if freezer_count > 1 && readings_per_freezer > 12 {
        records.push(Record::Observation(Observation {
            observation_id: "correction-freezer-1-step-12".into(),
            freezer_id: "freezer-001".into(),
            observed_at: 12 * 30,
            received_at: 12 * 30 + 7 * 86_400,
            temp_tenths: -200,
            gateway_sequence: u64::try_from(readings_per_freezer + 1)
                .expect("fixture size fits u64"),
            replaces_observation_id: Some("o-1-12".into()),
        }));
    }

    if let Some(duplicate) = records
        .iter()
        .find(|record| matches!(record, Record::Observation(_)))
        .cloned()
    {
        records.push(duplicate);
    }
    records
}

/// Returns a deterministic shuffled copy of `records`.
///
/// # Panics
/// Panics only on a platform where a slice index cannot fit in `u64`.
#[must_use]
pub fn shuffled(records: &[Record], seed: u64) -> Vec<Record> {
    let mut output = records.to_vec();
    let mut rng = Lcg::new(seed);
    for upper in (1..output.len()).rev() {
        let modulus = u64::try_from(upper + 1).expect("slice length fits u64");
        let other = usize::try_from(rng.next() % modulus).expect("index fits usize");
        output.swap(upper, other);
    }
    output
}

struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }
}
