# Trial 1 brief: Incremental calculation cache for a financial-planning desktop app

## Developer role and Rust experience

You maintain the document core of an established desktop financial-planning application.
You have nine years of production C# experience and eight months of Rust experience.
The calculation core is already Rust, called from the existing C# user interface.
You are comfortable with ownership and tests but have not built a persistent incremental dataflow engine.

## Existing system and baseline implementation

Customers edit large `.planbook` documents used for budget and headcount planning.
Each document has a checksummed append-only edit journal as its authoritative record.
The journal assigns every accepted edit batch a strictly increasing 64-bit sequence number.
Periodic calculated snapshots are disposable and may always be rebuilt from the journal.
The current Rust baseline replays edits, builds an in-memory workbook, and fully recalculates every formula after each accepted batch.
It is trusted because its exact scaled-integer arithmetic and error propagation have mature conformance tests.
It is slow on large workbooks: an ordinary one-cell edit blocks the interface for 1.8 to 4.2 seconds.
Do not replace the reference evaluator or change the journal format in this trial.

## Concrete pain point

Most edits affect a small dependency region, yet the baseline recalculates all 260,000 formulas.
An earlier hand-built invalidation cache missed dependencies hidden inside large ranges.
It also exposed values from two edit generations to concurrent export requests.
The team wants to evaluate whether a data materialization toolkit can maintain a disposable calculated snapshot exactly and publish it atomically.
A clean no-fit conclusion is preferable to weakening workbook semantics.

## Workload and data shape

Use one deterministic synthetic workbook with exactly 1,000,000 populated cells across 160 worksheets.
Exactly 740,000 cells are literals and exactly 260,000 cells are formulas.
Formula expansion yields 2,400,000 directed dependency edges.
The formulas contain direct references, rectangular ranges, arithmetic, comparisons, `IF`, `SUM`, `MIN`, and `MAX`.
All numeric values are signed 128-bit integers representing millionths; floating-point arithmetic is absent.
The fixture includes 12,000 range aggregates, 400 intentional dependency cycles, and 2,000 stable arithmetic or missing-reference errors.
Cycle membership and error values are part of the expected calculated state.
Replay one trace of exactly 10,000 accepted edit batches.
Exactly 7,000 batches change one to five literal cells.
Exactly 2,000 batches replace one formula with another valid formula.
Exactly 1,000 batches insert or delete a row and apply the existing reference-rewrite rules.
The three batch classes therefore sum to all 10,000 accepted batches.
The trace never deletes a worksheet and never exceeds the stated one-million-cell fixture size by more than 2 percent.

## Operational constraints

The authoritative journal remains byte-for-byte unchanged.
The candidate state must live in one local data directory and require no service or network access.
The target machine has four CPU cores and 2 GiB of memory available to the process.
The candidate's on-disk calculated state must remain at or below 600 MiB.
Initial construction from the complete journal must finish within 90 seconds.
Reopen from a valid calculated snapshot must finish within 8 seconds.
The application has one edit writer and twelve concurrent read-only callers for visible cells, exports, and totals.
Every read request must observe one complete accepted generation.
No caller may observe a mixture of sequence numbers within one response.

## Compact prototype boundary

Build a command-line prototype around the existing reference evaluator and supplied deterministic fixture generator.
Implement only ingestion of the canonical journal records, dependency maintenance, recalculation, snapshot publication, point reads, and rectangular-range reads.
Retain the reference evaluator as the oracle and benchmark baseline.
It is acceptable to write a small adapter between journal records and candidate-toolkit records.
Exercise one process and one local directory; do not add a server.
Run the writer and twelve reader tasks in the same process for concurrency tests.
Produce a machine-readable result manifest containing generation, timings, sizes, and pass or fail gates.
Keep the prototype small enough to discard if the evaluation is negative.

## Measurable acceptance criteria

After every one of the 10,000 batches, calculated values and stable errors must exactly equal the full reference evaluator.
The final calculated snapshot must be byte-identical across five clean builds.
The final snapshot must also be byte-identical when the journal is fed in chunk sizes of 1, 7, 64, 511, and 4,096 records.
For the 9,000 non-structural batches, recalculation latency must have p95 at or below 25 milliseconds and p99 at or below 75 milliseconds.
For the 1,000 structural batches, latency must have p95 at or below 2 seconds.
Initial construction, reopen time, memory, and disk usage must meet every stated operational limit.
During a 30-minute mixed writer-reader run, all twelve readers must return a self-consistent generation on every request.
The test harness must complete at least 1,000,000 read requests in that run with zero mixed-generation responses.
Invalid journal records must leave the last accepted generation and its calculated values unchanged.
A candidate that misses any equality, crash, or generation-consistency gate is not a fit regardless of speed.

## Failure and crash cases

Test a bad checksum, duplicate sequence, skipped sequence, truncated record, unknown formula opcode, and integer overflow.
Reject each bad record before it changes the published generation.
Inject 200 real process exits at deterministic points spanning journal append, candidate update, checkpoint creation, and generation publication.
After every restart, replay the authoritative journal and compare the visible generation with the reference evaluator.
No acknowledged journal batch may be missing after restart.
No unacknowledged or partial batch may become visible.
A corrupt calculated snapshot must trigger a rebuild from the journal rather than corrupt the journal or return guessed values.
Disk-full injection during checkpoint creation must preserve the prior readable generation.

## Concurrency, privacy, and determinism cases

Run readers continuously while formula and structural batches publish new generations.
Include ranges whose cells span both changed and unchanged dependency regions.
Use deterministic tie ordering for cycle reports: worksheet ID, row, then column.
Run all equality checks under one thread and four worker threads.
Thread count must not change any calculated value, error, cycle membership, or serialized byte.
Fixture values are synthetic, but treat them as confidential salary-plan data.
Logs and error messages may contain sheet IDs, cell coordinates, sequence numbers, and error codes.
Logs and error messages must not contain literal cell values, formula text, or workbook titles.
The prototype must make no outbound network request.

## Explicit non-goals

Do not implement the user interface, collaboration, cloud sync, permissions, or file-format conversion.
Do not add new formula functions, volatile time or random functions, macros, external workbook links, or floating-point compatibility.
Do not optimize initial journal durability or redefine when an edit is acknowledged.
Do not replace the authoritative journal with candidate state.
Do not claim compatibility with arbitrary spreadsheets outside the declared formula subset.
Do not turn the prototype into a general reactive programming framework.

## Decision rule

Recommend continued evaluation only if every exactness, failure, privacy, concurrency, and determinism gate passes.
Report performance separately for literal, formula, and structural batches.
If correctness passes but targets do not, report local correctness proof only.
If the toolkit requires authority over the edit journal or cannot publish generation-consistent reads, report no fit.
