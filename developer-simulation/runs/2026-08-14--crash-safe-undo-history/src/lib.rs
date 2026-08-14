#![allow(clippy::missing_errors_doc)]

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

pub mod baseline;
pub mod candidate;
pub mod fixture;
pub mod runner;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Object {
    #[serde(with = "u128_string")]
    pub id: u128,
    pub z: i64,
    pub transform: [i64; 6],
    pub visible: bool,
    pub fill: u32,
    pub points: Vec<(i64, i64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    Create {
        object: Object,
    },
    Delete {
        #[serde(with = "u128_string")]
        id: u128,
    },
    Move {
        #[serde(with = "u128_string")]
        id: u128,
        dx: i64,
        dy: i64,
    },
    Recolor {
        #[serde(with = "u128_string")]
        id: u128,
        fill: u32,
    },
    ToggleVisibility {
        #[serde(with = "u128_string")]
        id: u128,
    },
    InsertPoint {
        #[serde(with = "u128_string")]
        id: u128,
        index: usize,
        point: (i64, i64),
    },
    MovePoint {
        #[serde(with = "u128_string")]
        id: u128,
        index: usize,
        dx: i64,
        dy: i64,
    },
    RemovePoint {
        #[serde(with = "u128_string")]
        id: u128,
        index: usize,
    },
    Reorder {
        #[serde(with = "u128_string")]
        id: u128,
        z: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub commands: Vec<Command>,
}

impl Group {
    #[must_use]
    pub fn new(id: impl Into<String>, commands: Vec<Command>) -> Self {
        Self {
            id: id.into(),
            commands,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Commit { group: Group },
    Undo { count: usize },
    Redo { count: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitOutcome {
    pub group_id: String,
    pub sequence: u64,
    pub document_digest: String,
    pub undo_count: usize,
    pub redo_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryStatus {
    pub document_digest: String,
    pub active_head: Option<u64>,
    pub undo_count: usize,
    pub redo_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelError {
    code: String,
    message: String,
}

impl ModelError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
}

impl Display for ModelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ModelError {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    objects: BTreeMap<u128, Object>,
}

impl Document {
    #[must_use]
    pub fn objects(&self) -> &BTreeMap<u128, Object> {
        &self.objects
    }

    pub fn canonical_json(&self) -> Result<String, serde_json::Error> {
        #[derive(Serialize)]
        struct Canonical<'a> {
            objects: Vec<&'a Object>,
        }
        serde_json::to_string(&Canonical {
            objects: self.objects.values().collect(),
        })
    }

    pub fn digest(&self) -> Result<String, serde_json::Error> {
        Ok(fnv1a64(self.canonical_json()?.as_bytes()))
    }

    fn apply(&mut self, command: &Command) -> Result<Command, ModelError> {
        match command {
            Command::Create { object } => {
                if self.objects.contains_key(&object.id) {
                    return Err(ModelError::new(
                        "object_exists",
                        format!("object {} already exists", object.id),
                    ));
                }
                self.objects.insert(object.id, object.clone());
                Ok(Command::Delete { id: object.id })
            }
            Command::Delete { id } => {
                let object = self.objects.remove(id).ok_or_else(|| missing(*id))?;
                Ok(Command::Create { object })
            }
            Command::Move { id, dx, dy } => {
                let object = self.objects.get(id).ok_or_else(|| missing(*id))?;
                let arithmetic =
                    checked_translation((object.transform[4], object.transform[5]), (*dx, *dy))?;
                let object = self.objects.get_mut(id).ok_or_else(|| missing(*id))?;
                object.transform[4] = arithmetic.destination.0;
                object.transform[5] = arithmetic.destination.1;
                Ok(Command::Move {
                    id: *id,
                    dx: arithmetic.inverse.0,
                    dy: arithmetic.inverse.1,
                })
            }
            Command::Recolor { id, fill } => {
                let object = self.objects.get_mut(id).ok_or_else(|| missing(*id))?;
                let old = std::mem::replace(&mut object.fill, *fill);
                Ok(Command::Recolor { id: *id, fill: old })
            }
            Command::ToggleVisibility { id } => {
                let object = self.objects.get_mut(id).ok_or_else(|| missing(*id))?;
                object.visible = !object.visible;
                Ok(Command::ToggleVisibility { id: *id })
            }
            Command::InsertPoint { id, index, point } => {
                let object = self.objects.get_mut(id).ok_or_else(|| missing(*id))?;
                if *index > object.points.len() || object.points.len() == 32 {
                    return Err(ModelError::new(
                        "invalid_point_index",
                        format!("cannot insert point {index}"),
                    ));
                }
                object.points.insert(*index, *point);
                Ok(Command::RemovePoint {
                    id: *id,
                    index: *index,
                })
            }
            Command::MovePoint { id, index, dx, dy } => {
                let object = self.objects.get(id).ok_or_else(|| missing(*id))?;
                let point = object.points.get(*index).ok_or_else(|| {
                    ModelError::new(
                        "invalid_point_index",
                        format!("point {index} does not exist"),
                    )
                })?;
                let arithmetic = checked_translation(*point, (*dx, *dy))?;
                let object = self.objects.get_mut(id).ok_or_else(|| missing(*id))?;
                let point = object.points.get_mut(*index).ok_or_else(|| {
                    ModelError::new(
                        "invalid_point_index",
                        format!("point {index} does not exist"),
                    )
                })?;
                *point = arithmetic.destination;
                Ok(Command::MovePoint {
                    id: *id,
                    index: *index,
                    dx: arithmetic.inverse.0,
                    dy: arithmetic.inverse.1,
                })
            }
            Command::RemovePoint { id, index } => {
                let object = self.objects.get_mut(id).ok_or_else(|| missing(*id))?;
                if *index >= object.points.len() {
                    return Err(ModelError::new(
                        "invalid_point_index",
                        format!("point {index} does not exist"),
                    ));
                }
                let point = object.points.remove(*index);
                Ok(Command::InsertPoint {
                    id: *id,
                    index: *index,
                    point,
                })
            }
            Command::Reorder { id, z } => {
                let object = self.objects.get_mut(id).ok_or_else(|| missing(*id))?;
                let old = std::mem::replace(&mut object.z, *z);
                Ok(Command::Reorder { id: *id, z: old })
            }
        }
    }
}

struct TranslationArithmetic {
    destination: (i64, i64),
    inverse: (i64, i64),
}

fn checked_translation(
    current: (i64, i64),
    delta: (i64, i64),
) -> Result<TranslationArithmetic, ModelError> {
    let inverse = (
        delta.0.checked_neg().ok_or_else(|| {
            ModelError::new("coordinate_overflow", "x translation inverse overflow")
        })?,
        delta.1.checked_neg().ok_or_else(|| {
            ModelError::new("coordinate_overflow", "y translation inverse overflow")
        })?,
    );
    let destination = (
        current
            .0
            .checked_add(delta.0)
            .ok_or_else(|| ModelError::new("coordinate_overflow", "x translation overflow"))?,
        current
            .1
            .checked_add(delta.1)
            .ok_or_else(|| ModelError::new("coordinate_overflow", "y translation overflow"))?,
    );
    Ok(TranslationArithmetic {
        destination,
        inverse,
    })
}

fn missing(id: u128) -> ModelError {
    ModelError::new("missing_object", format!("object {id} does not exist"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    sequence: u64,
    group: Group,
    inverse: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DedupEntry {
    canonical_commands: Vec<u8>,
    diagnostic_fingerprint: String,
    outcome: CommitOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceModel {
    document: Document,
    history: Vec<HistoryEntry>,
    cursor: usize,
    dedup: BTreeMap<String, DedupEntry>,
    next_sequence: u64,
}

impl Default for ReferenceModel {
    fn default() -> Self {
        Self {
            document: Document::default(),
            history: Vec::new(),
            cursor: 0,
            dedup: BTreeMap::new(),
            next_sequence: 1,
        }
    }
}

impl ReferenceModel {
    pub fn commit(&mut self, group: Group) -> Result<CommitOutcome, ModelError> {
        self.commit_with_admission(group)
            .map(|(outcome, _is_new)| outcome)
    }

    pub(crate) fn commit_with_admission(
        &mut self,
        group: Group,
    ) -> Result<(CommitOutcome, bool), ModelError> {
        if group.commands.is_empty() || group.commands.len() > 20 {
            return Err(ModelError::new(
                "invalid_group_size",
                "a group must contain 1 to 20 commands",
            ));
        }
        let canonical_commands = canonical_commands(&group.commands)?;
        if let Some(old) = self.dedup.get(&group.id) {
            return if old.canonical_commands == canonical_commands {
                Ok((old.outcome.clone(), false))
            } else {
                Err(ModelError::new(
                    "group_id_conflict",
                    format!("group {} was already used with different content", group.id),
                ))
            };
        }

        let before = self.document.clone();
        let mut inverse = Vec::with_capacity(group.commands.len());
        for command in &group.commands {
            match self.document.apply(command) {
                Ok(command) => inverse.push(command),
                Err(error) => {
                    self.document = before;
                    return Err(error);
                }
            }
        }

        self.history.truncate(self.cursor);
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.history.push(HistoryEntry {
            sequence,
            group: group.clone(),
            inverse,
        });
        self.cursor = self.history.len();
        let outcome = CommitOutcome {
            group_id: group.id.clone(),
            sequence,
            document_digest: self.document.digest().map_err(json_error)?,
            undo_count: self.undo_count(),
            redo_count: self.redo_count(),
        };
        let diagnostic_fingerprint = fnv1a64(&canonical_commands);
        self.dedup.insert(
            group.id,
            DedupEntry {
                canonical_commands,
                diagnostic_fingerprint,
                outcome: outcome.clone(),
            },
        );
        Ok((outcome, true))
    }

    pub fn apply_action(&mut self, action: &Action) -> Result<HistoryStatus, ModelError> {
        match action {
            Action::Commit { group } => {
                self.commit(group.clone())?;
                self.status()
            }
            Action::Undo { count } => self.undo(*count),
            Action::Redo { count } => self.redo(*count),
        }
    }

    pub fn undo(&mut self, count: usize) -> Result<HistoryStatus, ModelError> {
        if count > self.cursor {
            return Err(ModelError::new(
                "undo_beyond_history",
                format!("cannot undo {count} groups"),
            ));
        }
        for _ in 0..count {
            let entry = &self.history[self.cursor - 1];
            for inverse in entry.inverse.iter().rev() {
                self.document.apply(inverse).map_err(|error| {
                    ModelError::new("internal_inverse_error", error.to_string())
                })?;
            }
            self.cursor -= 1;
        }
        self.status()
    }

    pub fn redo(&mut self, count: usize) -> Result<HistoryStatus, ModelError> {
        if count > self.redo_count() {
            return Err(ModelError::new(
                "redo_beyond_history",
                format!("cannot redo {count} groups"),
            ));
        }
        for _ in 0..count {
            let entry = &self.history[self.cursor];
            for command in &entry.group.commands {
                self.document
                    .apply(command)
                    .map_err(|error| ModelError::new("internal_redo_error", error.to_string()))?;
            }
            self.cursor += 1;
        }
        self.status()
    }

    pub fn status(&self) -> Result<HistoryStatus, ModelError> {
        Ok(HistoryStatus {
            document_digest: self.document.digest().map_err(json_error)?,
            active_head: self
                .cursor
                .checked_sub(1)
                .map(|index| self.history[index].sequence),
            undo_count: self.undo_count(),
            redo_count: self.redo_count(),
        })
    }

    #[must_use]
    pub fn undo_count(&self) -> usize {
        self.cursor
    }

    #[must_use]
    pub fn redo_count(&self) -> usize {
        self.history.len() - self.cursor
    }

    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn canonical_json(&self) -> Result<String, serde_json::Error> {
        self.document.canonical_json()
    }

    #[must_use]
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    pub fn compact(&mut self, keep: usize) {
        if self.history.len() <= keep {
            return;
        }
        let drop_count = (self.history.len() - keep).min(self.cursor);
        if drop_count == 0 {
            return;
        }
        self.history.drain(..drop_count);
        self.cursor -= drop_count;
    }
}

fn canonical_commands(commands: &[Command]) -> Result<Vec<u8>, ModelError> {
    serde_json::to_vec(commands).map_err(json_error)
}

fn json_error(error: impl Display) -> ModelError {
    ModelError::new("serialization_error", error.to_string())
}

#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

mod u128_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{value:032x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        let value = String::deserialize(deserializer)?;
        u128::from_str_radix(&value, 16).map_err(serde::de::Error::custom)
    }
}
