# Causal canvas compaction experiment

This is a deliberately reduced Rust replay and generation-publication core for the Trial 1 brief. It is runnable and independently tested, but it is not a production replacement for the TypeScript reducer or PostgreSQL authority.

The fit decision is **keep the baseline; adopt no BogKit component for this compaction core**. Fold is useful for persistent incremental views, but its generic keyed upsert is arrival-ordered unless the application first supplies the canvas-specific causal winner. ESE and ANNy solve embedding and approximate-search problems that are absent here.

## What the prototype covers

- the six declared operation kinds;
- per-document/per-actor sequence and dependency buffering;
- exact duplicate no-op and conflicting operation-ID rejection, including after compaction, publication, and reopen;
- a declared reduced causal LWW rule for scalar properties and moves;
- permanent object and list-element tombstones;
- deterministic ordered-list traversal even when an anchor is deleted;
- bounded NDJSON lines, payloads, pending operations, UTF-8, JSON, kind, and sequence errors;
- exact 30-day reconnect decisions and watermark-based retained history;
- deterministic canonical JSON, SHA-256 payload digests, and a separately stored SHA-256 digest over every manifest byte;
- complete-generation local publication, fsync, atomic rename, retry, and validated fallback;
- ordinary in-process returned errors after five completed publication stages;
- a CLI replay path from the declared input-directory shape.

## Important limits

The production TypeScript implementation and its canonical fixtures were not supplied. The reduced LWW tie-break here is: causal descendants win; concurrent writes compare `(actor_sequence, actor_id, operation_id)`. That rule must not be assumed to match production.

The randomized oracle covers a scalar subset with a hand-derived expected winner. The full 5,000,000-operation corpus was not generated or replayed: the fresh post-fix 500,000-operation representative run consumed 440,688 KiB RSS because this prototype retains accepted operations and identity metadata in memory. The recipe is preserved under `fixtures/`, with the missing corpus digest explicitly `null`.

The returned-error test is not a crash test: it does not spawn or kill a child process, inject a real disk-full or permission failure, interrupt partial writes or the `CURRENT` temporary-write/flush/rename/final-sync sequence, or test process/host crash and power loss. `crash_schedules.json` is parsed and shape-validated by the replay CLI but is not executed. PostgreSQL coordination, multi-process writers, the real TypeScript oracle, media payloads, keyed authentication, and Byzantine behavior also remain outside this prototype.

The permanent SHA-256 operation fingerprints needed to reject conflicting ID reuse make the corrected synthetic compacted artifact 3,719/8,670 bytes (42.90%). The brief's 40% retained-byte gate therefore fails; this is a no-fit result, not production readiness.

## Dependencies

Production dependencies are `serde`, `serde_json`, and `sha2`. The only development-only component dependency is local `fold`, used by one characterization test. ESE and ANNy are not linked. The package uses the lab's single nested workspace and lockfile.

## Run

Run these commands from the repository root. Build and runtime state stays
outside the checkout:

```console
cargo fmt --manifest-path developer-simulation/Cargo.toml \
  --package causal-canvas-compaction -- --check
CARGO_TARGET_DIR=/private/tmp/causal-canvas-target \
  cargo test --offline --locked --manifest-path developer-simulation/Cargo.toml \
  --package causal-canvas-compaction --all-targets
CARGO_TARGET_DIR=/private/tmp/causal-canvas-target \
  cargo clippy --offline --locked --manifest-path developer-simulation/Cargo.toml \
  --package causal-canvas-compaction --all-targets -- \
  -D warnings -D clippy::all -D clippy::pedantic
CARGO_TARGET_DIR=/private/tmp/causal-canvas-target \
  cargo build --release --offline --locked \
  --manifest-path developer-simulation/Cargo.toml \
  --package causal-canvas-compaction
```

Run the 50-history, 20-schedule reduced oracle:

```console
/private/tmp/causal-canvas-target/release/causal-canvas-compaction \
  oracle 50 20 64
```

Run a representative sequential workload and report 200-operation batch p95:

```console
/private/tmp/causal-canvas-target/release/causal-canvas-compaction \
  workload 500000
```

Run the largest declared visible-document shape. Use a new empty state path:

```console
/private/tmp/causal-canvas-target/release/causal-canvas-compaction \
  largest 40000 20000 /private/tmp/causal-largest-state
```

Run the directory-input integration fixture:

```console
/private/tmp/causal-canvas-target/release/causal-canvas-compaction replay \
  developer-simulation/runs/2026-08-16--causal-canvas-compaction/fixtures/smoke-input \
  /private/tmp/causal-smoke-state \
  canvas \
  10
```

The replay input directory contains `operations.ndjson`, `watermarks.json`, and `crash_schedules.json`. The replay command only parses and shape-validates the crash-schedule file. The publication tests request ordinary returned errors after five completed stages; they do not execute crash schedules or simulate actual I/O failures.

## Main library boundary

- `validate_ndjson_line`: bounded input decode and validation.
- `Core::apply` / `Core::apply_batch`: identity, causal buffering, and visible-state reduction.
- `Core::compact`: reconnect decisions, retained operations, permanent compact operation fingerprints, state, and deterministic manifest.
- `CompactionArtifact::reopen`: in-memory reopen from compacted bytes.
- `publish_generation` / `open_latest`: replaceable local generation directory.
- `Core::canonical_snapshot`: deterministic visible JSON.

See the [daily report](../../reports/2026-08-16.md) for the decision audit and
`evidence/` for preserved command results.
