//! Checksummed JSONL plus snapshot reference baseline.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Action, CommitOutcome, Group, HistoryStatus, ModelError, ReferenceModel};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryDiagnostic {
    pub code: Option<String>,
    pub discarded_tail_offset: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct JournalRecord {
    version: u8,
    action_index: u64,
    payload: String,
    crc32: u32,
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u8,
    action_index: u64,
    committed_groups: u64,
    model: ReferenceModel,
}

pub struct Baseline {
    directory: PathBuf,
    model: ReferenceModel,
    action_index: u64,
    committed_groups: u64,
    snapshot_every: u64,
    diagnostic: RecoveryDiagnostic,
}

impl Baseline {
    pub fn open(directory: impl AsRef<Path>, snapshot_every: u64) -> Result<Self, ModelError> {
        if snapshot_every == 0 {
            return Err(ModelError::new(
                "invalid_snapshot_interval",
                "snapshot interval must be positive",
            ));
        }
        let directory = directory.as_ref().to_path_buf();
        fs::create_dir_all(&directory).map_err(io_error)?;
        let snapshot_path = directory.join("snapshot.json");
        let (mut model, mut action_index, mut committed_groups) = if snapshot_path.exists() {
            let bytes = fs::read(&snapshot_path).map_err(io_error)?;
            let snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(json_error)?;
            if snapshot.version != 1 {
                return Err(ModelError::new(
                    "unsupported_snapshot",
                    "snapshot version is not supported",
                ));
            }
            (
                snapshot.model,
                snapshot.action_index,
                snapshot.committed_groups,
            )
        } else {
            (ReferenceModel::default(), 0, 0)
        };

        let journal_path = directory.join("history.jsonl");
        let (diagnostic, repair_offset) = recover_journal(
            &journal_path,
            &mut model,
            &mut action_index,
            &mut committed_groups,
        )?;
        if let Some(offset) = repair_offset {
            truncate_and_sync(&journal_path, offset, &directory)?;
        }

        Ok(Self {
            directory,
            model,
            action_index,
            committed_groups,
            snapshot_every,
            diagnostic,
        })
    }

    pub fn commit(&mut self, group: Group) -> Result<CommitOutcome, ModelError> {
        let mut proposed = self.model.clone();
        let (outcome, is_new) = proposed.commit_with_admission(group.clone())?;
        if !is_new {
            return Ok(outcome);
        }
        self.append(&Action::Commit { group })?;
        self.model = proposed;
        self.after_append()?;
        Ok(outcome)
    }

    pub fn undo(&mut self, count: usize) -> Result<HistoryStatus, ModelError> {
        let mut proposed = self.model.clone();
        let status = proposed.undo(count)?;
        self.append(&Action::Undo { count })?;
        self.model = proposed;
        Ok(status)
    }

    pub fn redo(&mut self, count: usize) -> Result<HistoryStatus, ModelError> {
        let mut proposed = self.model.clone();
        let status = proposed.redo(count)?;
        self.append(&Action::Redo { count })?;
        self.model = proposed;
        Ok(status)
    }

    pub fn compact(&mut self, keep: usize) -> Result<HistoryStatus, ModelError> {
        self.model.compact(keep);
        self.write_snapshot()?;
        let journal = File::create(self.journal_path()).map_err(io_error)?;
        journal.sync_all().map_err(io_error)?;
        sync_directory(&self.directory)?;
        self.model.status()
    }

    pub fn status(&self) -> Result<HistoryStatus, ModelError> {
        self.model.status()
    }

    pub fn canonical_json(&self) -> Result<String, ModelError> {
        self.model.canonical_json().map_err(json_error)
    }

    #[must_use]
    pub fn model(&self) -> &ReferenceModel {
        &self.model
    }

    #[must_use]
    pub fn diagnostic(&self) -> &RecoveryDiagnostic {
        &self.diagnostic
    }

    #[must_use]
    pub fn journal_path(&self) -> PathBuf {
        self.directory.join("history.jsonl")
    }

