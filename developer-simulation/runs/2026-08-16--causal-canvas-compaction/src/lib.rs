use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod store;
pub use store::{
    FaultInjection, FaultStage, OpenedGeneration, StoreError, open_latest, publish_generation,
};
mod suite;
pub use suite::{SuiteSummary, run_reduced_oracle_suite};

pub const OFFLINE_WINDOW_MINUTES: u64 = 30 * 24 * 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconnectDecision {
    CatchUp,
    SnapshotRequired,
}

#[must_use]
pub fn reconnect_decision(now_minute: u64, acknowledged_minute: u64) -> ReconnectDecision {
    if now_minute.saturating_sub(acknowledged_minute) <= OFFLINE_WINDOW_MINUTES {
        ReconnectDecision::CatchUp
    } else {
        ReconnectDecision::SnapshotRequired
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Limits {
    pub max_line_bytes: usize,
    pub max_payload_bytes: usize,
    pub max_pending_operations: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_line_bytes: 70 * 1024,
            max_payload_bytes: 64 * 1024,
            max_pending_operations: 10_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationError {
    InputTooLarge,
    InvalidUtf8,
    InvalidJson,
    UnknownOperationKind,
    PayloadTooLarge,
    InvalidSequence,
    SequenceOverflow,
}

/// Validates and decodes one bounded NDJSON operation line.
///
/// # Errors
///
/// Returns a stable validation category for oversized, malformed, unknown, or
/// invalid-sequence input.
pub fn validate_ndjson_line(bytes: &[u8], limits: Limits) -> Result<Operation, ValidationError> {
    if bytes.len() > limits.max_line_bytes {
        return Err(ValidationError::InputTooLarge);
    }
    let line = std::str::from_utf8(bytes).map_err(|_| ValidationError::InvalidUtf8)?;
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|_| ValidationError::InvalidJson)?;
    let kind = value
        .pointer("/payload/kind")
        .and_then(serde_json::Value::as_str)
        .ok_or(ValidationError::InvalidJson)?;
    if !matches!(
        kind,
        "create" | "set_property" | "move" | "list_insert" | "list_delete" | "object_delete"
    ) {
        return Err(ValidationError::UnknownOperationKind);
    }
    let operation: Operation =
        serde_json::from_value(value).map_err(|_| ValidationError::InvalidJson)?;
    let payload_size = serde_json::to_vec(&operation.payload)
        .map_err(|_| ValidationError::InvalidJson)?
        .len();
    if payload_size > limits.max_payload_bytes {
        return Err(ValidationError::PayloadTooLarge);
    }
    if operation.actor_sequence == 0 {
        return Err(ValidationError::InvalidSequence);
    }
    if operation.actor_sequence == u64::MAX {
        return Err(ValidationError::SequenceOverflow);
    }
    Ok(operation)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Operation {
    pub id: String,
    pub document_id: String,
    pub actor_id: String,
    pub actor_sequence: u64,
    pub dependency_clock: BTreeMap<String, u64>,
    pub accepted_minute: u64,
    pub payload: Payload,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    Create {
        object_id: String,
        object_kind: String,
    },
    SetProperty {
        object_id: String,
        key: String,
        value: String,
    },
    Move {
        object_id: String,
        x: i64,
        y: i64,
    },
    ListInsert {
        list_id: String,
        element_id: String,
        left: Option<String>,
        value: String,
    },
    ListDelete {
        list_id: String,
        element_id: String,
    },
    ObjectDelete {
        object_id: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    Applied { drained: usize },
    Buffered,
    Duplicate,
    Rejected(ApplyError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyError {
    ConflictingOperationId,
    InvalidSequence,
    SequenceOverflow,
    PendingLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingDiagnosis {
    pub pending_operations: usize,
    pub has_cycle: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientWatermark {
    pub client_id: String,
    pub document_id: String,
    pub acknowledged: BTreeMap<String, u64>,
    pub acknowledged_minute: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionArtifact {
    pub snapshot_json: String,
    pub retained_operation_ids: Vec<String>,
    pub reconnect_decisions: BTreeMap<String, ReconnectDecision>,
    pub manifest_json: String,
    pub uncompacted_bytes: usize,
    pub compacted_bytes: usize,
    state_json: String,
    retained_json: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompactionError {
    PendingOperations,
    Serialization(String),
}

impl fmt::Display for CompactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PendingOperations => formatter.write_str("causally pending operations remain"),
            Self::Serialization(error) => write!(formatter, "serialization error: {error}"),
        }
    }
}

impl std::error::Error for CompactionError {}

impl From<serde_json::Error> for CompactionError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value.to_string())
    }
}

#[derive(Serialize)]
struct CompactionManifest<'a> {
    document_id: &'a str,
    snapshot_digest: String,
    retained_digest: String,
    retained_operation_ids: &'a [String],
    reconnect_decisions: &'a BTreeMap<String, ReconnectDecision>,
    state_digest: String,
}

#[derive(Deserialize, Serialize)]
struct PersistedCore {
    frontiers: Vec<FrontierEntry>,
    documents: BTreeMap<String, Document>,
    seen: BTreeMap<String, String>,
    limits: Limits,
}

#[derive(Deserialize, Serialize)]
struct FrontierEntry {
    document_id: String,
    actor_id: String,
    sequence: u64,
}

#[derive(Default)]
pub struct Core {
    frontiers: BTreeMap<(String, String), u64>,
    pending: Vec<Operation>,
    documents: BTreeMap<String, Document>,
    seen: BTreeMap<String, String>,
    limits: Limits,
    history: Vec<Operation>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct Document {
    objects: BTreeMap<String, Object>,
    deleted_objects: BTreeSet<String>,
    lists: BTreeMap<String, List>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Object {
    kind: String,
    x: i64,
    y: i64,
    properties: BTreeMap<String, Register>,
    position_writer: Option<Writer>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct List {
    elements: BTreeMap<String, ListElement>,
    tombstones: BTreeSet<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct ListElement {
    left: Option<String>,
    value: String,
    writer: Writer,
}

#[derive(Clone, Deserialize, Serialize)]
struct Register {
    value: String,
    writer: Writer,
}

#[derive(Clone, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct Writer {
    actor_sequence: u64,
    actor_id: String,
    operation_id: String,
    dependency_clock: BTreeMap<String, u64>,
}

#[derive(Serialize)]
struct Snapshot<'a> {
    document_id: &'a str,
    objects: Vec<SnapshotObject<'a>>,
    lists: Vec<SnapshotList<'a>>,
}

#[derive(Serialize)]
struct SnapshotList<'a> {
    id: &'a str,
    elements: Vec<SnapshotListElement<'a>>,
}

#[derive(Serialize)]
struct SnapshotListElement<'a> {
    id: &'a str,
    value: &'a str,
}

#[derive(Serialize)]
struct SnapshotObject<'a> {
    id: &'a str,
    kind: &'a str,
    x: i64,
    y: i64,
    properties: BTreeMap<&'a str, &'a str>,
}

impl Core {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    pub fn apply(&mut self, operation: Operation) -> ApplyOutcome {
        if operation.actor_sequence == 0 {
            return ApplyOutcome::Rejected(ApplyError::InvalidSequence);
        }
        if operation.actor_sequence == u64::MAX {
            return ApplyOutcome::Rejected(ApplyError::SequenceOverflow);
        }
        let fingerprint = operation_fingerprint(&operation);
        if let Some(seen) = self.seen.get(&operation.id) {
            return if seen == &fingerprint {
                ApplyOutcome::Duplicate
            } else {
                ApplyOutcome::Rejected(ApplyError::ConflictingOperationId)
            };
        }

        if !self.is_ready(&operation) {
            let current = self
                .frontiers
                .get(&(operation.document_id.clone(), operation.actor_id.clone()))
                .copied()
                .unwrap_or(0);
            if operation.actor_sequence <= current {
                return ApplyOutcome::Rejected(ApplyError::InvalidSequence);
            }
            if self.pending.len() >= self.limits.max_pending_operations {
                return ApplyOutcome::Rejected(ApplyError::PendingLimit);
            }
            self.seen.insert(operation.id.clone(), fingerprint);
            self.history.push(operation.clone());
            self.pending.push(operation);
            return ApplyOutcome::Buffered;
        }

        self.seen.insert(operation.id.clone(), fingerprint);
        self.history.push(operation.clone());
        self.apply_ready(operation);
        let mut drained = 0;
        while let Some(index) = self.pending.iter().position(|op| self.is_ready(op)) {
            let ready = self.pending.remove(index);
            self.apply_ready(ready);
            drained += 1;
        }
        ApplyOutcome::Applied { drained }
    }

    /// Applies a caller-selected batch without making batch boundaries part of
    /// operation ordering or canonical output.
    pub fn apply_batch(
        &mut self,
        operations: impl IntoIterator<Item = Operation>,
    ) -> Vec<ApplyOutcome> {
        operations
            .into_iter()
            .map(|operation| self.apply(operation))
            .collect()
    }

    /// Returns byte-stable canonical visible state for one document.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if the in-memory state cannot be encoded.
    pub fn canonical_snapshot(&self, document_id: &str) -> Result<String, serde_json::Error> {
        let objects = self
            .documents
            .get(document_id)
            .map(|document| {
                document
                    .objects
                    .iter()
                    .map(|(id, object)| SnapshotObject {
                        id,
                        kind: &object.kind,
                        x: object.x,
                        y: object.y,
                        properties: object
                            .properties
                            .iter()
                            .map(|(key, register)| (key.as_str(), register.value.as_str()))
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let lists = self
            .documents
            .get(document_id)
            .map(|document| {
                document
                    .lists
                    .iter()
                    .map(|(id, list)| SnapshotList {
                        id,
                        elements: ordered_visible_elements(list),
                    })
                    .collect()
            })
            .unwrap_or_default();
        serde_json::to_string(&Snapshot {
            document_id,
            objects,
            lists,
        })
    }

    #[must_use]
    pub fn accepted_count(&self) -> usize {
        self.seen.len()
    }

    /// Digests the canonical snapshot bytes using the prototype's stable digest.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if the canonical snapshot cannot be encoded.
    pub fn snapshot_digest(&self, document_id: &str) -> Result<String, serde_json::Error> {
        self.canonical_snapshot(document_id)
            .map(|snapshot| stable_digest(snapshot.as_bytes()))
    }

    #[must_use]
    pub fn pending_diagnosis(&self) -> PendingDiagnosis {
        PendingDiagnosis {
            pending_operations: self.pending.len(),
            has_cycle: self.pending_has_cycle(),
        }
    }

    /// Produces a deterministic compacted artifact for one causally complete document.
    ///
    /// # Errors
    ///
    /// Returns `PendingOperations` if accepted work is still blocked, or a
    /// serialization error while producing artifact bytes.
    pub fn compact(
        &self,
        document_id: &str,
        clients: &[ClientWatermark],
        now_minute: u64,
    ) -> Result<CompactionArtifact, CompactionError> {
        if self
            .pending
            .iter()
            .any(|operation| operation.document_id == document_id)
        {
            return Err(CompactionError::PendingOperations);
        }
        let mut relevant_clients: Vec<_> = clients
            .iter()
            .filter(|client| client.document_id == document_id)
            .collect();
        relevant_clients.sort_by(|a, b| a.client_id.cmp(&b.client_id));

        let reconnect_decisions: BTreeMap<_, _> = relevant_clients
            .iter()
            .map(|client| {
                (
                    client.client_id.clone(),
                    reconnect_decision(now_minute, client.acknowledged_minute),
                )
            })
            .collect();
        let supported_clients: Vec<_> = relevant_clients
            .iter()
            .copied()
            .filter(|client| {
                reconnect_decision(now_minute, client.acknowledged_minute)
                    == ReconnectDecision::CatchUp
            })
            .collect();

        let document_history: Vec<_> = self
            .history
            .iter()
            .filter(|operation| operation.document_id == document_id)
            .collect();
        let retained: Vec<_> = document_history
            .iter()
            .copied()
            .filter(|operation| {
                supported_clients.iter().any(|client| {
                    client
                        .acknowledged
                        .get(&operation.actor_id)
                        .copied()
                        .unwrap_or(0)
                        < operation.actor_sequence
                })
            })
            .collect();
        let retained_operation_ids: Vec<_> = retained
            .iter()
            .map(|operation| operation.id.clone())
            .collect();
        let snapshot_json = self.canonical_snapshot(document_id)?;
        let retained_json = serde_json::to_vec(&retained)?;
        let persisted = PersistedCore {
            frontiers: self
                .frontiers
                .iter()
                .filter(|((document, _), _)| document == document_id)
                .map(|((document, actor), sequence)| FrontierEntry {
                    document_id: document.clone(),
                    actor_id: actor.clone(),
                    sequence: *sequence,
                })
                .collect(),
            documents: self
                .documents
                .get(document_id)
                .map(|document| BTreeMap::from([(document_id.to_string(), document.clone())]))
                .unwrap_or_default(),
            seen: self.seen.clone(),
            limits: self.limits,
        };
        let state_json = serde_json::to_string(&persisted)?;
        let manifest_json = serde_json::to_string(&CompactionManifest {
            document_id,
            snapshot_digest: stable_digest(snapshot_json.as_bytes()),
            retained_digest: stable_digest(&retained_json),
            retained_operation_ids: &retained_operation_ids,
            reconnect_decisions: &reconnect_decisions,
            state_digest: stable_digest(state_json.as_bytes()),
        })?;
        let uncompacted_bytes = document_history
            .iter()
            .map(|operation| serde_json::to_vec(operation).map(|bytes| bytes.len() + 1))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .sum();
        let compacted_bytes = state_json.len() + retained_json.len() + manifest_json.len() + 64;

        Ok(CompactionArtifact {
            snapshot_json,
            retained_operation_ids,
            reconnect_decisions,
            manifest_json,
            uncompacted_bytes,
            compacted_bytes,
            state_json,
            retained_json,
        })
    }

    fn is_ready(&self, operation: &Operation) -> bool {
        let expected = self
            .frontiers
            .get(&(operation.document_id.clone(), operation.actor_id.clone()))
            .copied()
            .unwrap_or(0)
            + 1;
        operation.actor_sequence == expected
            && operation.dependency_clock.iter().all(|(actor, sequence)| {
                self.frontiers
                    .get(&(operation.document_id.clone(), actor.clone()))
                    .copied()
                    .unwrap_or(0)
                    >= *sequence
            })
    }

    fn apply_ready(&mut self, operation: Operation) {
        let writer = Writer {
            actor_sequence: operation.actor_sequence,
            actor_id: operation.actor_id.clone(),
            operation_id: operation.id.clone(),
            dependency_clock: operation.dependency_clock.clone(),
        };
        let document = self
            .documents
            .entry(operation.document_id.clone())
            .or_default();
        match operation.payload {
            Payload::Create {
                object_id,
                object_kind,
            } => {
                if !document.deleted_objects.contains(&object_id) {
                    document.objects.entry(object_id).or_insert(Object {
                        kind: object_kind,
                        x: 0,
                        y: 0,
                        properties: BTreeMap::new(),
                        position_writer: None,
                    });
                }
            }
            Payload::SetProperty {
                object_id,
                key,
                value,
            } => {
                if let Some(object) = document.objects.get_mut(&object_id) {
                    let replace = object
                        .properties
                        .get(&key)
                        .is_none_or(|current| writer_wins(&writer, &current.writer));
                    if replace {
                        object.properties.insert(key, Register { value, writer });
                    }
                }
            }
            Payload::Move { object_id, x, y } => {
                if let Some(object) = document.objects.get_mut(&object_id) {
                    let replace = object
                        .position_writer
                        .as_ref()
                        .is_none_or(|current| writer_wins(&writer, current));
                    if replace {
                        object.x = x;
                        object.y = y;
                        object.position_writer = Some(writer);
                    }
                }
            }
            Payload::ListInsert {
                list_id,
                element_id,
                left,
                value,
            } => {
                let list = document.lists.entry(list_id).or_default();
                if !list.tombstones.contains(&element_id) {
                    let replace = list
                        .elements
                        .get(&element_id)
                        .is_none_or(|current| writer_wins(&writer, &current.writer));
                    if replace {
                        list.elements.insert(
                            element_id,
                            ListElement {
                                left,
                                value,
                                writer,
                            },
                        );
                    }
                }
            }
            Payload::ListDelete {
                list_id,
                element_id,
            } => {
                let list = document.lists.entry(list_id).or_default();
                list.tombstones.insert(element_id);
            }
            Payload::ObjectDelete { object_id } => {
                document.objects.remove(&object_id);
                document.deleted_objects.insert(object_id);
            }
        }
        self.frontiers.insert(
            (operation.document_id, operation.actor_id),
            operation.actor_sequence,
        );
    }

    fn pending_has_cycle(&self) -> bool {
        let mut edges = vec![Vec::new(); self.pending.len()];
        for (index, operation) in self.pending.iter().enumerate() {
            for (actor, sequence) in &operation.dependency_clock {
                let frontier = self
                    .frontiers
                    .get(&(operation.document_id.clone(), actor.clone()))
                    .copied()
                    .unwrap_or(0);
                if frontier >= *sequence {
                    continue;
                }
                if let Some(dependency) = self.pending.iter().position(|candidate| {
                    candidate.document_id == operation.document_id
                        && candidate.actor_id == *actor
                        && candidate.actor_sequence == *sequence
                }) {
                    edges[index].push(dependency);
                }
            }
        }

        graph_has_cycle(&edges)
    }
}

impl CompactionArtifact {
    /// Reopens compacted state, retained catch-up history, and permanent identity fingerprints.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if either artifact part is malformed.
    pub fn reopen(&self) -> Result<Core, serde_json::Error> {
        core_from_serialized(&self.state_json, &self.retained_json)
    }
}

fn graph_has_cycle(edges: &[Vec<usize>]) -> bool {
    fn visit(node: usize, edges: &[Vec<usize>], marks: &mut [u8]) -> bool {
        if marks[node] == 1 {
            return true;
        }
        if marks[node] == 2 {
            return false;
        }
        marks[node] = 1;
        if edges[node].iter().any(|next| visit(*next, edges, marks)) {
            return true;
        }
        marks[node] = 2;
        false
    }

    let mut marks = vec![0; edges.len()];
    (0..edges.len()).any(|node| visit(node, edges, &mut marks))
}

fn core_from_serialized(state_json: &str, retained_json: &[u8]) -> Result<Core, serde_json::Error> {
    let persisted: PersistedCore = serde_json::from_str(state_json)?;
    let retained: Vec<Operation> = serde_json::from_slice(retained_json)?;
    Ok(Core {
        frontiers: persisted
            .frontiers
            .into_iter()
            .map(|entry| ((entry.document_id, entry.actor_id), entry.sequence))
            .collect(),
        pending: Vec::new(),
        documents: persisted.documents,
        seen: persisted.seen,
        limits: persisted.limits,
        history: retained,
    })
}

fn operation_fingerprint(operation: &Operation) -> String {
    let canonical = serde_json::to_vec(operation).unwrap_or_default();
    stable_digest(&canonical)
}

fn ordered_visible_elements(list: &List) -> Vec<SnapshotListElement<'_>> {
    fn walk<'a>(
        list: &'a List,
        children: &BTreeMap<Option<&'a str>, Vec<&'a str>>,
        roots: impl IntoIterator<Item = &'a str>,
        visited: &mut BTreeSet<&'a str>,
        output: &mut Vec<SnapshotListElement<'a>>,
    ) {
        let mut stack: Vec<_> = roots.into_iter().collect();
        stack.reverse();
        while let Some(id) = stack.pop() {
            if visited.insert(id) {
                let element = &list.elements[id];
                if !list.tombstones.contains(id) {
                    output.push(SnapshotListElement {
                        id,
                        value: &element.value,
                    });
                }
                if let Some(next) = children.get(&Some(id)) {
                    stack.extend(next.iter().rev().copied());
                }
            }
        }
    }

    let mut children: BTreeMap<Option<&str>, Vec<&str>> = BTreeMap::new();
    for (id, element) in &list.elements {
        children
            .entry(element.left.as_deref())
            .or_default()
            .push(id);
    }
    let mut visited = BTreeSet::new();
    let mut output = Vec::new();
    walk(
        list,
        &children,
        children.get(&None).into_iter().flatten().copied(),
        &mut visited,
        &mut output,
    );
    for id in list.elements.keys() {
        if !visited.contains(id.as_str()) {
            walk(
                list,
                &children,
                std::iter::once(id.as_str()),
                &mut visited,
                &mut output,
            );
        }
    }
    output
}

fn writer_wins(candidate: &Writer, current: &Writer) -> bool {
    if candidate
        .dependency_clock
        .get(&current.actor_id)
        .copied()
        .unwrap_or(0)
        >= current.actor_sequence
    {
        return true;
    }
    if current
        .dependency_clock
        .get(&candidate.actor_id)
        .copied()
        .unwrap_or(0)
        >= candidate.actor_sequence
    {
        return false;
    }
    candidate > current
}

#[must_use]
pub fn stable_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
