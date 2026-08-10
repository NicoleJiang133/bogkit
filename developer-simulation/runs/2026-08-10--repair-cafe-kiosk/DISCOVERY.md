# Discovery log

## Brief-first understanding

I am the volunteer maintainer described in `BRIEF.md`: experienced with Python and SQL, but only intermediate in Rust. The replacement kiosk is deliberately a small, offline, single-writer system. Its hard problems are durable event ordering, referential and transition validation, atomic history/current-state updates, deterministic replay, and understandable recovery. Approximate search is useful but secondary.

My conventional baseline is an embedded SQL database with one append-only event table, item and borrower tables, a current-item table updated in the same transaction, uniqueness and foreign-key constraints, ordinary normalized lookup indexes, and either built-in full-text search or a small application-level fuzzy matcher. I expect this to take about two days and to give future volunteers familiar inspection, backup, and recovery tools.

## Public-material discovery order

1. Read the trial brief in full.
2. Read the root `README.md`.
3. Read all four public examples (`starter`, `timeseries`, `search`, and `chat`) and their manifests.
4. Wrote this initial hypothesis before inspecting implementation source or internal documentation.

## Initial component-fit hypothesis

- **Fold: plausible partial fit, requiring proof.** The public examples show one write transaction updating multiple persistent materialized views and one consistent read snapshot. That directly targets the split history/current-state failure. `KeyedStream` and `Table` may also support stable identifiers. I need to determine whether I can validate duplicate IDs, item/borrower references, and legal transitions before a write while keeping accepted events and current state atomic, and whether ordered history-at-time queries avoid a full scan.
- **ESE: likely not a fit.** Semantic embeddings are not required for asset tags, serial numbers, abbreviations, names, or nicknames. The kiosk has a strict 256 MiB budget and must remain easy to maintain offline. Normalization plus substring/token matching is the baseline until measurements show it misses the golden search set.
- **ANNy: likely not a fit.** Eight thousand inventory rows do not justify an HNSW index unless embedding search materially improves the fixed lookup set. It adds an approximate index and operational concepts while every selected result still needs canonical-ID resolution.

## Baseline and trial plan

I will first implement the smallest Fold-backed event/current-state prototype that can expose the important boundaries: master records, accepted events indexed by ID, current state by item, and an event history view. Validation will happen in the single writer before opening the Fold write transaction; the transaction must then update every derived view together. Tests will cover legal transitions, duplicate IDs, unknown references, deterministic replay, reopening, and unchanged snapshots after rejection.

I will use a generated representative fixture only if the small correctness model succeeds. The performance run should include 8,000 items, 1,200 borrowers, and 100,000 accepted events, then measure reopen, 1,000 warm exact lookups, deterministic rebuild, and a 50-query normalized search set. I will model interruptions at the application-visible boundaries that the public API exposes and separately document what that does and does not establish about real crash durability.

The adoption decision will compare this against embedded SQL on code and dependency burden, backup/recovery clarity, schema/constraint support, history query shape, and failure behavior. A successful Fold demo is not enough: Fold must beat or materially simplify the baseline for this particular kiosk.

## Discovery and debugging trail

