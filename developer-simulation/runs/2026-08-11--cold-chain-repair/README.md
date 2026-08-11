# Cold-chain repair trial

This is a disposable prototype for rebuilding freezer excursion summaries from an authoritative NDJSON archive. It deliberately evaluates BogKit rather than assuming it is the right answer.

## Verdict

**No fit for the production reducer.** Fold provided a useful crash-safe, idempotent keyed index of accepted records, but the time-ordered correction/configuration replay, validation, exact provenance, and canonical snapshot publication all remained application code. The independent full-replay baseline was simpler and faster. The Fold-backed prototype also exceeded the 384 MiB memory limit at one million observations.

ESE and ANNy were considered and rejected because deterministic temperature repair is neither an embedding nor nearest-neighbor problem.

## What is implemented

- External-tagged observation and configuration NDJSON records.
- Whole-archive validation for duplicate IDs, conflicting duplicates, invalid timestamps and bands, unknown/cross-freezer/multiple correction targets, correction cycles, and ambiguous configuration boundaries.
- A simple independent reference reducer.
- A separate candidate reducer backed by a disposable Fold `KeyedStream`/`Table` record index.
- Deterministic incident JSON with the triggering and contributing observation IDs and configuration IDs for every open/close transition.
- Actual child-process exits inside a Fold write transaction and immediately before/after canonical snapshot rename.
- A deterministic fixture generator, 100-seed differential verification, and one-million-observation benchmark.

## Commands

Run from the BogKit repository root. Use a scratch directory outside the repository so no runtime state is archived.

```console
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline -p cold-chain-repair -- generate /tmp/cold-chain/archive.ndjson 23 2 96
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline -p cold-chain-repair -- repair /tmp/cold-chain/archive.ndjson /tmp/cold-chain/state
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline -p cold-chain-repair -- demo /tmp/cold-chain/demo
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline -p cold-chain-repair -- verify 100 /tmp/cold-chain/verify
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline -p cold-chain-repair -- fault-demo /tmp/cold-chain/fault
```

Quality gates:

```console
cargo fmt --manifest-path developer-simulation/Cargo.toml -p cold-chain-repair -- --check
cargo test --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p cold-chain-repair --all-targets
cargo clippy --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p cold-chain-repair --all-targets -- -D warnings -D clippy::all -D clippy::pedantic
```

Performance comparison:

```console
cargo build --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p cold-chain-repair
/usr/bin/time -l developer-simulation/target/release/cold-chain-repair benchmark-reference 1000000 /tmp/cold-chain/reference-benchmark
/usr/bin/time -l developer-simulation/target/release/cold-chain-repair benchmark 1000000 /tmp/cold-chain/candidate-benchmark
```

On the declared test machine, the reference took 0.727 seconds inside the reducer (1.19 seconds wall) and peaked at 511,508,480 bytes RSS. The Fold-backed candidate took 3.320 seconds inside the rebuild (3.47 seconds wall) and peaked at 1,155,678,208 bytes RSS. Both violate the 384 MiB target because this prototype holds the generated archive and validation maps in memory; the candidate adds a second record representation plus Fold's index. These measurements demonstrate a failed prototype, not a standalone Fold performance defect.

## Minimal Fold serialization reproducers

Fold's public bounds accept ordinary Serde types, but its Postcard-backed read path panics for two common representations. These commands intentionally exit 101:

```console
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline -p cold-chain-repair --bin repro-postcard-serde -- tagged /tmp/fold-postcard-tagged
# panics in fold/src/pipeline/terminal/table.rs with: Err(WontImplement)

cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline -p cold-chain-repair --bin repro-postcard-serde -- skipped /tmp/fold-postcard-skipped
# panics in fold/src/pipeline/terminal/table.rs with: Err(DeserializeUnexpectedEnd)
```

The working archive type uses an externally tagged enum and always serializes optional fields. A documentation warning would prevent the first surprise; returning a typed error or checking storage compatibility before commit would be safer than a read-time panic.

## Important semantics chosen for the prototype

- A continuous run means consecutive effective readings of the same in/out classification; a long sampling gap does not itself break the run because the brief does not define a maximum gap.
- A transition is recorded on the first reading whose timestamp is at least five or ten minutes after the run's first reading.
- Only the final unreplaced observation in a correction chain is effective. Multiple corrections of the same direct target are rejected as ambiguous.
- Configuration changes at the same freezer and effective timestamp are rejected rather than resolved by arrival order.
- Final canonical output records open/close causes, not the historical sequence of repair operations, because output must be byte-identical regardless of batch boundaries.
