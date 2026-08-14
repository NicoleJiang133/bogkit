# Trial 1 discovery notes

## Baseline-first understanding

The required reference baseline is an append-only JSONL journal with one checksummed record per accepted command group, plus a full canonical JSON snapshot every 2,000 accepted groups. Recovery loads the newest valid snapshot, validates and replays its journal tail, and discards only a torn or checksum-invalid final record with an offset-only diagnostic. The same generated actions, durability/flush points, process-exit schedule, compiler settings, and host must be used for the baseline and any candidate.

The baseline is not optional scaffolding. It is the comparison oracle for the candidate's correctness and measurements. Before performance can matter, both implementations must preserve atomic command groups, the active undo/redo branch, duplicate submission outcomes, canonical output, and the acknowledgement boundary. A small prototype can honestly prove only a bounded subset of the 250,000-action/30-seed/400-exit campaign; unrun criteria must be reported as unproved rather than inferred.

## Public onboarding trail (ordered)

1. Read `BRIEF.md` in full.
2. Read the root `README.md` in full. It recommends generating a new example with `scripts/new-project.sh`, describes the workspace, and points to `cargo doc -pfold` for Fold's internal documentation.
3. Read every public example manifest and `main.rs`: `starter`, `timeseries`, `chat`, and `search`.
4. The smallest public persistence example is `starter`. It shows one `Stream`, one writer transaction (`wtx`) containing multiple inserts, and one consistent read transaction (`rtx`) spanning multiple durable views.
5. `timeseries` demonstrates retractions and incrementally maintained derived views. `chat` demonstrates a single writer plus read snapshots published to concurrent clients. `search` demonstrates keyed update/removal across several indexes.

### Discovery friction so far

1. **Documentation gap:** the root README describes persistence, transactions, and fast materialized reads, but does not state the crash acknowledgement boundary, filesystem sync policy, transaction return/error behavior, or recovery diagnostics needed by this trial.
2. **Example ambiguity:** examples delete their database directory at startup for determinism, so the primary examples do not demonstrate a close/reopen recovery cycle even though comments say state persists.
3. **API-shape friction:** `wtx` examples use closure callbacks and do not show rejected transactions, duplicate request IDs, compare-and-set, or a durable commit result. Whether those can be built cleanly is not yet known.
4. **Scope warning:** the public material presents an incremental database/view engine, not an undo journal. Branching undo, stable-ID idempotency, record-level checksums, and history-retention compaction may require a custom layer even if Fold stores current indexes well.

## Initial component fit hypotheses

### Fold

**Initial hypothesis: possible partial fit; inspect further.** Public examples claim that one `wtx` updates several durable views atomically and that `rtx` sees a consistent snapshot. Those properties could materially help a candidate represent the current document, group/result lookup, and inspector view under one writer. Retractions may help apply inverse commands. However, the public surface does not establish an acknowledgement-after-durable-flush guarantee, ordered replay log, branching history semantics, exact duplicate outcomes, crash injection behavior, or bounded compaction retaining 20,000 reversible groups. Fold is useful only if it carries central transactional/read-view persistence rather than wrapping a separate custom journal decoratively.

### Embedded Static Embeddings (ESE)

**Initial hypothesis: no fit.** ESE maps text to static embeddings for similarity search. This trial has structured fixed-point editor commands and exact recovery requirements, not approximate semantic retrieval. Adding embeddings would increase resource use and would not improve acknowledgement, atomicity, branching, or compaction.

### Approximate Nearest Neighbors (ANNy)

**Initial hypothesis: no fit.** ANNy provides HNSW approximate-nearest-neighbor search. The trial needs exact command identity, deterministic ordering, and canonical state. Approximate retrieval is not on the critical path and cannot serve as a correctness index.

### Public examples as starting points

