use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{CompactionArtifact, Core, core_from_serialized, stable_digest};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultStage {
    GenerationCreated,
    DataFlushed,
    ManifestFlushed,
    GenerationRenamed,
    DirectorySynced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FaultInjection {
    /// Requests an ordinary returned error after the named stage has completed.
    /// This does not terminate the process or synthesize an operating-system I/O failure.
    pub stage: FaultStage,
}

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Injected(FaultInjection),
    InvalidGeneration,
    GenerationConflict,
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "storage I/O error: {error}"),
            Self::Json(error) => write!(formatter, "generation JSON error: {error}"),
            Self::Injected(fault) => write!(
                formatter,
                "injected in-process returned error after {:?}",
                fault.stage
            ),
            Self::InvalidGeneration => formatter.write_str("no complete valid generation"),
            Self::GenerationConflict => {
                formatter.write_str("generation number already has different bytes")
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub struct OpenedGeneration {
    pub generation: u64,
    pub core: Core,
}

/// Publishes one complete local generation and then atomically advances `CURRENT`.
///
/// # Errors
///
/// Returns an I/O, serialization, generation-conflict, or requested in-process error.
pub fn publish_generation(
    root: &Path,
    generation: u64,
    artifact: &CompactionArtifact,
    fault: Option<FaultInjection>,
) -> Result<(), StoreError> {
    fs::create_dir_all(root)?;
    let final_path = generation_path(root, generation);
    let temporary_path = temporary_generation_path(root, generation);

    if final_path.exists() {
        if !files_match(&final_path, artifact)? {
            return Err(StoreError::GenerationConflict);
        }
    } else {
        if temporary_path.exists() {
            fs::remove_dir_all(&temporary_path)?;
        }
        fs::create_dir(&temporary_path)?;
        inject(fault, FaultStage::GenerationCreated)?;

        write_synced(
            &temporary_path.join("state.json"),
            artifact.state_json.as_bytes(),
        )?;
        write_synced(
            &temporary_path.join("retained.json"),
            &artifact.retained_json,
        )?;
        inject(fault, FaultStage::DataFlushed)?;

        write_synced(
            &temporary_path.join("manifest.json"),
            artifact.manifest_json.as_bytes(),
        )?;
        write_synced(
            &temporary_path.join("manifest.sha256"),
            stable_digest(artifact.manifest_json.as_bytes()).as_bytes(),
        )?;
        sync_directory(&temporary_path)?;
        inject(fault, FaultStage::ManifestFlushed)?;

        fs::rename(&temporary_path, &final_path)?;
        inject(fault, FaultStage::GenerationRenamed)?;
        sync_directory(root)?;
        inject(fault, FaultStage::DirectorySynced)?;
    }

    let current_temporary = root.join("CURRENT.tmp");
    if current_temporary.exists() {
        fs::remove_file(&current_temporary)?;
    }
    write_synced(&current_temporary, generation.to_string().as_bytes())?;
    fs::rename(current_temporary, root.join("CURRENT"))?;
    sync_directory(root)?;
    Ok(())
}

/// Opens the generation named by `CURRENT`, falling back to the newest valid generation.
///
/// # Errors
///
/// Returns an I/O error when the store cannot be scanned, or `InvalidGeneration`
/// when no complete generation passes manifest validation.
pub fn open_latest(root: &Path) -> Result<OpenedGeneration, StoreError> {
    if let Ok(current) = fs::read_to_string(root.join("CURRENT"))
        && let Ok(generation) = current.trim().parse::<u64>()
        && let Ok(opened) = open_generation(root, generation)
    {
        return Ok(opened);
    }
    let mut generations = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(number) = name
            .strip_prefix("generation-")
            .and_then(|number| number.parse::<u64>().ok())
        {
            generations.push(number);
        }
    }
    generations.sort_unstable_by(|a, b| b.cmp(a));
    for generation in generations {
        if let Ok(opened) = open_generation(root, generation) {
            return Ok(opened);
        }
    }
    Err(StoreError::InvalidGeneration)
}

fn open_generation(root: &Path, generation: u64) -> Result<OpenedGeneration, StoreError> {
    let path = generation_path(root, generation);
    let state = fs::read_to_string(path.join("state.json"))?;
    let retained = fs::read(path.join("retained.json"))?;
    let manifest = fs::read_to_string(path.join("manifest.json"))?;
    let manifest_digest = fs::read_to_string(path.join("manifest.sha256"))?;
    if manifest_digest.trim() != stable_digest(manifest.as_bytes()) {
        return Err(StoreError::InvalidGeneration);
    }
    let manifest_value: Value = serde_json::from_str(&manifest)?;
    verify_digest(&manifest_value, "state_digest", state.as_bytes())?;
    verify_digest(&manifest_value, "retained_digest", &retained)?;
    let document_id = manifest_value
        .get("document_id")
        .and_then(Value::as_str)
        .ok_or(StoreError::InvalidGeneration)?;
    let core = core_from_serialized(&state, &retained)?;
    let snapshot = core.canonical_snapshot(document_id)?;
    verify_digest(&manifest_value, "snapshot_digest", snapshot.as_bytes())?;
    Ok(OpenedGeneration { generation, core })
}

fn verify_digest(manifest: &Value, field: &str, bytes: &[u8]) -> Result<(), StoreError> {
    let expected = manifest
        .get(field)
        .and_then(Value::as_str)
        .ok_or(StoreError::InvalidGeneration)?;
    if expected == stable_digest(bytes) {
        Ok(())
    } else {
        Err(StoreError::InvalidGeneration)
    }
}

fn files_match(path: &Path, artifact: &CompactionArtifact) -> Result<bool, StoreError> {
    Ok(
        fs::read(path.join("state.json"))? == artifact.state_json.as_bytes()
            && fs::read(path.join("retained.json"))? == artifact.retained_json
            && fs::read(path.join("manifest.json"))? == artifact.manifest_json.as_bytes()
            && fs::read(path.join("manifest.sha256"))?
                == stable_digest(artifact.manifest_json.as_bytes()).as_bytes(),
    )
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), StoreError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn inject(fault: Option<FaultInjection>, stage: FaultStage) -> Result<(), StoreError> {
    if fault.is_some_and(|fault| fault.stage == stage) {
        Err(StoreError::Injected(fault.expect("checked above")))
    } else {
        Ok(())
    }
}

fn generation_path(root: &Path, generation: u64) -> PathBuf {
    root.join(format!("generation-{generation:020}"))
}

fn temporary_generation_path(root: &Path, generation: u64) -> PathBuf {
    root.join(format!("generation-{generation:020}.tmp"))
}
