# Baseline-first discovery hypothesis

## Starting position

I am approaching BogKit as a municipal-utility data engineer with nine years of
SQL/Python work and one year of Rust, and no prior BogKit knowledge. The
existing SQL/Python process remains the authority: it has auditor-understood
views, approved-adjustment import boundaries, and a safety property that issued
bills are never edited. A prototype earns adoption only if it makes a bounded,
deterministic repair plan easier to replay, attribute, or publish safely. It
must not become a second billing authority.

The direct chronological reference reducer is the default implementation. It
will validate immutable exports, resolve supersession, order each
service-point history, compute exact integer deltas, and emit canonical output.
That reference must exist independently of any BogKit component so the
candidate has a real oracle.

## Expectations from the public surface

From the root README and public examples only:

- Fold is described as a durable incremental stream-to-view framework with
  transactional writes, consistent read snapshots, keyed upsert/removal, and
  explicit retraction. The timeseries example shows per-key aggregation; the
  search example shows an update retracting old derived state. These properties
  may help demonstrate exact replay or incremental replacement, but do not by
  themselves solve temporal ordering, rollover, meter-install validity,
  correction causality, provenance, or atomic report-file publication.
- ESE is a static text-embedding component. This workload contains structured
  integer and timestamp data and prohibits approximation in the calculation,
  so ESE appears to be a no-fit unless component details reveal a non-embedding
  capability directly relevant to deterministic replay.
- ANNy is an approximate-nearest-neighbour index. Approximate similarity has no
  role in an exact billing repair plan, so ANNy appears to be a no-fit.
- Public examples use temporary databases and remove them for deterministic
  demos. The trial must instead keep all runtime state under
  `simulation-output/`, preserve prior reports on failure, and clean generated
  state before handoff.

## Baseline-first build hypothesis

First build the smallest direct, chronological, integer-only reducer and a
separately reasoned reference using disclosed literal fixtures. Record a
meaningful failing test before implementation. Only after the oracle and safety
contract are explicit should Fold be tested as an optional materialization or
replay layer. Retain it only if evidence shows a concrete reduction in replay,
provenance, bounded recomputation, or safe-publication work. A correct no-fit
result is preferable to forcing the dependency.

## Explicit component-fit questions

### Fold

1. Can records be deterministically keyed and upserted/retracted without input
   order or batch boundaries leaking into canonical output?
2. Can Fold expose the exact old record needed for supersession and provenance,
   or must application code maintain that mapping anyway?
3. Are a transaction's materialized views read as one consistent snapshot, and
   what happens on an application panic or reopen?
4. Can per-service-point invalidity be isolated without poisoning valid plans
   for unrelated points?
5. Does Fold materially bound recomputation for a corrected reading, or would
   the application still sort and reduce the complete service-point history?
6. Does persistence introduce a second durable authority, migration/cleanup
   burden, or nondeterministic physical state that is unnecessary for an
   offline report?
7. Does its API accept the structured record keys and exact integer values
   needed here without floating point or lossy conversions?

### ESE

1. Does it provide any exact structured-data function beyond text embeddings?
2. If not, can it be rejected immediately because embeddings neither validate
   histories nor improve exact replay/provenance/publication?

### ANNy

1. Does it provide any exact ordered lookup relevant to source IDs or time?
2. If it is solely approximate vector retrieval, can it be rejected immediately
   because approximation would be both unnecessary and unsafe here?

## Done criteria for this trial

- A runnable offline CLI under `simulation-output/` with disclosed fixtures,
  an independent chronological reference, canonical plan output, and atomic
  publication that preserves a prior report on all exercised failures.
- Permanent tests for rollover, regression ambiguity, meter replacement,
  supersession causality, duplicate/conflicting IDs, input/batch determinism,
  provenance, failure isolation, and publication lifecycle; at least one RED
  result preserved as evidence.
- Candidate/reference equality on disclosed cases and 100 deterministic seeds;
  byte equality across ten permutations and three batch sizes.
- A two-million-reading local release run with elapsed-time and peak-memory
  evidence, accurately scoped to this host.
- Fresh formatting, tests, strict Clippy, release demo, cleanup, reproduction
  instructions, and an evidence-limited trial report.