    fn append(&mut self, action: &Action) -> Result<(), ModelError> {
        let payload = serde_json::to_string(action).map_err(json_error)?;
        let record = JournalRecord {
            version: 1,
            action_index: self.action_index + 1,
            crc32: crc32fast::hash(payload.as_bytes()),
            payload,
        };
        let mut bytes = serde_json::to_vec(&record).map_err(json_error)?;
        bytes.push(b'\n');
        let mut journal = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.journal_path())
            .map_err(io_error)?;
        journal.write_all(&bytes).map_err(io_error)?;
        journal.sync_all().map_err(io_error)?;
        self.action_index += 1;
        if matches!(action, Action::Commit { .. }) {
            self.committed_groups += 1;
        }
        Ok(())
    }

    fn after_append(&self) -> Result<(), ModelError> {
        if self.committed_groups.is_multiple_of(self.snapshot_every) {
            self.write_snapshot()?;
        }
        Ok(())
    }

    fn write_snapshot(&self) -> Result<(), ModelError> {
        let snapshot = Snapshot {
            version: 1,
            action_index: self.action_index,
            committed_groups: self.committed_groups,
            model: self.model.clone(),
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(json_error)?;
        let temporary = self.directory.join("snapshot.json.tmp");
        let final_path = self.directory.join("snapshot.json");
        {
            let mut file = File::create(&temporary).map_err(io_error)?;
            file.write_all(&bytes).map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
        }
        fs::rename(&temporary, &final_path).map_err(io_error)?;
        sync_directory(&self.directory)
    }
}

fn recover_journal(
    journal_path: &Path,
    model: &mut ReferenceModel,
    action_index: &mut u64,
    committed_groups: &mut u64,
) -> Result<(RecoveryDiagnostic, Option<usize>), ModelError> {
    if !journal_path.exists() {
        return Ok((RecoveryDiagnostic::default(), None));
    }
    let bytes = fs::read(journal_path).map_err(io_error)?;
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let Some(relative_end) = bytes[offset..].iter().position(|byte| *byte == b'\n') else {
            return Ok((tail_diagnostic(offset), Some(offset)));
        };
        let end = offset + relative_end;
        let is_final = end + 1 == bytes.len();
        let record = match serde_json::from_slice::<JournalRecord>(&bytes[offset..end]) {
            Ok(record) => record,
            Err(_) if is_final => return Ok((tail_diagnostic(offset), Some(offset))),
            Err(error) => {
                return Err(ModelError::new(
                    "middle_journal_corruption",
                    format!("invalid record at offset {offset}: {error}"),
                ));
            }
        };
        if record.version != 1 || crc32fast::hash(record.payload.as_bytes()) != record.crc32 {
            if is_final {
                return Ok((tail_diagnostic(offset), Some(offset)));
            }
            return Err(ModelError::new(
                "middle_journal_corruption",
                format!("checksum or version failure at offset {offset}"),
            ));
        }
        if record.action_index > *action_index {
            if record.action_index != *action_index + 1 {
                return Err(ModelError::new(
                    "journal_sequence_gap",
                    format!(
                        "expected action index {}, found {}",
                        *action_index + 1,
                        record.action_index
                    ),
                ));
            }
            apply_recovered_record(&record, model, offset, committed_groups)?;
            *action_index = record.action_index;
        }
        offset = end + 1;
    }
    Ok((RecoveryDiagnostic::default(), None))
}

fn apply_recovered_record(
    record: &JournalRecord,
    model: &mut ReferenceModel,
    offset: usize,
    committed_groups: &mut u64,
) -> Result<(), ModelError> {
    let action: Action = serde_json::from_str(&record.payload).map_err(json_error)?;
    let unique_commit = match action {
        Action::Commit { group } => model
            .commit_with_admission(group)
            .map(|(_outcome, is_new)| is_new),
        Action::Undo { count } => model.undo(count).map(|_status| false),
        Action::Redo { count } => model.redo(count).map(|_status| false),
    }
    .map_err(|error| {
        ModelError::new(
            "invalid_journal_action",
            format!("offset {offset}: {error}"),
        )
    })?;
    if unique_commit {
        *committed_groups += 1;
    }
    Ok(())
}

fn tail_diagnostic(offset: usize) -> RecoveryDiagnostic {
    RecoveryDiagnostic {
        code: Some("discarded_invalid_final_record".to_string()),
        discarded_tail_offset: Some(offset as u64),
    }
}

fn truncate_and_sync(
    journal_path: &Path,
    offset: usize,
    directory: &Path,
) -> Result<(), ModelError> {
    let journal = OpenOptions::new()
        .write(true)
        .open(journal_path)
        .map_err(io_error)?;
    let valid_len = u64::try_from(offset)
        .map_err(|error| ModelError::new("journal_offset_overflow", error.to_string()))?;
    journal.set_len(valid_len).map_err(io_error)?;
    journal.sync_all().map_err(io_error)?;
    sync_directory(directory)
}

fn sync_directory(path: &Path) -> Result<(), ModelError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(io_error)
}

fn io_error(error: impl std::fmt::Display) -> ModelError {
    ModelError::new("io_error", error.to_string())
}

fn json_error(error: impl std::fmt::Display) -> ModelError {
    ModelError::new("serialization_error", error.to_string())
}
