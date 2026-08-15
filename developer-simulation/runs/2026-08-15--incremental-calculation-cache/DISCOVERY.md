# Discovery and debugging trail

## 2026-08-15 — Baseline-first hypothesis (before internals)

The public README and examples suggest that **Fold is the only plausible candidate** for this trial. Its public API shows durable materialized views, transactional writes, retractions, keyed replacement, and one read transaction spanning several views. Those properties may fit a disposable calculated snapshot and generation-consistent reads.

The main uncertainty is deeper than simple aggregation. The workbook needs a dynamic dependency graph, range-derived dependencies, deterministic cycle/error state, formula replacement, and row edits that rewrite references. The examples demonstrate incremental count/table/search maintenance, but not graph reachability, topological recalculation, cycle handling, or atomic publication of a large externally evaluated state. I expect a small adapter may be able to publish an already-computed generation atomically, but I do not yet have evidence that Fold itself can perform the workbook dependency/recalculation work efficiently or safely.

ESE and ANNy appear unrelated: ESE produces text embeddings and ANNy provides approximate nearest-neighbor search. Financial formulas need exact signed 128-bit values and exact dependency semantics; approximation or embedding would be a correctness failure. I therefore expect both to be no-fit unless later evidence reveals an exact non-search facility.

### Baseline plan and falsifiable questions

1. Run the smallest public Fold example unchanged to establish that the detached baseline builds and that insert/retract plus consistent reads actually work here.
2. Inspect only the Fold surfaces needed to answer: can a single transaction atomically update a generation marker and calculated cells, can a rejected input leave both untouched, and can reopening recover that accepted state?
3. Write acceptance tests first for a deliberately tiny reference evaluator and journal subset. The critical break each test must catch is mixed publication (new generation with old values or vice versa), acceptance of an invalid sequence/checksum/overflow, nondeterministic serialization, or failure to reopen the last accepted state.
4. If the public API cannot support those properties without taking ownership of the authoritative journal, retain the smallest exact failure reproducer and conclude no-fit.

### Claims explicitly out of scope for the compact run

The brief's million-cell/10,000-batch workload, 30-minute/1,000,000-read concurrency run, 200 real process exits, 2 GiB memory cap, disk-full injection, and production percentile targets cannot honestly be established by a small time-bounded trial unless fully executed. Any smaller run will be labeled local functional evidence only and will not be used to recommend continued evaluation under the brief's all-gates decision rule.

## Baseline observation

`cargo run --locked -p starter` did not reach the public demo. Although the starter source uses only Fold, its manifest declares ANNy and ESE too. Cargo therefore ran ESE's build script, which attempted to download a model and failed at DNS resolution. This is meaningful baseline failure evidence for the trial's no-network constraint: the advertised smallest example is not offline-buildable from this sanitized checkout as declared. No network request succeeded, and no application was run. The trial integration should depend only on Fold so it does not activate ESE or ANNy.

## Internals and test-first checkpoint

The minimal relevant internals confirm that a Fold write transaction commits its sinks together, a read transaction pins one database snapshot, a `KeyedStream` retracts a replaced row before inserting its replacement, and the table sink supports point reads plus ordered iteration. They also show an API boundary: `Stream::wtx` needs exclusive mutable access while `Stream::rtx` borrows the same stream. There is no public clonable read handle in the inspected surface, so simultaneous application threads cannot directly hold independent Fold read transactions while a writer uses the stream. The public chat example works around this by publishing cloned snapshots through a watch channel.

Acceptance tests were written before behavior. The first compile correctly failed because the trial library did not exist. After adding only API-shaped placeholders, the focused test produced meaningful RED: `range_dependency_recalculates_and_publishes_one_generation` panicked at the explicit unimplemented bootstrap path. The next step is the smallest real evaluator, dependency invalidation adapter, Fold-backed atomic persistence, and in-process publication needed to make that contract green.

