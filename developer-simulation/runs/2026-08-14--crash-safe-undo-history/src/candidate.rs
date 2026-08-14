//! Fold-backed candidate adapter.
//!
//! The entire logical state is one typed Fold table value. This deliberately
//! small adapter exercises Fold's central atomic transaction, durable commit,
//! and snapshot read behavior. It is not claimed to be the production data
//! layout for the full 60,000-object workload.

use std::path::{Path, PathBuf};
use std::{fs, fs::File};

use fold::pipeline::{Keyed, terminal};
use fold::stream::Stream;

use crate::{CommitOutcome, Group, HistoryStatus, ModelError, ReferenceModel};

type StateStream = Stream<Keyed<u8, Vec<u8>>, terminal::Table<u8, Vec<u8>>>;

pub struct Candidate {
    path: PathBuf,
    stream: StateStream,
    model: ReferenceModel,
}

impl Candidate {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ModelError> {
        let path = path.as_ref().to_path_buf();
        let stream: StateStream = std::panic::catch_unwind({
            let path = path.clone();
            move || {
                Stream::<Keyed<u8, Vec<u8>>, terminal::Table<u8, Vec<u8>>>::new(
                    path,
                    terminal::Table::<u8, Vec<u8>>::new("undo_state"),
                )
            }
        })
        .map_err(|_| ModelError::new("store_open_failed", "Fold could not open the store"))?;
        let model = stream
            .rtx(|state| state.get(&0))
            .map_or_else(|| Ok(ReferenceModel::default()), |bytes| decode(&bytes))?;
        Ok(Self {
            path,
            stream,
            model,
        })
    }

    pub fn commit(&mut self, group: Group) -> Result<CommitOutcome, ModelError> {
        let before = self.model.clone();
        let (outcome, is_new) = self.model.commit_with_admission(group)?;
        if !is_new {
            return Ok(outcome);
        }
        if let Err(error) = self.persist() {
            self.model = before;
            return Err(error);
        }
        Ok(outcome)
    }

    pub fn undo(&mut self, count: usize) -> Result<HistoryStatus, ModelError> {
        let before = self.model.clone();
        let status = self.model.undo(count)?;
        if let Err(error) = self.persist() {
            self.model = before;
            return Err(error);
        }
        Ok(status)
    }

    pub fn redo(&mut self, count: usize) -> Result<HistoryStatus, ModelError> {
        let before = self.model.clone();
        let status = self.model.redo(count)?;
        if let Err(error) = self.persist() {
            self.model = before;
            return Err(error);
        }
        Ok(status)
    }

    pub fn compact(&mut self, keep: usize) -> Result<HistoryStatus, ModelError> {
        let before = self.model.clone();
        self.model.compact(keep);
        if let Err(error) = self.persist() {
            self.model = before;
            return Err(error);
        }
        self.status()
    }

    pub fn status(&self) -> Result<HistoryStatus, ModelError> {
        self.stream.rtx(|state| state.get(&0)).map_or_else(
            || ReferenceModel::default().status(),
            |bytes| decode(&bytes)?.status(),
        )
    }

    pub fn canonical_json(&self) -> Result<String, ModelError> {
        self.stream.rtx(|state| state.get(&0)).map_or_else(
            || {
                ReferenceModel::default()
                    .canonical_json()
                    .map_err(json_error)
            },
            |bytes| decode(&bytes)?.canonical_json().map_err(json_error),
        )
    }

    #[must_use]
    pub fn model(&self) -> &ReferenceModel {
        &self.model
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn persist(&mut self) -> Result<(), ModelError> {
        let bytes = serde_json::to_vec(&self.model).map_err(json_error)?;
        self.stream.wtx(|tx| tx.insert(&Keyed::new(0, bytes)));
        self.stream.checkpoint();
        Ok(())
    }
}

pub fn compact_directory(path: impl AsRef<Path>, keep: usize) -> Result<HistoryStatus, ModelError> {
    let path = path.as_ref();
    let parent = path.parent().ok_or_else(|| {
        ModelError::new(
            "invalid_store_path",
            "candidate store needs a parent directory",
        )
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            ModelError::new(
                "invalid_store_path",
                "candidate store name is not valid UTF-8",
            )
        })?;
    let temporary = parent.join(format!("{name}.compact.tmp"));
    let backup = parent.join(format!("{name}.compact.backup"));
    if temporary.exists() || backup.exists() {
        return Err(ModelError::new(
            "compaction_artifact_exists",
            "remove or recover the existing compaction artifact first",
        ));
    }

    let old = Candidate::open(path)?;
    let mut model = old.model.clone();
    model.compact(keep);
    drop(old);
    {
        let mut replacement = Candidate::open(&temporary)?;
        replacement.model = model;
        replacement.persist()?;
    }

    fs::rename(path, &backup).map_err(io_error)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::rename(&backup, path);
        return Err(io_error(error));
    }
    sync_directory(parent)?;
    fs::remove_dir_all(&backup).map_err(io_error)?;
    sync_directory(parent)?;
    Candidate::open(path)?.status()
}

fn decode(bytes: &[u8]) -> Result<ReferenceModel, ModelError> {
    serde_json::from_slice(bytes).map_err(json_error)
}

fn json_error(error: impl std::fmt::Display) -> ModelError {
    ModelError::new("serialization_error", error.to_string())
}

fn io_error(error: impl std::fmt::Display) -> ModelError {
    ModelError::new("io_error", error.to_string())
}

fn sync_directory(path: &Path) -> Result<(), ModelError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}
