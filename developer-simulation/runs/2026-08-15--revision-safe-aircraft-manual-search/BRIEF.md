# Trial 2 brief: Revision-safe offline aircraft-maintenance manual search

## Developer role and Rust experience

You maintain the offline search core of an established aircraft-maintenance tablet application.
You have eleven years of production Kotlin experience and four months of Rust experience.
You can integrate a Rust library through the application's existing native boundary.
You have maintained SQLite full-text search but have not shipped a vector or hybrid retrieval system.

## Existing system and baseline implementation

The application installs signed, licensed manual packages while a tablet is connected at a maintenance base.
It is then used without network access on the hangar floor.
The signed package and its manifest are authoritative; any local search index is disposable.
An existing, separately tested parser emits canonical section records and validates package signatures.
Each record carries a stable section ID, revision ID, effective interval, title, body, identifiers, and an aircraft-applicability expression.
The current baseline stores active text in SQLite full-text search and filters aircraft applicability after retrieving the top 20 lexical matches.
It performs well for exact fault codes but poorly for technicians' terse symptoms and vocabulary differences.
Post-ranking filtering can also remove all 20 candidates even when an eligible section ranked lower.
The application team will retain the package parser and signature verifier.

## Concrete pain point

Technicians search phrases such as "pump chatters after cold soak" when the manual says "intermittent cavitation at low fluid temperature."
Improving recall is valuable, but returning a section for the wrong aircraft configuration or superseded revision is unacceptable.
The team wants to evaluate a local hybrid index that applies hard eligibility before final ranking and returns exact evidence.
The index must survive interrupted updates without making search unavailable.
A relevance improvement that weakens eligibility, privacy, or reproducibility is a no-fit result.

## Workload and data shape

The deterministic fixture contains exactly 2,400,000 section revisions from 600 manuals.
Source text and metadata total 28 GiB after the existing parser emits canonical records.
There are 400 aircraft configuration snapshots across 12 aircraft families.
Each configuration supplies exactly 160 boolean, categorical, or bounded-integer attributes.
Applicability expressions contain AND, OR, NOT, equality, membership, and closed integer ranges.
At a selected effective time, 780,000 section revisions are active for the median configuration.
Each stable section ID has between one and nine revisions, with exactly one active revision when the input is valid.
Identifiers include fault codes, maintenance task numbers, part numbers, and standardized subsystem codes.
Text includes abbreviations and approved synonyms in English; multilingual retrieval is outside this trial.
Use a private judged benchmark of exactly 600 queries.
Exactly 200 queries are identifier lookups, 200 are terse symptom descriptions, and 200 are paraphrases.
Every judged query specifies an aircraft configuration snapshot and effective time.
Use a separate deterministic latency trace of 5,000 queries in the same three query classes.

## Operational constraints

All indexing and queries run on a four-core CPU-only tablet with 8 GiB physical memory.
Peak process memory must remain at or below 2 GiB.
The complete local search generation must remain at or below 9 GiB on disk.
An initial build from canonical records must finish within 45 minutes.
A fortnightly delta generation containing up to 120,000 changed section revisions must activate within 8 minutes.
There is no query-time or build-time network access.
The application may issue sixteen concurrent searches while one updater builds the next generation.
Existing readers must remain on the old complete generation until the new complete generation atomically activates.
The 28 GiB source package is outside the search-index disk limit and remains read-only.

## Compact prototype boundary

Consume the existing parser's canonical records and a validated manifest; do not parse original publisher formats.
Build only eligibility data, lexical and optional semantic retrieval data, deterministic ranking, exact citations, and generation activation.
Compare against the existing SQLite full-text baseline on the same records and queries.
If semantic retrieval is attempted, use only locally bundled model files with fixed versions and checksums.
Return the top ten eligible section revisions with score components and exact source references.
Source references must include manual ID, stable section ID, revision ID, and effective interval.
Exercise one active generation and one staging generation in a single local data directory.
Do not integrate with the production tablet shell or publisher download service.
Produce a machine-readable report containing gates, recall, latency, memory, disk, and generation hashes.

## Measurable acceptance criteria

Across all 600 judged queries, every returned item must satisfy the selected aircraft applicability expression and effective time.
Across all 600 judged queries, every returned item must be the active revision for its stable section ID.
Identifier queries must achieve 100 percent recall at 5.
Terse symptom queries must achieve at least 92 percent recall at 10.
Paraphrase queries must achieve at least 82 percent recall at 10.
The candidate must improve recall at 10 by at least 15 percentage points over the baseline on the combined 400 symptom and paraphrase queries.
On the 5,000-query latency trace, p95 must be at or below 120 milliseconds and p99 at or below 250 milliseconds.
All 5,000 latency queries must return without network access or generation mismatch.
Initial build time, delta activation time, memory, and index size must meet every stated operational limit.
Five clean builds of the same package must produce the same generation hash.
The same query set must return the same ordered section and revision IDs across all five builds.
Any ineligible or superseded result is an automatic no-fit outcome regardless of recall or speed.

## Failure and crash cases

Test a bad package signature, manifest hash mismatch, truncated canonical record, duplicate revision ID, and missing parent generation.
Test two active revisions for one stable section ID and an applicability expression with an unknown attribute.
Every such invalid input must reject the entire staging generation and preserve the active generation.
Inject disk-full failures during record ingestion, index finalization, and activation.
Inject 120 real process exits at deterministic boundaries across initial build, delta build, verification, and activation.
After restart, queries must use either the complete old generation or the complete new generation, never a mixture.
Orphaned staging data may be resumed or discarded, but it must not be searchable before complete validation.
Corruption detected in the active generation must fail closed with a clear local error; it must not silently return unchecked results.

## Concurrency, privacy, and determinism cases

Run sixteen query workers continuously during each failure-free and crash-injected update.
Tag every response with one generation hash and verify that all result evidence belongs to that generation.
Tie-break equal scores by stable section ID and then revision ID.
Fix tokenization, model checksum if used, score rounding, and candidate merge order.
Changing worker count from one to four must not change ordered result IDs or source references.
Manual text is licensed and must never leave the device.
Queries may contain tail numbers or technician-entered observations and must not be stored after the response.
Logs may include query class, timing, generation hash, and error code.
Logs must not include raw queries, manual text, tail numbers, embeddings, or snippets.
The test harness must verify zero attempted outbound connections.

## Explicit non-goals

Do not parse publisher PDF, XML, images, diagrams, or video in this prototype.
Do not perform optical character recognition, speech input, translation, answer generation, or conversational follow-up.
Do not change manual text, repair applicability metadata, or infer missing revisions.
Do not implement package download, authentication, authorization, fleet assignment, or maintenance writeback.
Do not learn ranking from technician clicks or send telemetry.
Do not claim airworthiness approval or replace technician judgment.
Do not broaden results by ignoring uncertain eligibility.

## Decision rule

Recommend continued evaluation only if every eligibility, revision, failure, privacy, concurrency, and determinism gate passes.
Report exact, symptom, and paraphrase recall separately from latency and storage costs.
If safety gates pass but recall or operational targets do not, report a bounded local proof rather than a fit.
If safe pre-ranking filtering or atomic generation replacement cannot be expressed cleanly, report no fit.
