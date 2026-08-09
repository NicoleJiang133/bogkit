# Ordered discovery and friction trail

The scenario brief at `/private/tmp/bogkit-sim-2026-08-09-designer/trial2-brief.md` was read first and treated as the entire acceptance contract. No prior lab material, other trial, GitHub state, automation state, or private input was inspected.

## Public discovery order

1. Root `README.md`.
   - Learned that BogKit contains Fold, ESE, ANNy, and four named examples.
   - Did not run `scripts/new-project.sh`, because it would create an example and alter the root workspace, outside the permitted output boundary.
2. `examples/starter/Cargo.toml`, then `examples/starter/src/main.rs`.
   - Observed Fold persisting a count and bag under transactional insert/retract operations.
   - Friction: the starter manifest includes Fold, ESE, and ANNy even though its code uses only Fold, so it does not demonstrate dependency selection.
3. `examples/timeseries/Cargo.toml`, then `examples/timeseries/src/main.rs`.
   - Observed incremental `Filter`, `KeyBy`, `Aggregate`, `Table`, `Bag`, and `Count` views.
4. `examples/chat/Cargo.toml`, then `examples/chat/src/main.rs`.
   - Observed a single owner for a persistent Fold stream and publication of snapshots.
5. `examples/search/Cargo.toml`, then `examples/search/src/main.rs`.
   - Observed BM25 plus HNSW search over ESE embeddings.
   - The example explicitly says pipeline closure types make ordinary typed helper functions impractical, so it uses macros. That is meaningful API friction for a Rust beginner and mixed-language team.
6. Only after the public README and examples: `anny/readme.md`, `ese/readme.md`, and `fold/Cargo.toml`.
   - ANNy identifies itself as approximate nearest-neighbor search.
   - ESE documents static text embeddings.
   - Fold's manifest confirms it brings its persistence stack; the public examples remained the clearest usage documentation available in discovery.

## Selection conclusion

- Fold: no fit. The required result is a one-shot exact comparison between two static ordered policies. Persisted incremental views add lifecycle and storage concerns without providing the exact ordered rectangle-difference operation.
- ESE: no fit. Text embeddings have no role in CIDR/protocol/port semantics.
- ANNy: no fit. Approximate lookup cannot support an exact no-change verdict or exhaustive shadowing classification.

The standalone implementation uses only Rust plus Serde for strict JSON input/output. No BogKit core, examples, manifests, lockfiles, or Git state were changed.