The first implementation run reached real behavior and failed only on the test's ordering assumption. The implementation returned cells in the brief's required worksheet/row/column order (`B1`, `C1`, then `A2`); the test had incorrectly put the directly edited `A2` first. I corrected the hand-written expectation to the declared deterministic coordinate ordering rather than changing the implementation.

The first full acceptance run passed six of nine tests and exposed two implementation/fixture issues. The evaluator accidentally published a synthetic value for a referenced-but-nonexistent cell in addition to the real formula's missing-reference error; publication is now restricted to populated workbook cells. Separately, the formula-replacement fixture used a threshold that made the expected branch unreachable; the deterministic fixture threshold was corrected so the hand-computed expected value exercises dependency rewiring and conditional arithmetic.

## Green integration and decision trail

The retained prototype now passes its bounded tests: exact hand-derived values for range invalidation and formula replacement, deterministic cycle/missing-reference ordering, point and rectangular reads, reopen, validation-before-publication, five logical clean builds, the five requested input chunk groupings, and 24,000 reads across twelve tasks with no mixed-generation response. The concurrency result belongs to the adapter's cloned immutable snapshot plus `RwLock`, not to a clonable Fold database reader.

The release demonstration also made the evidence boundary concrete. A three-batch seven-cell store reopened in milliseconds and occupied only a few kilobytes after close, while the live 100-batch concurrency store occupied roughly 64 MiB. That is useful measured local evidence, but neither figure predicts a million-cell, 10,000-batch store. The manifest separately labels the production-scale gates `NotRun`.

Repository search found no `.planbook` evaluator or declared million-cell fixture generator outside `BRIEF.md`. The compact prototype therefore uses a deliberately tiny independent evaluator rather than claiming equivalence to the unavailable production oracle.

Decision: **no fit for the full brief as currently evidenced**. Fold is useful as a transactional persistence primitive, but the public/internally documented surface inspected here does not supply workbook dependency maintenance, structural reference rewrites, direct concurrent reader handles, or the full crash/recovery harness. The external adapter implements only enough to prove a narrow publication pattern; the mandatory large-scale correctness, structural, crash, disk-full, memory, disk, and long concurrency gates remain unrun. Under the brief's all-gates rule, local success cannot justify continued evaluation. ESE and ANNy remain clear no-fit components for exact financial calculation.

## Fix round 1 — skeptical-review lifecycle corrections

The skeptical reviewer reproduced three archive-evidence defects:

1. a second `bootstrap()` rewound generation 1 to generation 0 and reopened the sequence-1 gate;
2. `Trial::open` on used calculated state returned a fresh in-memory view instead of rejecting; and
3. read-back `reopen` restored calculated cells but not formula/literal definitions, so the next batch after a formula replacement diverged from uninterrupted execution.

I wrote three permanent regressions before changing the lifecycle. The first run produced the expected RED evidence: repeated bootstrap returned success, open-on-existing returned success, and close/reopen/continue produced different generation-3 values from uninterrupted execution.

The lifecycle is now explicit and one-way. `open` accepts only unused calculated state; `bootstrap` accepts only a fresh instance. `recover_from_journal` is the only restart-to-writable path. It replays the complete caller-owned accepted journal prefix into the initial workbook without publishing, evaluates that reconstructed workbook, and requires exact equality with the persisted calculated snapshot before enabling the next write. Repeated initialization and recovery mismatch return errors without a Fold publication transaction.

The repaired regressions are GREEN. The continuation case applies a literal edit, replaces a range formula, closes the store, recovers from those two authoritative records, applies sequence 3, and compares the complete snapshot and canonical logical bytes with uninterrupted execution.

The demo's previous `reopen_gate` meant read-back only and was too broad. It is replaced by `recovery_continuation_gate`; the measured `recovery_replay_microseconds` covers journal replay plus validation before the next write, while the gate itself passes only after that next write matches uninterrupted execution.

Execution-path clarification: affected-region discovery is computed and checked, but it does not bound work. Every accepted batch still fully evaluates the entire fixture and republishes every calculated cell through Fold. This remains a narrow atomic-publication proof, not incremental recalculation evidence, and the no-fit decision is unchanged.