- `starter`: useful API sketch for atomic multi-view writes and snapshot reads.
- `timeseries`: useful evidence that retractions can update derived materializations, but its floating-point presentation and aggregation domain are unrelated.
- `chat`: useful ownership sketch for one writer and many readers; its `watch` broadcasts are process-local and do not prove persistence or crash recovery.
- `search`: no direct fit beyond keyed upsert/removal semantics; its search indexes are unrelated to exact history.

## Initial selection rule

Proceed only with Fold, and only after a small public-contract spike can show a clean reopen path and clarify commit durability behavior. ESE and ANNy will not be dependencies. If Fold's public API or documented storage contract cannot prove the acknowledgement rule, the honest outcome is a bounded partial-fit reproducer or no-fit result, with the JSONL baseline retained as the reference rather than inventing a new subsystem.

## Post-prototype decision and skeptical review round 1

The smallest meaningful candidate used Fold for one authoritative whole-state value, with the JSONL-plus-snapshot design retained as an independently persisted comparator. This isolated Fold's atomic-write and snapshot-read contribution without claiming that Fold supplies branching undo, command inversion, stable-ID outcomes, or retention. The one-host measurement then decisively rejected that full-state layout: it failed the bounded edit-latency and baseline non-regression gates. The resulting classification is **`local_proof_only_no_production_fit`**, not a subsystem proposal and not a finding against normalized Fold storage.

Skeptical review round 1 returned `REJECTED_UNTIL_FIXED` and identified four prototype correctness gaps. Each was reproduced as a failing test before repair:

1. A recognized partial/checksum-invalid final JSONL tail was ignored in memory but left on disk. A later acknowledged append could join that bad tail and disappear on the next reopen.
2. `i64::MIN` movement could panic while constructing an inverse after mutation. Destination overflow also needed explicit prevalidation.
3. Exact stable-ID retries returned the old outcome but still caused persistence work; the baseline also advanced its accepted-group snapshot cadence.
4. Deduplication treated a 64-bit command digest as authoritative equality instead of preserving exact canonical command content.

The repaired implementation physically truncates a recognized bad final tail, syncs the journal and containing directory before a later append can occur, and still treats middle corruption as fatal. Object and point translation now calculate both inverse and destination values before mutation. Stores distinguish a newly admitted commit from an exact retry, so retries return the original outcome with no write and do not affect snapshot cadence. Dedup records now persist exact canonical command bytes across restart and compaction; the digest remains diagnostic only.

### Additional ordered debugging friction

5. Tail recovery looked correct after one reopen because the in-memory model discarded the bad suffix; only a reopen, later commit, second-reopen test exposed that the disk was never repaired.
6. Group-level clone-and-apply protected persistent state from an error, but it did not prevent debug-mode arithmetic panic inside a command or satisfy the stronger prevalidation contract.
7. Stable-ID equality and stable-ID persistence were separate concerns: returning the correct old outcome did not prove that a retry was write-free or that snapshot cadence counted only unique commits.
8. A convenient fixed-width 64-bit digest was adequate for diagnostics and transcript checks, but not for authoritative command equality. Persisting exact canonical bytes is the smallest repair; inventing a new index or API would be unjustified.
9. Strict Clippy caught refactoring and test-style issues after the semantic fixes; the final package-scoped strict gate passed without changing BogKit or public examples.

### Revised fit hypotheses after evidence

- **Fold: bounded partial utility, no production fit for this layout.** Atomic whole-state writes and snapshot reads materially helped the local proof. The full-state rewrite failed performance/storage comparison, and concurrency, power-loss, and scale requirements remain unproved.
- **ESE: no fit, unchanged.** No observed requirement became semantic retrieval.
- **ANNy: no direct fit, unchanged.** Exact ordered history cannot use approximate equality. Its presence in the dependency tree is a packaging observation through Fold, not a functional defect.

No reviewed failure was traced to BogKit itself. The four rejected-round items were prototype defects. Public durability/reopen guidance remains the smallest plausible upstream improvement; documentation and examples should precede API changes.
