# Repair-cafe kiosk Fold trial

This is a deliberately narrow, offline prototype used to decide whether Fold should replace a conventional embedded SQL database for a single-writer tool-lending kiosk.

It stores item and borrower master rows, accepted events by immutable ID, and a per-item current-state/history projection in one Fold database. The application validates references, strict append-only sequence order, and transitions before each single-writer transaction. ESE and ANNy are not selected as application features or declared as direct dependencies: deterministic normalized lookup is enough for this fixture and remains easier to operate under the 256 MiB limit. Current Fold still pulls ANNy into the normal dependency graph transitively; the trial records that as packaging friction.

Run from the `developer-simulation/` workspace root. The archived performance
evidence uses the workspace's ordinary release profile:

```sh
cargo test --offline --locked --release -p repair-cafe-fold-trial --all-targets
cargo fmt -p repair-cafe-fold-trial -- --check
cargo clippy --offline --locked --release -p repair-cafe-fold-trial --all-targets --all-features -- -D warnings
cargo run --offline --locked --release -p repair-cafe-fold-trial -- demo
cargo run --offline --locked --release -p repair-cafe-fold-trial -- interruption
cargo run --offline --locked --release -p repair-cafe-fold-trial -- benchmark
```

`benchmark` generates 8,000 items, 1,200 borrowers, 100,000 valid ordered events, four contradictory source rows, and one malformed row. It imports and checkpoints the accepted data, reports every rejected row, measures reopen and point lookups, exercises 50 search queries, and reconstructs the materialized view independently three times.

`interruption` launches a child that aborts after writing an event inside an uncommitted Fold transaction, verifies the old state after reopening, then verifies the complete new state after a committed write. It models process interruption at the application-visible boundaries, not power loss or arbitrary storage corruption.

Generated databases live under this prototype's ignored `target/` directory and successful runs delete them. The deliberately aborting interruption child can leave database residue there until the parent or a later cleanup removes it.
