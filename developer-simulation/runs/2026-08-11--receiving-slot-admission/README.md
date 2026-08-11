# Receiving-slot admission boundary reproducer

This is a deliberately small evaluation artifact, not a production scheduler. It models the warehouse admission rules with logical timestamps, then demonstrates the transaction boundary that prevents Fold from serving as this system's production authority.

The modeled authority covers a 24-door warehouse, holds, confirmation, cancellation, rescheduling, expiry, exact retries, payload-mismatch rejection, stable reason codes, and invariant checks. A four-thread test exercises colliding retries behind a process-local mutex. That test is useful for the domain model only; it is not evidence for PostgreSQL or six independent replicas.

The crash-boundary demo uses real child-process exits around a mocked atomic authoritative commit. The authoritative booking and audit are written together, while Fold is a separate fjall-backed mirror. An exit after the authoritative commit but before `Fold::wtx` leaves the mirror behind. Retry can repair the mirror, but no API observed in BogKit can make both commits one PostgreSQL transaction. The mock is intentionally labeled and cannot substitute for a real PostgreSQL test.

## Run

From the BogKit repository root:

```console
cargo fmt --manifest-path developer-simulation/Cargo.toml -p receiving-slot-admission -- --check
cargo test --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p receiving-slot-admission --all-targets
cargo clippy --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p receiving-slot-admission --all-targets -- -D warnings -D clippy::all -D clippy::pedantic
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p receiving-slot-admission
```

The demo cleans its temporary authority and Fold directories. Cargo build output can be removed with `cargo clean`.

## Interpretation

The result is **no-fit for the production admission authority**. PostgreSQL must remain authoritative and must commit the accepted command and audit record together. Fold owns an embedded `fjall::SingleWriterTxDatabase`; its public constructor takes a filesystem path, and its `wtx` creates and commits that store's transaction internally. The evaluated API exposes no PostgreSQL storage adapter, caller-owned transaction, outbox hook, or prepare/commit protocol.

The local model expires due holds at the injected command timestamp before every new command decision. Exact replays return their stored result without advancing logical time. Regression tests cover capacity release without a sweep, rescheduling after an unrelated expiry, invalid reschedule durations, and a genuinely stale version.

ESE and ANNy are not used because this problem requires exact transactional scheduling, not embeddings or approximate search. A PostgreSQL outbox feeding an optional derived Fold projection might be possible, but it would add another persisted system without improving admission correctness, so this prototype does not recommend it.
