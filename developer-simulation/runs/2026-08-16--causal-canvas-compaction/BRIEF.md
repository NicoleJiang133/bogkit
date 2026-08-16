# Trial 1 brief: Causal canvas-history compaction

## Developer role and Rust experience

- You maintain synchronization for a collaborative diagram and whiteboard service.
- You have seven years of production TypeScript and Go experience.
- You have used Rust for one internal service for about nine months.
- You are comfortable with ownership and async code but not advanced type-level techniques.

## Existing system and baseline implementation

- Browser clients create immutable operations for shapes, text blocks, connectors, and layers.
- Each operation has a document ID, actor ID, per-actor sequence, dependency clock, and payload.
- A WebSocket service relays operations and appends them to a PostgreSQL table.
- A TypeScript reducer reconstructs a document from a snapshot plus later operations.
- The reducer uses last-writer-wins registers for scalar fields and ordered identifiers for lists.
- Deletions leave tombstones so delayed moves or edits cannot incorrectly recreate an object.
- Every 10,000 accepted operations per document, a background job writes a full snapshot.
- The baseline keeps all operations and tombstones for 30 days, then runs a bespoke compactor.
- PostgreSQL remains the production authority for accepted operations and snapshot generations.

## Concrete pain

- Busy documents accumulate several gigabytes of operations and hundreds of thousands of tombstones.
- A reconnecting client may download far more history than the current canvas contents require.
- The compactor has twice removed metadata needed by a client that was offline for several weeks.
- One incident caused two clients to show different connector endpoints until a full reload.
- Engineers do not trust changes because the baseline tests cover only a few hand-written schedules.
- The team wants a smaller, independently testable compaction and replay core.
- A replacement is useful only if it preserves the existing operation semantics exactly.

## Workload and data shape

- The production service has 5,000 active documents and about 40 million operations per day.
- The prototype fixture contains 100 documents and exactly 5,000,000 accepted operations.
- Each document has 64 actors and between 25,000 and 75,000 operations.
- A typical operation is 180 bytes; the largest permitted payload is 64 KiB.
- Operations cover create, set-property, move, list-insert, list-delete, and object-delete.
- Exactly 8% of fixture deliveries are duplicates and 12% arrive out of causal order.
- Five percent of actors disappear for 7 to 30 simulated days before reconnecting.
- The largest final document has 40,000 live objects and 20,000 retained list elements.
- Logical time is integer based; wall-clock arrival time cannot resolve concurrent edits.

## Operational constraints

- Existing operation IDs, merge rules, and canonical JSON output cannot change.
- The production database and WebSocket protocol cannot be replaced in this trial.
- The supported offline window is 30 full days from an acknowledged server watermark.
- A client within that window must be able to catch up without a destructive full reset.
- Clients older than the window may be told to fetch a fresh snapshot before submitting edits.
- Compaction may run concurrently with new appends but publishes only complete generations.
- Readers must observe either the prior generation or the complete next generation.
- The prototype runs on two CPU cores with 1 GiB of memory and no network access.
- Hash-map iteration order, thread timing, and input batch boundaries must not affect output.
- Untrusted payloads must return bounded errors rather than panic or allocate without limit.

## Measurable acceptance criteria

- For 50 generated histories and 20 delivery schedules each, all replicas end byte-identically.
- The result must match the un-compacted TypeScript baseline's canonical JSON for every history.
- Every reconnecting actor aged 30 days or less must apply its pending valid operations successfully.
- An actor older than 30 days must receive one deterministic `snapshot_required` decision.
- Duplicate delivery must never change state, counters, or the final digest.
- A conflicting reuse of an operation ID must be rejected and must not change visible state.
- Compacted retained bytes must be at most 40% of the uncompacted fixture bytes.
- Replaying all 5,000,000 operations must finish within 75 seconds on the declared machine.
- Peak measured resident memory must remain below 768 MiB.
- Applying a 200-operation batch must have p95 latency below 4 milliseconds after warm-up.
- Opening the largest compacted document must finish within 750 milliseconds.
- Twenty randomized input batchings must produce byte-identical snapshots and manifests.
- Every injected publication crash must leave either the prior or next valid generation readable.
- After restart, re-running compaction must be idempotent and produce the same generation digest.

## Explicit non-goals

- Do not build a browser interface, drawing engine, or WebSocket server.
- Do not redesign conflict semantics or introduce user-visible conflict resolution.
- Do not implement branching undo, comments, presence, cursors, or access control.
- Do not replace PostgreSQL or test horizontal database coordination.
- Do not support clients offline for more than 30 days without a snapshot reset.
- Do not compress media attachments or inspect their contents.
- Do not claim Byzantine protection; authenticated service clients are assumed.

## Compact self-contained prototype boundary

- Build one Rust library plus one command-line harness.
- Input is a directory of NDJSON operations, actor watermarks, and crash schedules.
- The library exposes validate, apply, compact, reopen, and canonical-snapshot operations.
- The harness can run the baseline oracle and compare canonical outputs and error codes.
- Store prototype state in one replaceable local directory, separate from fixture input.
- Use only the six declared operation kinds and the existing merge rules described above.
- Generate the 100-document fixture from a fixed seed and save only its recipe and digests.
- The deliverable is source, tests, benchmark output, and a short fit decision.
- Time-box implementation and investigation to two developer days.

## Fault, crash, and adversarial cases

- Deliver valid operations in reverse actor order and in 20 random interleavings.
- Repeat individual operations and whole batches before and after restart.
- Reuse an operation ID with a different payload, dependency clock, or document ID.
- Supply missing dependencies, dependency cycles, actor-sequence gaps, and sequence overflow.
- Delete an object concurrently with property edits, moves, and connector changes.
- Reinsert list elements whose neighbors were deleted and later compacted.
- Reconnect actors at 29 days 23 hours 59 minutes and at 30 days plus one minute.
- Truncate an input line and supply invalid UTF-8, oversized payloads, and unknown operation kinds.
- Terminate after generation creation, data flush, manifest flush, rename, and directory sync.
- Simulate disk-full errors at each publication stage and retry with the same input.
- Corrupt the newest generation manifest and confirm deterministic fallback or hard failure.

## Evidence for the fit decision

### Keep the baseline

- Keep it if the prototype fails any convergence, offline-window, or crash-safety criterion.
- Keep it if retained-byte reduction is below 60% after equivalent metadata is counted.
- Keep it if integration requires changing operation semantics, IDs, or the production authority.
- Keep it if the new core is slower and does not remove a meaningful amount of custom logic.

### Adopt a component

- Adopt only the narrow component that passes every correctness and recovery test.
- Require at least a 60% byte reduction, the latency targets, and stable memory use.
- Require a clear boundary that lets PostgreSQL and the TypeScript oracle remain in place.
- Require ordinary error handling and enough inspectability to explain retained metadata.
- Prefer staged shadow replay on recorded histories before any production read path changes.

### Conclude no fit

- Conclude no fit if safe collection needs causal semantics the candidate cannot represent.
- Conclude no fit if complete-generation publication needs an external transaction it lacks.
- Conclude no fit if bounded adversarial input handling depends on pervasive custom wrappers.
- Conclude no fit if using it adds storage duplication without replacing baseline complexity.
- A no-fit result is acceptable and should preserve all measured counterexamples.
