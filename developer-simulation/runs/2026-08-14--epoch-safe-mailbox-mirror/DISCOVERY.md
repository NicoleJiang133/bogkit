# Discovery: epoch-safe mailbox mirror repair

## Starting point

I am approaching this as a desktop synchronization developer with deep SQLite experience and limited Rust experience. The server transcript is authoritative. The local store is disposable projection state. The hard problems are transactional batch admission, idempotent replay, epoch isolation, all-or-nothing snapshot publication, deletion/recreation by stable identity, deterministic recovery, and predictable UI reads. Protocol parsing and two-way synchronization are outside the boundary.

Before considering BogKit, the reference design is the stated SQLite baseline:

- one explicit transaction per accepted response batch;
- message identity indexed by `(mailbox_id, uid_validity, uid)`;
- active mailbox metadata and cursors stored with the visible epoch;
- a separate staging area for a full snapshot;
- a single transaction that replaces the visible epoch with the complete staged epoch;
- duplicate batch admission recorded durably in the same transaction as its effects;
- unsigned persisted values constrained to the exact SQLite `INTEGER` domain before admission; and
- explicit cleanup of both visible and staged rows for the deleted stable mailbox ID;
- UI readers using ordinary read transactions so they see either the state before a commit or the state after it.

That baseline already has a strong fit for the workload. A BogKit candidate must first preserve those semantics, then earn its added complexity with the required code-size or performance improvement. It is acceptable for the result to be partial-fit or no-fit.

## Onboarding trail

I read only the public root `README.md` and the four public examples before writing this file.

1. The root README recommends creating a workspace example and presents Fold, ESE, and ANNy as independent components that need not all be used.
2. `starter` shows a Fold `Stream` with one writer transaction updating a persistent count and bag, and one read transaction seeing a consistent snapshot.
3. `timeseries` shows incremental keyed aggregates and retained raw records, including retraction.
4. `chat` shows a single ingest owner and snapshot readers. It says each Fold write commits transactionally and that a read transaction sees consistent materialized views.
5. `search` shows `KeyedStream`, `upsert`, and `remove`, with multiple indexes updated together. It is oriented to text and vector search, not mailbox synchronization.

The public examples make Fold's atomic multi-view update and durable reopening directly relevant. They do not publicly demonstrate crash injection, a transactional epoch-swap primitive, durable duplicate-batch admission, snapshot staging, multi-process recovery, or stable iteration ordering. Those gaps are questions, not defects.

## Initial fit hypotheses for every public component

### Fold: possible partial fit, subject to baseline comparison

Potential material help:

- one writer transaction could update message state and materialized counts together;
- keyed upsert/remove could make incremental add, flag replacement, and expunge easier to express;
- read transactions may give consistent counts and message views;
- durable reopening may eliminate some bespoke rebuilding.

Risks to test before selection:

- the mailbox snapshot is a set replacement, while the examples expose record-level insert/retract/upsert operations;
- atomic epoch publication may still require custom staging and a large explicit retraction/insertion transaction;
- replay detection and cursor monotonicity may remain custom state;
- deterministic canonical iteration may require sorting outside the component;
- the storage shape and write amplification could be worse than the indexed SQLite baseline;
- it is not yet shown that readers can remain on the old complete epoch while a new one is staged.

Selection threshold: use Fold only if a minimal test proves it can own the central projection behavior without weakening batch transactions or reader visibility. If it helps only incremental records while epoch replacement needs a bounded custom layer, classify partial fit and retain SQLite for the hard boundary.

### ESE: no apparent fit

The mailbox records contain fixed fingerprints, dates, sizes, flags, and identifiers. There is no text or semantic-search requirement. Embeddings would add cost without serving an acceptance criterion. I will not use ESE unless later public behavior reveals an unexpected directly relevant capability.

### ANNy: no apparent fit

There is no nearest-neighbor query. Message identity lookup, counts, cursor access, and canonical enumeration require exact behavior. Approximate vector indexing is unrelated. I will not use ANNy.

## Baseline-first experiment plan

