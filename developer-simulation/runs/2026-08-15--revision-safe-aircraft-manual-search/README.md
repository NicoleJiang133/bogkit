# Revision-safe manual search trial

This is a bounded, synthetic failure reproducer for the aircraft-manual brief. It uses Fold BM25 to show that retrieving a global top 20 and then applying aircraft eligibility can return nothing even though an eligible matching section is ranked next.

It also includes a deliberately exhaustive fallback that validates the complete fixture boundary before selection. It rejects repeated canonical document IDs, repeated ranked document IDs, duplicate revision IDs, overlapping active revisions, unknown applicability data, and missing ranked metadata. For valid unique input it filters active and applicable revisions before the final top ten, applies the required deterministic tie-break, and returns exact evidence. Reversing valid metadata rows does not change eligibility or evidence. This is safety-correct only for the tested 21-document fixture and is not presented as scalable to 2.4 million revisions.

The first skeptical review round rejected the retained selection because repeated canonical IDs were overwritten in row order and repeated ranked IDs returned duplicate evidence. Fix round 1 added fail-closed typed errors plus permanent regressions for both cases and for valid metadata-order independence. The overall product decision remains no-fit because the BogKit search readers still lack bounded query-time eligibility filtering.

Run everything from the repository root without network access and keep build output outside this checkout:

```console
cargo fmt --manifest-path developer-simulation/Cargo.toml --package revision-safe-manual-search-trial -- --check
CARGO_TARGET_DIR=/private/tmp/bogkit-trial2-target cargo test --offline --locked --manifest-path developer-simulation/Cargo.toml --package revision-safe-manual-search-trial --all-targets
CARGO_TARGET_DIR=/private/tmp/bogkit-trial2-target cargo clippy --offline --locked --manifest-path developer-simulation/Cargo.toml --package revision-safe-manual-search-trial --all-targets -- -D warnings -D clippy::all -D clippy::pedantic
CARGO_TARGET_DIR=/private/tmp/bogkit-trial2-target cargo run --offline --quiet --manifest-path developer-simulation/Cargo.toml --package revision-safe-manual-search-trial
```

The demo reports zero eligible results from the bounded baseline, one from the exhaustive safety proof, and the exact manual, stable-section, revision, and effective-interval reference.
