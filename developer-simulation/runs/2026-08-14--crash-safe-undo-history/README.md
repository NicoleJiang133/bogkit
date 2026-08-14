# Undo history persistence lab

This is a deliberately small, offline command-line prototype comparing:

- a Fold-backed candidate that stores the complete undo state as one atomic table value; and
- a checksummed JSONL baseline with a canonical JSON snapshot every 2,000 accepted unique edit groups.

It preserves canonical document JSON as the interchange output. It does not modify BogKit or integrate with an editor.

## Outcome and review state

The bounded outcome is **`local_proof_only_no_production_fit`**. Fold materially supplies an atomic whole-state write and consistent read snapshot, but the tested full-state layout is slower and much larger than the JSONL baseline. This result does not rule out a normalized Fold layout and does not establish a BogKit defect.

Skeptical review round 1 returned `REJECTED_UNTIL_FIXED`. The repaired candidate now:

- physically truncates a recognized incomplete or checksum-invalid final JSONL record to the last valid byte, syncs the file, and syncs its directory before returning from reopen; middle corruption is fatal;
- checks both inverse and destination coordinate arithmetic before changing an object or point, returning stable `coordinate_overflow` errors for `i64::MIN` and destination overflow;
- returns the original outcome for exact retries without appending the baseline, rewriting the candidate, or advancing the unique-commit snapshot interval; and
- persists and compares exact canonical command bytes for deduplication across restart and compaction. The 64-bit digest is diagnostic only.

The scoped re-review approved the repaired prototype for archival with no
remaining Critical, Important, or Minor findings. This is not a production
approval.

## What the tests establish

The 24 integration tests cover atomic command groups, whole-group undo/redo, redo-branch invalidation, exact retries and conflicting ID reuse, reopen, a real child-process exit after durable commit but before response, every truncation byte of a small final JSONL record, physical repair of partial and checksum-invalid final tails followed by a later commit and second reopen, fatal middle corruption, snapshot cadence, object/point x/y overflow and rollback, deterministic fixtures, three-way candidate/baseline/reference comparison, and compaction retention of recent undo, remaining redo, outcomes, and exact dedup content. The three adapters share one semantic transition core, so this is not a fully independent semantic oracle.

This is not the requested production-scale campaign. The 60,000-object/250,000-action run, 30 seeds, 400 process exits, four concurrent readers, 20,000-group retention window, peak resident memory, every crash point, and hard total-time/reopen/disk limits remain unproved. The candidate already fails the bounded p95 and baseline non-regression gates, so scaling this layout would not be operationally useful.

## Reproduce

Run from the repository root with the nested lab workspace and its checked-in
lockfile, without network access:

```console
cargo fmt --manifest-path developer-simulation/Cargo.toml --package undo-history-lab -- --check
cargo clippy --manifest-path developer-simulation/Cargo.toml --offline --locked --package undo-history-lab --all-targets -- -D warnings -D clippy::all -D clippy::pedantic
cargo test --manifest-path developer-simulation/Cargo.toml --offline --locked --package undo-history-lab --all-targets
```

Run the focused review regressions:

```console
cargo test --manifest-path developer-simulation/Cargo.toml --offline --locked --package undo-history-lab --test baseline_recovery --test candidate_contract --test model_contract --test overflow_recovery
```

Generate and compare the bounded 32-object/1,000-action fixture. The output directory must not already contain `candidate.fold` or `baseline`:

```console
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package undo-history-lab -- generate /tmp/undo-fixture.json 9001 32 1000
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package undo-history-lab -- run /tmp/undo-fixture.json /tmp/undo-run
```

The `run` command prints canonical document JSON followed by one JSON report containing both stores' status, edit-submission latency, reopen time, disk use, checkpoint count, and deterministic transcript digest.

Exercise inspection, recovery, and compaction:

```console
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package undo-history-lab -- inspect candidate /tmp/undo-run/candidate.fold
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package undo-history-lab -- inspect baseline /tmp/undo-run/baseline
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package undo-history-lab -- recover candidate /tmp/undo-run/candidate.fold
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package undo-history-lab -- recover baseline /tmp/undo-run/baseline
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package undo-history-lab -- compact candidate /tmp/undo-run/candidate.fold 20000
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package undo-history-lab -- compact baseline /tmp/undo-run/baseline 20000
```

The generator defaults to 60,000 objects and 250,000 submitted actions if its last three arguments are omitted. Fixture generation is supported, but running that full-state candidate at the default scale is not claimed safe or within the declared limits.

## CLI

```text
undo-history-lab generate <fixture.json> [seed] [objects] [actions]
undo-history-lab run <fixture.json> <output-dir>
undo-history-lab recover <candidate|baseline> <store>
undo-history-lab compact <candidate|baseline> <store> [keep]
undo-history-lab inspect <candidate|baseline> <store>
```

`fault-commit` is a test-only hidden mode that exits with status 86 after the durable store call and before any response is printed.

## Layout

- `src/lib.rs`: document commands and reference transitions
- `src/fixture.rs`: deterministic fixture generator
- `src/baseline.rs`: checksummed JSONL, physical tail repair, and snapshots
- `src/candidate.rs`: Fold-backed atomic adapter and directory-rebuild compaction
- `src/runner.rs`: common-workload comparator and metrics
- `src/main.rs`: CLI
- `tests/`: acceptance, recovery, fault, determinism, compaction, overflow, and RED-to-GREEN contract tests
