# Discovery trail

## Initial reading, before component source

I am evaluating this as a backend engineer who normally reaches for PostgreSQL transactions, unique constraints, and advisory or row locks. My production baseline would keep a command envelope, its durable idempotency record, the booking mutation, interval-capacity checks, and the append-only decision audit inside one `SERIALIZABLE` PostgreSQL transaction. The transaction would take a database timestamp once, lock one stable warehouse/request scope, replay an existing `(tenant_id, request_id)` result when its canonical payload hash matches, reject a mismatch, expire relevant holds, choose one compatible door in a stable SQL order, write state and audit, and commit them together. PostgreSQL's result is the only authoritative result; a retry after an ambiguous client timeout reads that durable row.

From the public README and examples, BogKit appears to contain three independent building blocks:

- Fold is an embedded, persistent Rust stream whose inserts and removals transactionally update materialized views. The examples give the stream ownership to one process/thread and store data in Fold's own on-disk database.
- ESE turns text into static embeddings. That is unrelated to exact slot admission.
- ANNy is a nearest-neighbor index. Approximate similarity search is also unrelated to exact scheduling constraints.

My initial hypothesis is that Fold could express convenient local projections such as occupancy totals or an audit-derived view, but not the required production commit boundary unless it can participate in a caller-owned PostgreSQL transaction. A second durable database would create exactly the dual-write ambiguity this trial is meant to eliminate. I therefore need to establish whether Fold exposes a PostgreSQL-backed storage adapter, caller-owned transaction hook, write-ahead/outbox integration, or prepare/commit protocol. If it does not, the honest result is likely no-fit for the admission authority, even if a process-local demo can model the rules.

The smallest meaningful evidence will be a deterministic executable admission model that exercises the business semantics and the exact failure boundary: a simulated PostgreSQL transaction commits authoritative booking and audit state, while a separate Fold write can be skipped by an injected process exit. This is not PostgreSQL evidence and will not be presented as such; it is only a reproducer of why an independent Fold commit cannot replace or join the existing transaction. If a real disposable PostgreSQL server is already available, I will additionally use it. I will not install or start one.

## Questions to answer

1. Can Fold use PostgreSQL as its authority or join an existing PostgreSQL transaction?
2. Does Fold support concurrent writers from six independent processes, or does its public shape assume one stream owner?
3. Can a caller atomically persist a stable command outcome and audit record with the Fold state mutation?
4. If the answer is no, is there any smaller non-authoritative role for Fold that materially improves the PostgreSQL baseline without adding a fragile dual-write path?
5. Can the model make expiry-at-confirmation deterministic, keep retries idempotent, provide exact stable reasons, and survive injected exit boundaries?

## Evidence standard

I will distinguish (a) behavior proven by executable deterministic tests, (b) API facts established by minimal source inspection, and (c) production claims that remain unverified without PostgreSQL and independent processes. Passing an in-memory or local-file model is not evidence for serializable PostgreSQL concurrency, database latency, or six-replica crash safety.

## Environment check

No disposable PostgreSQL environment was available. `psql`, `postgres`, and `pg_isready` were absent; there was no listener on TCP 5432 and no PostgreSQL connection environment variable. A Docker CLI existed, but OrbStack was stopped and the Docker socket was unavailable. I did not install anything or start a service. This prevents the database-backed concurrency and performance criteria from being evaluated honestly.

## Minimum source inspection

I searched Fold for PostgreSQL, storage, and transaction integration points, then read its public stream transaction implementation. The dependency manifest names `fjall` and has no PostgreSQL dependency. `Stream` owns a `fjall::SingleWriterTxDatabase`; `Stream::new` opens it from a filesystem path. Each `wtx` calls `self.store.write_tx()` and later commits it internally. `WriteTx` wraps a concrete `fjall::SingleWriterWriteTx`, while `PipelineInitCtx` creates fjall keyspaces. I found no PostgreSQL adapter, external transaction parameter, two-phase prepare, or outbox callback. This confirms the initial transaction-boundary concern rather than merely inferring it from an example.

ESE and ANNy solve semantic encoding and approximate-neighbor lookup, respectively. Neither contributes to exact slot constraints or transaction ownership, so adding them would be unjustified.

## Prototype and debugging trail

I built `receiving-slot-admission`, a Rust CLI with three intentionally separated claims:

1. A deterministic local authority model exercises the domain semantics over 24 doors. It uses logical timestamps, durable-result-style idempotency, exact reason codes, a 10,000-command fixture, 100 generated seeds, four-thread collision attempts over 30 runs, and invariant checks.
2. A real child-process crash harness exits immediately before and after a mocked atomic authority commit. A Fold `KeyedStream` is a separate durable decision mirror. The after-commit exit reliably leaves authoritative booking plus audit present while Fold has no row, directly demonstrating the dual-commit gap.
3. A repeat-heavy 100,000-command local benchmark ensures the harness is runnable, but is printed with an explicit warning that it is not PostgreSQL performance evidence.

The first test run found a real bug in my model: two overlapping holds could both be confirmed, violating the explicit confirmed-booking invariant. I added a deterministic conflict check at confirmation time; the second full run passed. During final review I also noticed that hold expiry used a hard-coded 15 instead of the request's validated lifetime, corrected it, and reran every check. These were prototype defects, not BogKit defects.

The skeptical review then found two more model defects and one verification defect. Expired holds remained active for unrelated capacity decisions unless an explicit sweep ran; I now expire due holds from the injected command timestamp before every new, non-replayed command and added one-door regressions for a new hold and a reschedule. Reschedule also accepted durations outside `1..=8`; durations 0 and 9 now reject without changing the booking or version. Finally, the originally stated strict lint gate omitted pedantic lints and failed with checked-conversion, elapsed-time precision, and naming diagnostics. I replaced every reported truncating conversion with a checked conversion carrying its bounded-value reason, retained benchmark samples as `Duration`, renamed the similar binding, and added no lint allowances. A distinct wrong-version test now proves `stale_reschedule_version` rather than confusing it with payload mismatch on reuse of an existing operation key.

After those corrections, the full gate passed with eight tests and strict pedantic Clippy. The resulting fit decision remained no-fit. Fold can atomically update its own materialized views, but the required unit is the existing PostgreSQL state plus PostgreSQL audit. Repairing a Fold mirror on retry is possible, yet that is eventual reconciliation, not the required same-transaction guarantee. An outbox could make a non-authoritative projection reliable, but no useful projection in this admission path justified the extra persisted subsystem.
