# Support-case finder evaluation

This is a runnable evaluation harness for a bilingual "related resolved cases" panel. It keeps approved case fields in a Fold `KeyedStream`, maintains BM25 and ESE/ANNy indexes from the same updates, and returns deterministic hybrid results with exact source excerpts.

It is evidence for local mechanics, not a recommendation to deploy. The disclosed retrieval fixture is tiny and the scale fixture is synthetic. Neither replaces the team's frozen 200-query evaluation or a run in the specified 2-core, 2 GiB container.

## Commands

Run these from the `developer-simulation/` workspace root. The archived
performance evidence uses the workspace's ordinary release profile.

Run the disclosed retrieval, crash recovery, deterministic ordering, update, and privacy checks:

```console
cargo run --offline --locked --release -p support-case-finder
```

Run the synthetic 75,000-case build and 2,000-record mixed refresh:

```console
cargo run --offline --locked --release -p support-case-finder -- --scale 75000 2000
```

Run code checks:

```console
cargo test --offline --locked --release -p support-case-finder --all-targets
cargo fmt -p support-case-finder -- --check
cargo clippy --offline --locked --release -p support-case-finder --all-targets -- -D warnings
```

The first ESE build downloads its model and tokenizer into `target/ese-cache`.
The archived commands assume those two files are already cached; remove
`--offline` for an authorized first build. Query execution itself does not
make model or search-service calls.

## Expected failure reproducer

The evaluated Fold revision documents that a panicking `wtx` rolls back. The committed row does survive, but catching that panic and attempting another write in the same process fails because the underlying writer lock is poisoned:

```console
cargo run --offline --locked --release -p support-case-finder --bin panic_poison
```

Expected exit status: `101`. Expected final error contains:

```text
poisoned tx lock: PoisonError
```

This command is intentionally excluded from the successful demo path.

## Data boundaries

All cases are fabricated. The `Case` type contains only the approved projection fields: case ID, language, product area, subject, cleaned body, resolution, and modified time. Displayed hits contain case ID, product area, language, and an excerpt copied byte-for-byte from the approved body. No names, addresses, account identifiers, attachments, or raw messages are present.
