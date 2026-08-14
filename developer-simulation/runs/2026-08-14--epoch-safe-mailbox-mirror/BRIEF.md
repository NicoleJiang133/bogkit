# Trial 2: Epoch-safe mailbox mirror repair for a desktop mail client

## Developer persona and Rust experience

You maintain synchronization for a cross-platform desktop mail client. You have twelve years of production Kotlin and SQLite experience and three months of Rust experience. You can read Rust and write small command-line programs, but you need ordinary APIs and debuggable on-disk state.

## Existing system and baseline

The client mirrors message headers and flags locally so mailbox lists and unread counts remain available offline. The mail server is authoritative. The current SQLite mirror uses mailbox, message, and cursor tables updated in explicit transactions. Its most troublesome recovery code handles an identity epoch change: message numeric IDs can be reused after the server changes a mailbox's `uid_validity` value, so old and new epochs must never mix.

Implement a faithful SQLite baseline with one transaction per accepted response batch, indexed `(mailbox_id, uid_validity, uid)` identity, and staging tables for a full rescan. A completed rescan swaps the staged epoch into view atomically. The candidate and baseline consume the same already-decoded transcript; protocol parsing is not part of the trial.

## Concrete pain

Replace bespoke mirror-repair bookkeeping with a smaller, easier-to-reason-about local projection while preserving exact behavior after disconnects and process exits. Bugs today leave ghost messages, reuse old flags for a new message with the same numeric ID, double unread counts after replay, or expose half of a replacement epoch to the UI.

## Workload and data shape

Generate a deterministic NDJSON transcript for 80 mailboxes, 300,000 live messages, and 600,000 decoded server responses. Mailbox IDs are stable opaque strings. A message identity is `(mailbox_id, uid_validity, uid)`. Store only a 16-byte content fingerprint, internal date as an integer, byte size, and a bit set for `seen`, `answered`, `flagged`, and `draft`; there are no subjects, addresses, bodies, or attachments.

The transcript contains:

- complete mailbox snapshots delimited by begin and commit records;
- incremental message additions, flag replacements, and expunges;
- highest-known change cursors per mailbox;
- duplicate batches after lost acknowledgements;
- mailbox rename and deletion;
- connection loss in the middle of a response batch;
- an epoch change followed by a full snapshot in which numeric IDs may be reused; and
- explicit UI checkpoints requesting message count, unread count, selected message identities, cursor, and mirror status.

At least 12 mailboxes change epoch, 20 are renamed, 10 are deleted and later recreated under the same display name but a new stable ID, and 5% of batches are exact duplicates. Independent reference code applies only complete valid batches and full snapshots.

## Operational constraints

- Run offline on one macOS host using generated transcripts and one ordinary user-writable directory.
- The remote transcript is authoritative; the local mirror is disposable derived state and must never invent a server change.
- One sync writer and up to eight concurrent UI readers may access the mirror.
- Keep peak resident memory at or below 384 MiB and persistent storage at or below 1.5 times the measured SQLite baseline.
- Process the full 600,000-response transcript within 75 seconds, answer count checkpoints at p95 below 10 ms, and reopen within 5 seconds on the declared host.
- Do not use network access, real mail, personal data, an external database server, unsafe code, or privileged services.

## Measurable acceptance criteria

1. Across 30 fixed seeds, every checkpoint exactly matches the independent reference model for visible identities, flags, per-mailbox total and unread counts, cursor, display name, and status.
2. Replaying a complete batch any number of times is idempotent. Replaying a partial batch after recovery yields the same state as one complete application.
3. A changed `uid_validity` prevents every prior-epoch message and flag from entering the replacement epoch, even when numeric IDs are reused.
4. UI readers observe either the last complete old epoch or the complete new epoch during a rescan, never a mixture. A disconnect, invalid response, or process exit before snapshot commit leaves the old epoch visible and marks the mailbox as needing repair.
5. Rename preserves the stable mailbox identity and its messages. Deletion removes that stable identity. Recreating the same display name under a new stable ID does not resurrect prior state.
6. All 400 process-exit trials reopen automatically and match the reference at the last complete accepted batch. Corrupt or truncated input produces a mailbox-and-sequence-only diagnostic and no partial mutation.
7. Three runs of each fault-free seed produce byte-identical checkpoint output, final canonical manifest, and diagnostics regardless of input chunk sizes and concurrent-reader schedules.
8. The timing, memory, and storage limits above are met. Report candidate and SQLite baseline throughput, count-query p50 and p95, reopen time, peak resident memory, disk use, write amplification if measurable, and code size excluding tests and generated fixtures.
9. The candidate must reduce non-test mirror and repair code by at least 20% or improve full-run time or reopen time by at least 30%, while remaining within 20% of SQLite's count-query p95. Correctness and atomic visibility remain hard gates.

## Explicit non-goals

- Parsing wire protocol, opening a network connection, authentication, encryption, server discovery, or sending commands.
- Message bodies, attachments, MIME parsing, full-text or semantic search, ranking, or spam classification.
- Uploading local flag changes, resolving two-way conflicts, composing mail, or supporting multiple server replicas.
- Treating local state as authoritative, preserving a replaced epoch for user access, or repairing arbitrary disk corruption.
- Replacing the application's account database or UI.

## Compact self-contained prototype boundary

Build a deterministic transcript generator, an independent pure reference model, a candidate mirror adapter, the SQLite baseline, a concurrent checkpoint reader, and a crash-capable command-line harness. The harness supports `generate`, `apply`, `resume`, `verify`, and `summarize`. It stores only the synthetic fields listed above and emits a canonical JSON manifest plus privacy-safe metrics and diagnostics.

## Baseline comparison required

Run candidate and SQLite baseline with identical decoded batches, commit boundaries, reader schedules, crash points, host, and compiler settings. First compare every checkpoint against the reference, then compare the measurements in criterion 8. Record how full-snapshot staging, epoch replacement, replay detection, cursor storage, and counts are expressed in each implementation. Do not credit a throughput gain obtained by weakening transaction boundaries or reader visibility.

## Fault and determinism cases

- Exit before batch admission, after individual response decoding, immediately before commit, after commit but before acknowledgement, during full-snapshot staging, during epoch publication, and during cleanup of abandoned staging data.
- Truncate every record position in a small transcript and 64 seeded positions in the full transcript; also inject invalid JSON, missing batch terminators, decreasing cursors, unknown mailbox IDs, and an incremental event for an uncommitted epoch.
- Duplicate each batch once, replay the final 10,000 batches after restart, and reorder only responses that the generator marks independent. Reject illegal reorderings without state change.
- Reuse every old numeric ID in at least one replacement epoch with deliberately different flags and fingerprints.
- Schedule eight readers at seeded points during incremental batches, rename, delete, full rescan, epoch publication, and recovery.
- Vary transcript chunk sizes from one byte to 1 MiB and permute input map insertion order; logical outputs and diagnostics must remain identical.

## Evidence classification

**Success:** All correctness, epoch isolation, atomic visibility, crash recovery, determinism, resource, and baseline gates pass, with the candidate approach responsible for the central projection and transaction behavior and at least one measured maintenance or performance gain.

**Partial fit:** Incremental add, flag, expunge, replay, and query behavior is clean, but full epoch replacement, reader isolation, recovery, or one resource gate requires a bounded custom mechanism. Report the exact unproved boundary and retain SQLite for it.

**No fit:** Any run exposes mixed epochs, resurrects deleted identities, loses or duplicates accepted responses, cannot recover deterministically, exceeds hard limits, or requires more bespoke state machinery without the required code-size or performance benefit over SQLite.
