use std::fs;
use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::baseline::Baseline;
use crate::candidate::Candidate;
use crate::fixture::{Fixture, seed_model};
use crate::{Action, Command, Group, HistoryStatus, ModelError, ReferenceModel};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingMetrics {
    pub sample_count: usize,
    pub p50_commit_micros: u64,
    pub p95_commit_micros: u64,
    pub worst_commit_micros: u64,
    pub reopen_micros: u64,
    pub final_disk_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    pub canonical_json: String,
    pub reference_status: HistoryStatus,
    pub candidate_status: HistoryStatus,
    pub baseline_status: HistoryStatus,
    pub checkpoints_verified: usize,
    pub transcript_digest: String,
    pub candidate: TimingMetrics,
    pub baseline: TimingMetrics,
    pub peak_resident_bytes: Option<u64>,
    pub declared_limits_evaluated: bool,
}

#[allow(clippy::too_many_lines)]
pub fn run_fixture(
    fixture: &Fixture,
    directory: impl AsRef<Path>,
) -> Result<RunReport, ModelError> {
    let directory = directory.as_ref();
    fs::create_dir_all(directory).map_err(io_error)?;
    let candidate_path = directory.join("candidate.fold");
    let baseline_path = directory.join("baseline");
    if candidate_path.exists() || baseline_path.exists() {
        return Err(ModelError::new(
            "output_exists",
            "run output paths must not already exist",
        ));
    }

    let mut reference = ReferenceModel::default();
    seed_model(&mut reference, fixture)?;
    let mut candidate = Candidate::open(&candidate_path)?;
    let mut baseline = Baseline::open(&baseline_path, 2_000)?;
    for (chunk_index, chunk) in fixture.initial_objects.chunks(20).enumerate() {
        let group = Group::new(
            format!("seed-{}-initial-{chunk_index}", fixture.seed),
            chunk
                .iter()
                .cloned()
                .map(|object| Command::Create { object })
                .collect(),
        );
        candidate.commit(group.clone())?;
        baseline.commit(group)?;
    }

    let mut candidate_latencies = Vec::with_capacity(fixture.actions.len());
    let mut baseline_latencies = Vec::with_capacity(fixture.actions.len());
    let mut transcript_hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut checkpoints_verified = 0_usize;
    let mut checkpoint_index = 0_usize;
    for (index, action) in fixture.actions.iter().enumerate() {
        let is_edit_group = matches!(action, Action::Commit { .. });
        let expected = reference.apply_action(action)?;
        feed_hash(
            &mut transcript_hash,
            &serde_json::to_vec(&expected).map_err(json_error)?,
        );

        let started = Instant::now();
        let candidate_status = apply_candidate(&mut candidate, action)?;
        let candidate_elapsed = micros(started.elapsed());
        if is_edit_group {
            candidate_latencies.push(candidate_elapsed);
        }
        if candidate_status != expected {
            return Err(ModelError::new(
                "candidate_mismatch",
                format!("candidate diverged at submitted action {}", index + 1),
            ));
        }

        let started = Instant::now();
        let baseline_status = apply_baseline(&mut baseline, action)?;
        let baseline_elapsed = micros(started.elapsed());
        if is_edit_group {
            baseline_latencies.push(baseline_elapsed);
        }
        if baseline_status != expected {
            return Err(ModelError::new(
                "baseline_mismatch",
                format!("baseline diverged at submitted action {}", index + 1),
            ));
        }

        if fixture
            .checkpoints
            .get(checkpoint_index)
            .is_some_and(|checkpoint| checkpoint.at_action == index + 1)
        {
            let checkpoint = &fixture.checkpoints[checkpoint_index];
            if checkpoint.status != expected {
                return Err(ModelError::new(
                    "fixture_checkpoint_mismatch",
                    format!(
                        "fixture checkpoint {} is inconsistent",
                        checkpoint.at_action
                    ),
                ));
            }
            checkpoint_index += 1;
            checkpoints_verified += 1;
        }
    }

    let reference_status = reference.status()?;
    let canonical_json = reference.canonical_json().map_err(json_error)?;
    let candidate_status = candidate.status()?;
    let baseline_status = baseline.status()?;
    drop(candidate);
    drop(baseline);

    let started = Instant::now();
    let reopened_candidate = Candidate::open(&candidate_path)?;
    let candidate_reopen = micros(started.elapsed());
    if reopened_candidate.status()? != reference_status {
        return Err(ModelError::new(
            "candidate_reopen_mismatch",
            "candidate changed after reopen",
        ));
    }
    drop(reopened_candidate);

    let started = Instant::now();
    let reopened_baseline = Baseline::open(&baseline_path, 2_000)?;
    let baseline_reopen = micros(started.elapsed());
    if reopened_baseline.status()? != reference_status {
        return Err(ModelError::new(
            "baseline_reopen_mismatch",
            "baseline changed after reopen",
        ));
    }

    Ok(RunReport {
        canonical_json,
        reference_status,
        candidate_status,
        baseline_status,
        checkpoints_verified,
        transcript_digest: format!("{transcript_hash:016x}"),
        candidate: summarize(
            &mut candidate_latencies,
            candidate_reopen,
            directory_size(&candidate_path)?,
        ),
        baseline: summarize(
            &mut baseline_latencies,
            baseline_reopen,
            directory_size(&baseline_path)?,
        ),
        peak_resident_bytes: None,
        declared_limits_evaluated: fixture.initial_objects.len() == 60_000
            && fixture.actions.len() == 250_000,
    })
}

fn apply_candidate(store: &mut Candidate, action: &Action) -> Result<HistoryStatus, ModelError> {
    match action {
        Action::Commit { group } => {
            store.commit(group.clone())?;
            store.status()
        }
        Action::Undo { count } => store.undo(*count),
        Action::Redo { count } => store.redo(*count),
    }
}

fn apply_baseline(store: &mut Baseline, action: &Action) -> Result<HistoryStatus, ModelError> {
    match action {
        Action::Commit { group } => {
            store.commit(group.clone())?;
            store.status()
        }
        Action::Undo { count } => store.undo(*count),
        Action::Redo { count } => store.redo(*count),
    }
}

fn summarize(latencies: &mut [u64], reopen_micros: u64, final_disk_bytes: u64) -> TimingMetrics {
    latencies.sort_unstable();
    let percentile = |percent: usize| -> u64 {
        if latencies.is_empty() {
            0
        } else {
            latencies[((latencies.len() - 1) * percent) / 100]
        }
    };
    TimingMetrics {
        sample_count: latencies.len(),
        p50_commit_micros: percentile(50),
        p95_commit_micros: percentile(95),
        worst_commit_micros: latencies.last().copied().unwrap_or(0),
        reopen_micros,
        final_disk_bytes,
    }
}

fn feed_hash(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn directory_size(path: &Path) -> Result<u64, ModelError> {
    if path.is_file() {
        return fs::metadata(path)
            .map(|metadata| metadata.len())
            .map_err(io_error);
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let child = entry.path();
        total += if child.is_dir() {
            directory_size(&child)?
        } else {
            entry.metadata().map_err(io_error)?.len()
        };
    }
    Ok(total)
}

fn io_error(error: impl std::fmt::Display) -> ModelError {
    ModelError::new("io_error", error.to_string())
}

fn json_error(error: impl std::fmt::Display) -> ModelError {
    ModelError::new("serialization_error", error.to_string())
}
