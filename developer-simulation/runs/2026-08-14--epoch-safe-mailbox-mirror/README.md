# Epoch-safe mailbox mirror laboratory

This is an isolated, synthetic comparison harness for Trial 2. It contains:

- a pure Rust reference model;
- a faithful SQLite baseline using the host's `/usr/bin/sqlite3`;
- a deterministic framed-NDJSON generator;
- `generate`, `apply`, `resume`, `verify`, and `summarize` commands; and
- a decisive Fold transaction reproducer.

No real mail, addresses, subjects, bodies, attachments, credentials, or network access are used. The SQLite mirror is disposable derived state.

## Build and test

From the repository root with the nested lab workspace:

```console
cargo test --manifest-path developer-simulation/Cargo.toml --offline --locked --package mailbox-mirror-lab --all-targets
cargo fmt --manifest-path developer-simulation/Cargo.toml --package mailbox-mirror-lab -- --check
cargo clippy --manifest-path developer-simulation/Cargo.toml --offline --locked --package mailbox-mirror-lab --all-targets --no-deps -- \
  -D warnings -D clippy::all -D clippy::pedantic
```

Run the Fold transaction-boundary reproducer:

```console
cargo test --manifest-path developer-simulation/Cargo.toml --offline --locked --package mailbox-mirror-lab --test fold_reproducer -- --nocapture
```

The first test characterizes documented behavior: returning `Err` normally from a Fold write closure commits writes already made in that closure. The second characterizes the separate, already-tracked current-main correctness defect: a panic rolls back the rejected row, but after it is caught, a later write on the same stream panics with `poisoned tx lock`. Panic/catch-unwind is therefore not a safe application rollback strategy. This trial does not edit Fold core.

## Exact integer domain and delete cleanup

The reference and SQLite models accept every unsigned field only in the inclusive range `0..=9_223_372_036_854_775_807` (`i64::MAX`). This covers UID validity, UID, cursor, byte size, and batch sequence. Both models reject a larger value with `integer outside SQLite INTEGER range` before batch admission or any state mutation. Boundary and atomic-nonmutation regressions are in `tests/fault_matrix.rs`.

Deleting a stable mailbox ID explicitly removes its `staging_messages` rows inside the same admitted SQLite transaction before deleting the mailbox. The regressions in `tests/sqlite_baseline.rs` cover reopen, two delete/recreate lifecycles under the same stable ID, incomplete staging, and a rejected rescan batch.

## Small CLI demonstration

Use an ordinary user-writable temporary directory and remove it afterwards:

```console
mkdir -p /tmp/mailbox-mirror-demo
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package mailbox-mirror-lab -- generate \
  --output /tmp/mailbox-mirror-demo/transcript.ndjson \
  --seed 29 --mailboxes 30 --messages 300 --responses 1200
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package mailbox-mirror-lab -- verify \
  --transcript /tmp/mailbox-mirror-demo/transcript.ndjson \
  --database /tmp/mailbox-mirror-demo/verified.sqlite
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package mailbox-mirror-lab -- resume \
  --transcript /tmp/mailbox-mirror-demo/transcript.ndjson \
  --database /tmp/mailbox-mirror-demo/verified.sqlite
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package mailbox-mirror-lab -- summarize \
  --database /tmp/mailbox-mirror-demo/verified.sqlite
```

`apply` creates a new SQLite mirror without the independent reference comparison:

```console
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package mailbox-mirror-lab -- apply \
  --transcript /tmp/mailbox-mirror-demo/transcript.ndjson \
  --database /tmp/mailbox-mirror-demo/applied.sqlite
```

`verify` compares every generated checkpoint and the final canonical manifest against the pure reference. `resume` replays the entire transcript against an existing mirror and reports the duplicate responses. `summarize` emits the canonical visible manifest.

## Full workload command

The measured full-scale run used:

```console
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package mailbox-mirror-lab -- generate \
  --output /tmp/mailbox-mirror-demo/full.ndjson \
  --seed 43 --mailboxes 80 --messages 300000 --responses 600000
cargo run --manifest-path developer-simulation/Cargo.toml --offline --locked --release --package mailbox-mirror-lab -- verify \
  --transcript /tmp/mailbox-mirror-demo/full.ndjson \
  --database /tmp/mailbox-mirror-demo/full.sqlite
```

On the declared host after the repair, the single run matched the reference in 49.101 seconds. Exact evidence and limitations are in `evidence/full-campaign.txt`.

## Transcript safety and framing

Every accepted response batch has explicit `begin` and `commit` records. Responses remain in memory until a matching commit record arrives. An explicit `disconnect` discards the incomplete batch. The final `end` record carries response and record counts, so truncation at a clean batch boundary is still detected.

Diagnostics contain only a stable mailbox ID, sequence, and fixed diagnostic code. They do not echo event contents or display names.

## Important limits

This is a no-fit experiment, not a proposed mail subsystem. It does not provide protocol parsing, real networking, two-way flag changes, arbitrary disk-corruption repair, or account/UI integration. It does not prove the full 30-seed, 400-process-exit, peak-RSS, write-amplification, or dense-checkpoint gates. It has no safe Fold candidate/resource comparison because the candidate did not cross the correctness threshold. It deliberately retains SQLite rather than presenting an incomplete Fold adapter as a replacement.
