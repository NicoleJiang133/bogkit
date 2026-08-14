# Trial 1: Crash-safe branching undo history for a vector editor

## Developer persona and Rust experience

You maintain the document core of a mature macOS vector-illustration editor. You have eight years of Swift and C++ experience and six months of part-time Rust experience. You are comfortable with ownership basics, enums, tests, and common collection types, but have not designed a Rust persistence layer.

## Existing system and baseline

The editor stores each saved document as canonical JSON. During a session it keeps an in-memory command stack and writes a full-document autosave every 30 seconds. Undo history disappears after a restart, and a crash between autosaves can lose acknowledged edits. Large illustrations make full-document autosaves visibly pause the UI.

For this trial, preserve the canonical JSON format and implement a small reference baseline: append each committed command group as one checksummed JSONL record, write a full canonical JSON snapshot every 2,000 committed groups, and rebuild the in-memory undo stack from the latest valid snapshot plus its tail. Use the same flush policy, fixtures, and host for both implementations.

## Concrete pain

Add persistent undo and redo without making ordinary edits slower or letting recovery produce a document that the user never acknowledged. The difficult cases are grouped edits, undo followed by a new edit, compaction while history remains reversible, duplicate retries after an ambiguous crash, and a torn final write.

## Workload and data shape

Build a deterministic generator for one synthetic illustration containing 60,000 objects. An object has a stable 128-bit ID, z-order position, affine transform represented as six fixed-point integers, visibility, fill color, and zero to 32 path points. Do not use floating-point values.

Generate 250,000 submitted actions across these operations:

- create or delete an object;
- move, recolor, or toggle visibility;
- insert, move, or remove a path point;
- reorder an object;
- group two to 20 commands atomically;
- undo or redo one to 50 committed groups; and
- issue a new group after undo, which invalidates the abandoned redo branch.

Every submitted edit group has a stable client-generated ID. Ten percent of edit-group submissions repeat an earlier ID. The generator must retain enough information for an independent in-memory reference model to compute the expected document, active history head, and available undo and redo counts at every checkpoint.

## Operational constraints

- Run entirely offline in one ordinary user-writable directory on one macOS host.
- Use one editor writer process and up to four concurrent read-only inspector threads.
- Keep the canonical saved JSON as the interchange format; the new store is session history, not a new document format.
- Use at most 256 MiB of measured resident memory and 512 MiB of persistent storage after final compaction.
- Complete the 250,000-action fault-free run within 90 seconds, keep p95 edit-group commit latency below 4 ms, and reopen the final document within 5 seconds on the declared host.
- A successful commit response means the group must be present after a process exit. A group interrupted before a response may be wholly present or absent, but never partial. Retrying its stable ID must return the original outcome without applying it twice.
- No unsafe code, network access, external database server, or privileged service.

## Measurable acceptance criteria

1. Across 30 fixed seeds, the candidate's canonical document digest, active history head, and undo and redo counts equal the independent reference model at every 1,000-action checkpoint.
2. A group of two to 20 commands is always all-or-nothing. Undo and redo operate on whole groups. A new group after undo makes the abandoned redo path unavailable.
3. Duplicate group IDs never apply twice and return the same recorded result, including after restart and after compaction.
4. All 400 injected process-exit trials satisfy the acknowledgement rule and reopen automatically. A truncated or checksum-invalid final record may be discarded with a clear offset-only diagnostic; earlier valid history must remain usable.
5. Concurrent inspectors see a complete state from either before or after a committed group, never a partially applied group.
6. Compaction preserves the final document, active head, remaining undo and redo behavior, duplicate-ID outcomes, and at least the most recent 20,000 reversible groups.
7. Repeating a fault-free seed three times produces byte-identical canonical JSON, document digests, command-result transcript, and diagnostics.
8. The resource and timing limits above are met. Report candidate and baseline p50 and p95 commit latency, reopen time, peak resident memory, final disk use, and total code size excluding tests and generated fixtures.
9. Relative to the baseline, the candidate must show at least one material improvement: 40% lower final disk use, 2x faster reopen, or removal of the visible full-snapshot pause measured as a 50% reduction in worst commit latency. It may not regress baseline p95 commit latency or peak memory by more than 25%.

## Explicit non-goals

- Multi-user collaboration, conflict-free replicated data types, cloud synchronization, and network protocols.
- Rendering, hit testing, file-format migration, thumbnail generation, or graphical UI work.
- Power-loss guarantees below the operating system's acknowledged write behavior.
- Infinite history retention; preserving the stated reversible window is enough.
- Encryption, compression research, or recovery from arbitrary middle-of-file corruption.

## Compact self-contained prototype boundary

Create one command-line harness, one candidate history adapter, the JSONL-plus-snapshot baseline, the pure in-memory reference model, a deterministic fixture generator, and tests. The harness accepts `generate`, `run`, `recover`, `compact`, and `inspect` modes. Its observable output is canonical JSON plus a compact JSON metrics and diagnostic record. Do not integrate with the actual editor or build a GUI.

## Baseline comparison required

Run the candidate and reference baseline over identical generated command groups, flush points, process-exit schedules, hardware, and compiler settings. Correctness gates come before speed. Compare the metrics listed in criterion 8 and identify which persistence concepts had to be written outside the candidate approach. A faster result is not a fit if it weakens acknowledgement, grouping, undo, or recovery behavior.

## Fault and determinism cases

- Exit immediately before append, during encoded-record write, after append but before commit completion, after commit completion but before response, during snapshot creation, during snapshot replacement, and during compaction cleanup.
- Truncate the final record at every byte position for a small fixture and at 64 deterministic positions for the full fixture.
- Replay the last 1,000 submitted group IDs after every recovery.
- Interleave four inspector threads at seeded yield points around commits, undo, redo, and compaction.
- Run identical logical groups with input arrays allocated in different orders; canonical outputs and diagnostics must remain identical.
- Attempt an invalid command inside a multi-command group, an undo beyond available history, a redo after branch invalidation, and reuse of a group ID with different content. Each must fail without state change.

## Evidence classification

**Success:** All correctness, recovery, determinism, resource, and baseline gates pass, and the implementation uses the candidate approach for the central journal, transaction, read-view, and compaction behavior rather than as a decorative wrapper.

**Partial fit:** The approach cleanly handles durable grouped commands and ordinary recovery but needs a bounded custom layer for branch semantics, duplicate-ID retention, or compaction; or it passes correctness while missing one performance or storage gate. State exactly which production requirement remains unproved.

**No fit:** It cannot guarantee the acknowledgement and all-or-nothing rules, cannot preserve branch and compaction semantics, exceeds hard resource limits, requires replacing the canonical document model, or is more complex with no measured advantage over the JSONL-plus-snapshot baseline.