- Source inspection confirmed that one `wtx` uses one underlying Fjall write transaction for every pipeline sink. `Tx::rtx` flushes buffered operators into that still-uncommitted transaction, and a panic aborts both the underlying transaction and pending pipeline state. This was stronger and clearer than I expected from the short README.
- I first tried the default `cargo test`; dependency resolution attempted to reach crates.io and failed because this checkout has no network. `cargo test --offline` used the existing cache and succeeded. I treat this as an execution-environment event, not a BogKit defect.
- The first search fixture scored 42/50 because I accidentally generated some nickname queries for the wrong tool category. I fixed the fixture, not the ranking algorithm; the corrected, category-matched golden set scored 50/50. This is prototype evidence only because the queries are synthetic.
- The first memory harness rebuilt three databases sequentially in one process. Its allocator retained pages and the peak rose from 130.9 MiB after import to 260.2 MiB after the third rebuild, just above the kiosk budget. I corrected the experiment so the three stated "runs" are genuinely separate child processes and compare streamed canonical-view files byte-for-byte. Corrected runs observed 172.4-173.6 MiB coordinator peaks and 130.1-172.7 MiB rebuild-child peaks, all under the limit.
- `/usr/bin/time -l` could not report resident memory in this sandbox (`sysctl kern.clockrate: Operation not permitted`). The final harness reads its own `getrusage(RUSAGE_SELF)` value, adding only a direct `libc` declaration for measurement; `libc` was already transitive through Fold's storage stack.
- The interruption child now forces a mid-transaction `rtx`, proving the event and projection are visible inside the uncommitted transaction, and then aborts the process. Reopening sees the complete old state. A separately committed write reopens with the complete new event and projection.

## Final fit conclusion

Fold can implement this kiosk correctly at the representative scale, and its atomic multi-view transaction is genuine value. I would still choose embedded SQL for the production kiosk. The requirements are dominated by uniqueness, references, state-transition checks, ordered history, ad hoc inspection, and boring backup/recovery. Fold leaves those rules in application code and requires bespoke typed pipeline plumbing, while the conventional baseline expresses most of them in familiar tables, indexes, constraints, and transactions.

ESE and ANNy remain no-fit. The 50-query normalized lookup set passed without embeddings or approximate vectors. Moreover, selecting Fold alone still brought ANNy into the normal dependency graph because `fold` depends on it unconditionally; that weakens the component-selective story for a small offline application.

## Skeptical review: ordering failure and correction

The reviewer found an important prototype bug in my caller-supplied event ordering. Before the fix, this exact sequence reproduced it:

1. accept a checkout with `sequence=10`;
2. submit a distinct return with `sequence=5`;
3. live preflight sees the currently checked-out state and accepts the return;
4. the aggregate sorts history as return-before-checkout and `replay(...).expect(...)` panics with `accepted event history must replay`.

The root cause was a mismatch between the write boundary and the materialization rule: validation considered only current state, while replay treated the caller's sequence as authoritative historical order. Because the reviewer separately confirmed that a caught Fold write panic poisons the current writer lock, this prototype panic could also make later valid writes fail. That consequence strengthens the no-fit decision: application invariants around Fold must be explicit and must be checked before entering `wtx`.

I chose a strict, global append-only contract. Every accepted event sequence is unique and must be greater than the latest accepted sequence across the kiosk. Live writes now scan accepted events to reject a reused or non-increasing sequence before `wtx`; import validation applies the same checks and advances the ID/sequence sets only for accepted rows. Backdated insertion is intentionally unsupported. A correction must arrive as a new later event rather than being inserted into prior history.

Two permanent regressions cover the policy. A table-driven live test accepts checkout 10, rejects return 5 and a distinct return 10 with exact errors, proves event history/current state are byte-for-byte unchanged after each rejection, then commits return 11 successfully in the same writer. A separate import test accepts sequences 10 and 11 while reporting sequence 5 as out-of-order and the second 10 as duplicate. The transactional `expect` remains only as an internal invariant assertion: caller-controlled ordering and transitions are now validated under the stated single-writer rule before entering the transaction.

The small correction deliberately scans all accepted event rows to find the latest sequence and detect reuse, making each live command O(total events). That is correct for this bounded proof but not a production-grade sequence allocator or index. I am recording the cost instead of adding another materialized view or subsystem to a trial whose recommendation remains embedded SQL.

During final parallel test verification, one test intermittently failed to open its database with Fjall `Locked`. The harness generated paths from process ID plus wall-clock nanoseconds, so concurrently starting tests could still collide at the host clock's effective resolution. I added an atomic per-process suffix to every generated path. Three consecutive six-test runs then passed. This was a prototype fixture-isolation issue; it is not included as BogKit evidence.
