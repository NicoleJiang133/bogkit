# Discovery log

## Brief-first understanding

I maintain a PostgreSQL-backed support system and need to decide whether a small local semantic or hybrid index can beat our measured lexical baseline without becoming another source of truth. The decision hinges on retrieval quality on the team's frozen 200-query set, predictable CPU-only operation, transactional refresh behavior, exact source references, deterministic ranking, and manageable operations. A toy search demo alone cannot establish production fit.

## Public-material discovery order

1. Read `simulation-output/BRIEF.md` before learning anything about BogKit.
2. Read the root `README.md`.
3. Read the four public examples and their manifests: `starter`, `timeseries`, `chat`, and `search`.
4. Wrote this initial hypothesis and baseline plan before inspecting implementation source or internal documentation.

## Initial component-fit hypothesis

- **Fold:** potentially useful for keeping a document table, lexical index, and semantic index consistent under keyed upserts and removals. The search example claims transactional updates, genuine HNSW deletion, and persistence. Important unknowns are whether a failed multi-step refresh preserves the last complete daily snapshot and whether index-open/refresh operations fit normal service lifecycle needs.
- **ESE:** potentially useful because it embeds locally and therefore appears compatible with network-free query handling. The public material does not identify its model, English/Spanish quality, memory use, embedding speed, model-file lifecycle, or production CPU requirements. Cross-language support is therefore unproven.
- **ANNy:** potentially useful as the semantic nearest-neighbor index used by the search example. The example demonstrates use but not deterministic tie ordering, memory/disk sizing, 75,000-record build time, or recall under daily edits/deletions.

My provisional view is **evaluation-only, not adoption-ready**. The combination maps unusually directly to the desired hybrid search shape, but none of the production acceptance thresholds can be inferred from the README or twelve-record example.

## Baseline and experiment plan

The real acceptance gate remains the existing PostgreSQL evaluation: 116/200 recall@5 and 34 ms warm p95. I do not have the private frozen case/query set or the declared 2-core production runner, so this trial must not manufacture a comparable recall number.

I will build a minimal, runnable support-case index with only approved fields and a small bilingual, synonym-heavy fixture. It will:

1. Use keyed records so an edit or deletion can update every maintained view.
2. Compare BogKit BM25, semantic, and hybrid results on disclosed fixture queries, including English and Spanish.
3. Return case ID, product area, language, and an exact substring from the approved source body; scan serialized/displayed content for prohibited field names.
4. Apply an interrupted-refresh simulation that performs no transaction, prove the old index remains queryable, then apply a mixed transaction and prove edits/deletions are reflected.
5. Add deterministic score and case-ID tie sorting outside the index so displayed ordering has an explicit rule.
6. Run the fixture evaluation three times and compare ordered IDs byte-for-byte.
7. Measure local fixture build, update, query latency, and disk size only as a smoke measurement, explicitly not as evidence for the 75,000-record thresholds.

I will inspect source only as needed to resolve public-example API questions, and record any false starts or undocumented behavior.

## What changed after source inspection and running code

- The ESE build script identifies its embedded weights as `static-retrieval-mrl-en-v1`. The root README did not state this English-only provenance, which materially changes the Spanish part of the fit assessment.
- `cargo test --offline` was my first build attempt. Cargo had every Rust dependency cached, but ESE's build script independently attempted to download `model.safetensors` and failed on DNS. I copied the already-approved model and tokenizer cache into this prototype's ignored `target/ese-cache`, then offline builds worked. This is an undocumented reproducibility and build-operations requirement, not a query-time network dependency.
- My first interrupted-refresh test caught a panic around `KeyedStream::wtx` in the same process. The committed state remained readable, but the next write panicked with `poisoned tx lock`. I preserved that as `src/bin/panic_poison.rs`. For the actual process-interruption acceptance check, I changed the harness to abort a child process mid-transaction, reopen the index in the parent, verify the old committed state, and then apply a successful retry.
- The public example's reciprocal-rank fusion sorts only by score. I added an explicit case-ID tie break to the application result list and compared output bytes across three fresh processes. This makes the observed fixture deterministic, but it cannot repair a tie at the HNSW candidate cutoff; that boundary remains uncertain.
- A full synthetic scale smoke test was practical, so I added an explicit `--scale 75000 2000` mode. It measures mechanics and rough resource shape without pretending that repeated fabricated text is a retrieval-quality corpus.

## Final fit hypothesis

Fold's atomic keyed updates and the local BM25/HNSW shape are a strong match for the index-maintenance mechanics. BogKit is not yet a justified fit for this bilingual production feature: the private recall gate is untested, the disclosed fixture shows no gain over lexical search, the embedded model is English-specific, the required 2-core/2 GiB environment was not available, and a caught refresh panic poisons further writes in the process. The narrow recommendation is to keep the prototype as evaluation evidence and not adopt it until those gates are resolved.
