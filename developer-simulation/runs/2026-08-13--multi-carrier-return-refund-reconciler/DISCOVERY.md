# Trial 1 discovery notes

## Baseline-first hypothesis

The safest compact design is a direct, deterministic batch reconciler over one
immutable snapshot: validate and canonicalize records; group by return; reject
conflicting event IDs; apply corrections by explicit target ID; derive proposed
units and cents with integer arithmetic; independently verify conservation and
caps; serialize a canonical report to a sibling temporary file; then publish it
with one atomic rename. This preserves the existing system's most important
property because the program has no network or payment-execution capability.

The nightly Python/CSV baseline can be improved without adding an incremental
database: stable sorting, explicit provenance, a reference calculation, and an
atomic publication boundary address most of the stated pain directly. My first
comparison therefore has to be this direct approach, not a component-led design.

## What I expect from the public surface

The root README presents Fold as a durable incremental stream-to-view engine
whose write transaction updates multiple materialized views consistently. The
starter and time-series examples show insert/retract semantics, keyed
aggregation, and durable local state. The search example adds ESE embeddings and
ANNy HNSW indexing; those are relevance/search facilities rather than exact
accounting tools. The chat example demonstrates a single writer and consistent
snapshot reads, but it also makes Fold the source of truth, which conflicts with
this brief's PostgreSQL-authoritative, read-only snapshot boundary.

## Explicit component-fit questions

### Fold

- Can an immutable batch be loaded deterministically without retaining a second
  authoritative-looking copy between runs?
- Do duplicate inserts, explicit retractions, and correction-before-original
  arrival have semantics that help more than a sorted in-memory map?
- Can all return views be read from one transaction and exported without losing
  the complete-report-only publication guarantee?
- Are input validation, uniqueness/conflict quarantine, exact cent allocation,
  and report provenance native enough to reduce custom reconciliation code?
- Does measured evidence show a material correctness, auditability, or runtime
  advantage over the direct batch/reference approach at the target workload?

### ESE

- Is any requirement semantic or approximate? If not, embeddings would weaken
  exactness and are a no-fit even if easy to call.

### ANNy

- Is any requirement nearest-neighbor retrieval? If not, an approximate index
  would be actively inappropriate for identity, SKU, event, or money matching.

## Initial adoption gate

I will retain a component only if a runnable comparison shows a material benefit
without weakening PostgreSQL authority, exactness, deterministic output,
privacy, idempotency, or atomic publication. Otherwise the prototype will use a
direct Rust batch implementation and record a grounded no-fit decision.
