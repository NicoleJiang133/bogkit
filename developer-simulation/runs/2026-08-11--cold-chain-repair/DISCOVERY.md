# Discovery trail

## Before reading BogKit internals

I am approaching this as the platform engineer in the brief: strong in Python, still fairly new to Rust, and looking for a reliable way to repair derived cold-chain incident state from an authoritative NDJSON archive.

From the public README, BogKit is a Cargo workspace containing three fairly independent pieces. Fold is the only obvious candidate for this problem: it incrementally maintains durable views from inserts and retractions. ESE and ANNy are embedding and nearest-neighbor tools and do not belong in a deterministic temperature reducer.

The examples make Fold look attractive for four narrow properties:

- one write transaction can update multiple materialized views atomically;
- a `KeyedStream` can upsert by identifier and retract the previous value;
- state reopens from a local path after restart;
- reads observe one consistent snapshot.

They do not yet show the harder parts of this brief: changing an old event whose consequence depends on every later event, replaying a freezer under a backdated configuration, using an NDJSON archive rather than Fold as authority, validating an entire batch before publication, or exporting the exact causal observations and configuration versions behind transitions.

## Candidate baseline

My baseline is deliberately boring and independent of BogKit:

1. Validate a complete incoming batch in memory against the authoritative archive index.
2. Append the accepted records to NDJSON.
3. For each affected freezer, resolve corrections and configuration intervals, sort the effective readings by `(observed_at, observation_id)`, and replay its state machine from the beginning of the seven-day repair horizon.
4. Replace the affected freezer's canonical incident summary in a disposable snapshot.
5. Write the complete derived snapshot to a sibling temporary file, sync it, then rename it atomically.

The reference reducer should be a separate, simple implementation that resolves the full logical history and replays it from scratch. It should not share the candidate's indexing or incremental repair code. Exact JSON bytes should come from explicitly sorted vectors and fixed field order.

## Component-use hypothesis

My initial hypothesis is that Fold could be useful as an optional durable index of raw accepted records or current keyed values, but it may be a poor fit for the actual temporal repair reducer. A late reading, correction, or backdated configuration can change an incident boundary and all later reasons for one freezer; the public primitives show local map/filter/aggregate updates, not an ordered state machine with suffix invalidation and exact provenance.

I will first try to represent accepted observations by ID in a `KeyedStream` and verify durability, idempotent upsert, and atomic visibility. I will compare that with the simpler archive-plus-atomic-snapshot baseline. I will reject Fold for the production boundary if it introduces a second durable authority, cannot express the time-ordered repair without rebuilding a freezer externally, or cannot make archive append plus derived-state publication one atomic commit.

ESE and ANNy are out of scope unless later documentation reveals a deterministic non-search purpose, which seems unlikely.

## Evidence plan

- Use hand-checked fixtures for threshold boundaries, duplicates, six-hour lateness, corrections, and backdated limits.
- Create a separate full-replay reference reducer.
- Differential-test chronological, shuffled, and repeated batches for at least 100 seeds.
- Inject failure immediately before and after the snapshot replacement and verify restart convergence.
- Run a one-million-observation rebuild and measure elapsed time and peak resident memory on this machine.
- Keep any durable state disposable and the NDJSON file authoritative.

At this point I have read only the blind brief, the public root README, and the public examples. I have not inspected component source or prior simulation material.

## After inspecting the minimum Fold source

I read Fold's stream transaction plumbing, keyed stream, table terminal, and their focused tests. The implementation confirmed that a `KeyedStream` stores its primary-key row and downstream materializations in the same Fjall transaction. A panic unwinds through `Stream::wtx`, calls the pipeline's abort hook, and drops the uncommitted database transaction. `checkpoint` requests a full sync. Those are useful properties for a disposable record index.

The source also confirmed the main mismatch. Fold's pipeline operators handle local deltas and per-key aggregates. Nothing in the public operator set represents an event-time-ordered state machine where replacing one old reading invalidates a suffix of later decisions. I would still have to resolve corrections, choose historical configurations, sort every affected freezer timeline, replay it, build exact reasons, and atomically replace an external canonical export. Fold would not own the authoritative archive append or the export rename, so it cannot make that entire boundary one transaction.

## Implementation and debugging trail

I wrote tests before reducer behavior and watched the initial boundary test fail against an empty stub. I then built a literal full-replay reference and a structurally separate candidate reducer. The candidate validates the archive first, upserts accepted records into a disposable Fold `KeyedStream<String, Record, Table<...>>` in bounded chunks, checks the indexed count, and then repairs freezer timelines. ESE and ANNy were not imported.

The first real Fold integration failed at runtime in `TableReader::iter`:

```text
called `Result::unwrap()` on an `Err` value: WontImplement
```

I initially suspected the isolated lock had selected Fjall/LSM-tree 3.1.8 while BogKit's root lock used 3.1.6. Pinning both older versions did not change the failure. The actual cause was the record's ordinary Serde internally tagged enum representation, which Postcard cannot deserialize. Changing to an externally tagged enum exposed a second runtime panic:

```text
called `Result::unwrap()` on an `Err` value: DeserializeUnexpectedEnd
```

That came from `skip_serializing_if` omitting a `None` field that Postcard's schema-dependent decoder expected. Always serializing the option fixed it, and restoring Fjall/LSM-tree 3.1.8 remained green. I preserved both cases as minimal binaries rather than changing Fold.

The completed differential run covered 100 deterministic seeds. For each seed, the candidate received shuffled records and then shuffled records with a repeated batch; both matched the independent chronological full replay exactly. The full test suite has seven acceptance tests and two CLI/subprocess tests.

Actual child processes were terminated inside an uncommitted Fold transaction and immediately before and after the canonical snapshot rename. Reopening showed the prior index after the uncommitted exit, the prior snapshot before rename, the next snapshot after rename, and convergence after restart.

## Scale result and revised fit decision

On a Mac16,11 (arm64, 14 logical CPUs, 64 GiB RAM), macOS 26.5.2, Rust 1.95.0, the one-million-observation/400-freezer reference replay took 0.727 seconds inside the reducer and peaked at 511,508,480 bytes RSS. The Fold-backed candidate took 3.320 seconds inside the rebuild and peaked at 1,155,678,208 bytes RSS. Both were far below 60 seconds, but both exceeded the brief's 384 MiB ceiling; the candidate exceeded it by a much larger margin.

This benchmark includes full in-memory fixture and validation maps, so it is not evidence that Fold itself consumes one gigabyte. It is evidence that this straightforward integration cannot meet the facility constraint, and that adding the Fold index made an already-too-large design materially heavier. A production implementation would need streaming validation plus disk partitioning or an archive-native index. That redesign would still leave the temporal reducer outside Fold.

My final decision is **no fit for the production reducer**. Fold worked as documented for atomic keyed state once the hidden serialization restrictions were discovered, and it was useful for testing crash behavior. But it did not remove the hard domain code or replace the simpler archive/replay/snapshot baseline. The defensible BogKit improvement is small: document the Postcard-compatible Serde subset prominently, and preferably surface serialization failures as typed errors instead of panicking. I would not add a temporal-repair subsystem to Fold based on this single trial.