1. Build a small faithful SQLite baseline and a pure in-memory reference over the same decoded batches.
2. Use hand-derived fixtures to test duplicate replay, UID reuse across epochs, incomplete snapshot retention, rename, deletion/recreation, cursor rejection, and canonical checkpoints.
3. Measure the baseline locally before choosing Fold.
4. Inspect only the minimum Fold surface needed to test the hypothesis.
5. If Fold cannot safely own epoch publication or would merely duplicate SQLite's staging machinery, produce a decisive minimal reproducer and a no-fit or partial-fit result instead of a broad replacement.

## Initial uncertainties

- Whether the environment already has all Rust dependencies cached for a fully offline build.
- Whether Fold offers a public keyed-record API suitable for composite mailbox identities without serializing custom keys awkwardly.
- Whether its transaction and persistence layer supports cheap atomic replacement of a large logical set.
- Whether active readers can be held across a writer commit and prove old-or-new epoch visibility.
- Whether deterministic persistence and diagnostics hold across crash points and input chunking.
- Whether a full 600,000-response, 300,000-message campaign is safe within this trial's available wall time; any smaller run will be labeled as such and will not be credited as meeting the full resource gate.

## Baseline evaluation and component decision

After implementing the reference and SQLite paths test-first, I ran a release-mode local baseline with 20 mailboxes, 10,000 messages, 120 accepted batches, and 200 count checkpoints. It processed 10,020 decoded responses at 11,045 responses/second; count-query p50 was 2.457 ms and p95 was 3.179 ms; reopen was 4.283 ms; the SQLite database plus sidecars occupied 2,867,200 bytes. This is a small local measurement, not proof of the 600,000-response gate. A linear throughput estimate would be about 54 seconds, but I will not treat that estimate as a measured full-run result.

Only after this measurement did I inspect the minimum Fold transaction and keyed-table implementation needed for a selection decision. Fold materially provides atomic durable writes, pinned read snapshots, keyed upsert/remove, and crash rollback. Its public `wtx` contract commits when a closure returns normally, including when that return value is `Err`; a `Result` return has no special rollback meaning. The runnable reproducer confirms that documented boundary. That behavior is API/onboarding friction for fallible external batches, not a BogKit correctness defect by itself.

For this workload, decoded batches can fail after earlier responses have been examined. A production adapter would therefore need a bespoke prevalidation/overlay layer that reproduces transaction state. Full snapshot publication would additionally need durable staged-message keys, staged UID enumeration, visible UID enumeration, replay records, cursor validation, and metadata bookkeeping. Consistent multi-key UI reads require a Fold sink in addition to `KeyedStream`'s own primary table, duplicating storage before comparison with SQLite.

Decision: use Fold only in the decisive transaction reproducer, not as the candidate projection. ESE and ANNy remain unused because they do not address exact mailbox identity or count queries. The trial proceeds as a no-fit baseline/reference comparator unless later evidence reverses one of these boundaries. No API change is proposed from this one scenario; a documentation/example note would be the smallest upstream improvement if broader developer evidence supports it.

## Post-review characterization and baseline repairs

The skeptical review supplied an independent current-main observation that required extending the original Fold reproducer. The extended same-stream test now proves two distinct facts: the panicking transaction's row is absent, then a later valid `KeyedStream::wtx` panics with Fjall's `poisoned tx lock`. This writer-poisoning behavior is the already-tracked BogKit/Fold correctness defect; it is separate from the documented normal-`Err` commit boundary. Catching a panic is not a safe rollback workaround on this checkout. The trial records the defect without editing core. The smallest already-recorded core candidate is to drop the underlying transaction guard before resuming the panic and add both Stream and KeyedStream regressions; no new API is proposed here.

The same review exposed two prototype defects in the baseline, both repaired test-first. First, every Rust `u64` field now has the exact shared domain `0..=9_223_372_036_854_775_807` in both the pure reference and SQLite implementation. UID validity, UID, cursor, byte size, and batch sequence are all checked before duplicate admission or mutation; max-boundary values round-trip exactly, while max-plus-one is rejected with identical diagnostics and no state or admission change. Second, deleting a mailbox now removes `staging_messages` for that stable ID inside the same admitted transaction. Physical-table tests cover reopen, repeated delete/recreate under the same ID, an incomplete staged epoch, and a failed rescan event. These are prototype fixes, not new BogKit findings.
