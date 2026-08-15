# Incremental calculation cache trial

This package is the retained, discardable prototype for the 2026-08-15 BogKit developer simulation. Its decision is **no fit for the full financial-planning brief**.

The bounded proof uses Fold to persist a generation marker and calculated cells atomically. It also checks range-dependency discovery, exact `i128` arithmetic, deterministic error ordering, journal validation, one-way initialization, authoritative-journal recovery, and immutable application-level snapshot publication.

Important execution limit: the adapter computes an affected-cell list, but it still fully evaluates the entire tiny workbook and republishes every calculated cell after each accepted batch. It is not an incremental recalculation performance proof.

## Exact offline verification

Run these commands from the repository root. Build and runtime state stays outside the package.

```sh
mkdir -p /private/tmp/financial-snapshot-runtime

CARGO_TARGET_DIR=/private/tmp/financial-snapshot-target \
TMPDIR=/private/tmp/financial-snapshot-runtime \
cargo test --offline --locked --manifest-path developer-simulation/Cargo.toml \
  --package financial-snapshot-trial --all-targets

CARGO_TARGET_DIR=/private/tmp/financial-snapshot-target \
TMPDIR=/private/tmp/financial-snapshot-runtime \
cargo clippy --offline --locked --manifest-path developer-simulation/Cargo.toml \
  --package financial-snapshot-trial --all-targets -- \
  -D warnings -D clippy::all -D clippy::pedantic

cargo fmt --check --manifest-path developer-simulation/Cargo.toml \
  --package financial-snapshot-trial
```

These commands use the single archived workspace and lockfile.

## Runnable release demonstration

```sh
CARGO_TARGET_DIR=/private/tmp/financial-snapshot-target \
TMPDIR=/private/tmp/financial-snapshot-runtime \
cargo run --offline --locked --release \
  --manifest-path developer-simulation/Cargo.toml \
  --package financial-snapshot-trial --bin financial-snapshot-trial -- \
  /private/tmp/financial-snapshot-data \
  /private/tmp/financial-snapshot-result.json
```

The demo removes `/private/tmp/financial-snapshot-data` itself. After verification, remove the remaining external build/runtime state:

```sh
rm -rf /private/tmp/financial-snapshot-target \
  /private/tmp/financial-snapshot-runtime \
  /private/tmp/financial-snapshot-result.json
```

## Valid lifecycle

1. `Trial::open` is only for an unused calculated-state directory.
2. `bootstrap` may run exactly once.
3. Accepted encoded records remain in the caller-owned authoritative journal.
4. After a process restart, call `Trial::recover_from_journal` with the original workbook and the complete accepted journal prefix. Replay occurs without publication and must exactly equal the persisted calculated snapshot before writes are enabled.
5. Continue with the next strictly increasing journal sequence.

Repeated initialization, opening an already-used store as new, incomplete replay, mismatched replay, malformed records, and out-of-order sequences are rejected without replacing the last persisted snapshot.

## Evidence limits

The retained fixture has seven cells and three accepted batches. Its recovery-continuation regression includes a formula replacement and compares uninterrupted execution with close/recover/continue. The demo runs twelve reader tasks and 24,000 requests.

The production evaluator, million-cell fixture, 10,000-batch trace, structural row edits, raw database-byte determinism, one/four-worker comparison, 200 process exits, corrupt-snapshot rebuild, disk-full injection, 30-minute concurrency run, one-million-read run, memory cap, and production performance gates were not available or not run. Logical canonical bytes—not raw Fold database bytes—are compared. Concurrent application reads use an adapter-owned immutable snapshot, not a directly clonable Fold reader.

See [`DISCOVERY.md`](DISCOVERY.md), [`result-manifest.json`](result-manifest.json),
and the [daily report](../../reports/2026-08-15.md) for the complete decision audit.
