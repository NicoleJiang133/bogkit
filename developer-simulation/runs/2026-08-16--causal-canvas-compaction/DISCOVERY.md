# Discovery notes

## Baseline-first hypothesis

The production baseline should remain authoritative unless a narrow BogKit component can preserve the existing causal operation semantics, the full 30-day reconnect contract, and atomic generation publication without merely moving the bespoke logic elsewhere. The smallest useful experiment is therefore not a new database. It is an independent replay/compaction core with a literal oracle scenario and a replaceable local generation directory, while PostgreSQL and the TypeScript reducer remain outside the prototype boundary.

Before reading component implementation, my expectation is that most of the hard problem is domain-specific: dependency validation, operation identity, deterministic last-writer ordering, tombstone retention against acknowledged actor watermarks, canonical JSON, and complete-generation publication. A generic incremental view engine could potentially help with current-state materialization, but only if it also exposes enough identity, ordering, deletion, persistence, and transaction behavior to avoid duplicating those rules. The initial experiment could only exercise ordinary returned errors at stage boundaries; process crash and I/O-fault evidence would require a separate harness.

## What the public material led me to expect

- The root README describes Fold as a persistent incremental-programming framework with iterator-like materialization primitives.
- The starter example demonstrates transactional fan-out to a count and a durable bag, consistent snapshot reads, exact-record retraction, and reopenable storage.
- The timeseries example demonstrates keyed aggregation and retraction-driven updates, but no causal clock or compaction policy.
- The chat example shows a single-writer ownership pattern and publishes a freshly read consistent snapshot after each committed write. This looks relevant to visibility within one Fold database, not necessarily to atomic replacement of an externally visible multi-file generation.
- The search example presents `KeyedStream` upsert/remove behavior and HNSW/BM25 views. It says HNSW deletion avoids tombstones for search churn, which is not equivalent to the canvas tombstones required to reject delayed edits.
- ESE is presented as static text embedding and ANNy as HNSW approximate-nearest-neighbor indexing. Neither public example suggests causal replay, compaction, or generation publication behavior.
- No component README was present; the only public component overview was the root README, supplemented by the public examples.

Sequence correction: my initial case-sensitive `README*` inventory missed the three lowercase `readme.md` files. I discovered them from component manifest metadata after writing the first version of this note, but before opening any implementation source, and read them immediately. Fold's component README explicitly promises delta-based incremental materialization, fjall-backed transactional/crash-safe writes, and consistent snapshots across sinks. ANNy's confirms a no-dependency in-memory HNSW index; ESE's confirms two text-embedding functions. These details strengthen, rather than change, the hypotheses below. The inventory miss itself is recorded as discovery friction.

## Components considered

### Fold

Potentially relevant for persistent, incrementally maintained views and consistent transactions. Concrete fit questions to answer from the smallest source surface:

1. Can inputs carry the existing operation identity and be deduplicated or conflict-checked without a parallel custom index?
2. Does persisted iteration have a documented deterministic order suitable for canonical bytes, or must the caller sort?
3. Can a keyed row be atomically compared and updated, or does conflict-safe apply require separate application logic?
4. What durability and crash guarantees does a write transaction provide, and do they extend to complete-generation directory publication?
5. Can retention decisions inspect per-actor dependency/watermark metadata, or is that necessarily custom logic?
6. Does the API bound decoded payload size before allocation?

### ESE

Initially no fit. The workload has no semantic-search or embedding requirement. Retaining it would add model/runtime/data concerns without replacing causal replay or compaction logic. I will inspect only enough public API/source metadata to confirm the absence of a relevant storage or causal primitive if needed.

### ANNy

Initially no fit. Approximate-nearest-neighbor search does not express exact operation identity, dependency clocks, list ordering, tombstones, or atomic generation publication. The example's deletion behavior is specifically inappropriate as a substitute for causal tombstone retention. I will inspect only enough public API/source metadata to confirm that it is an index rather than a replay/compaction store if needed.

## Initial fit/no-fit questions

- Can Fold eliminate a meaningful part of the bespoke correctness surface, or would it merely duplicate current state beside the authoritative operation/snapshot data?
- Are Fold's transactions sufficient only inside one store, leaving the required prior-or-next directory publication protocol custom?
- Would exact deterministic output require sorting all materialized rows in the caller, reducing the value of the component for canonical snapshots?
- Does safe collection still require domain-specific acknowledgment and dependency-frontier proofs that none of the three components represents?
- If those answers are unfavorable, the honest result is a standalone reduced-model reproducer plus an evidence-backed no-fit decision, not an adoption claim.

## Intended prototype and evidence limits

I intend to build a small Rust library and CLI that models a narrow but causal subset of the brief: scalar property writes and object deletion across multiple actors, exact operation identity, causal buffering, deterministic last-writer selection, actor-age decisions, conservative compaction, and replaceable generation publication. Tests will derive expected canonical JSON and digests from literal scenarios rather than from the implementation. This reduced model can expose integration and safety boundaries; it cannot prove parity with the unavailable TypeScript reducer, all six operation kinds, the 5,000,000-operation fixture, PostgreSQL coordination, or a production crash model.

## Reviewer fix round 1/5 discovery correction

The initial handoff was `REJECTED_UNTIL_FIXED` after independent lifecycle tests found two prototype correctness defects and one evidence defect. The corrections changed the trial evidence as follows:

1. Operation identity is not catch-up history. Reconstructing the identity map from retained operations allowed a compacted-away ID to be reused with different bytes after reopen. The prototype must persist one compact canonical SHA-256 fingerprint for every accepted ID whose reuse remains forbidden, including IDs from other document IDs if identity is global.
2. Selected payload digests do not bind semantic manifest metadata. `reconnect_decisions` could be changed while all three selected digests still passed. The complete canonical manifest bytes now require a separately stored SHA-256 digest; this detects a changed field and makes `open_latest` fall back or fail. This is an integrity check, not keyed authentication or Byzantine protection.
3. Stage-return injection is not termination or an I/O fault. The code returns an ordinary error after each of five completed stages. It does not kill a child, interrupt a write, synthesize disk-full/permission errors, exercise `CURRENT` sub-stages, or test process/host crash or power loss. The CLI only parses and shape-validates `crash_schedules.json`; it does not execute the schedules.
4. Correct permanent identity metadata changes the fit evidence. On the same hand-built overwrite-heavy input, compacted bytes are 3,719/8,670 (42.90%), so the brief's at-most-40% gate now fails. This strengthens the original baseline/no-fit decision and invalidates the initial handoff's synthetic PASS claim.
