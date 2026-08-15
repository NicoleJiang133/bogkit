# Discovery trail

## 1. Public-material hypothesis (before component internals)

### Baseline first

The existing SQLite full-text baseline is likely strong for exact identifiers and weak in two distinct ways that must be measured separately: vocabulary mismatch hurts recall, and retrieving a lexical top 20 before applying applicability can erase every candidate even when an eligible result exists below rank 20. The smallest useful fixture should first reproduce the second failure exactly, because semantic ranking cannot compensate for an unsafe candidate boundary.

### Expected component fit

- **Fold may fit bounded incremental materialization.** The public examples show persistent streams, transactional writes, consistent read snapshots, keyed replacement, and retraction. Those properties look useful for maintaining revision metadata and search views from canonical records.
- **ESE may fit a small offline semantic experiment.** The public search example computes embeddings locally as a pure function, so it may help the symptom/paraphrase classes without sending licensed text or queries off-device.
- **ANNy may fit unfiltered semantic candidates.** The public search example uses it for local HNSW retrieval and deletes replaced nodes rather than leaving tombstones.

### Likely non-fit or unresolved areas

- The public BM25 and HNSW examples expose only `search(query, limit)` / nearest-neighbor calls; they do not show a metadata predicate, allow-list, or filtered traversal. Post-filtering a fixed candidate list would repeat the baseline's safety/recall defect. A per-configuration index would duplicate a median 780,000 active revisions across 400 snapshots and therefore looks incompatible with the 9 GiB limit unless internals reveal a shared filtered-index design.
- The examples demonstrate transactions inside one database, but not two complete on-disk generations, manifest validation, atomic activation across process exits, or fail-closed corruption behavior. These must not be inferred from the examples.
- The hybrid example fuses candidates in a `HashMap` and sorts only by score; equal-score ordering is not explicitly tied by stable section ID and revision ID. The trial requires an explicit deterministic tie-break.
- Nothing public establishes the 2.4-million-revision build, 16-reader concurrency, 2 GiB memory, 9 GiB disk, or crash-recovery targets. A compact run can only be a bounded local proof or exact failure reproducer, not evidence that those operational gates pass.

### Planned decision test

Inspect only the search and transaction surfaces needed to answer whether eligibility can be applied before bounded retrieval and whether generation replacement is an exposed capability. If no filtered retrieval surface exists, preserve a runnable baseline failure reproducer and, at most, a safe exhaustive toy proof using ESE over already-eligible records. That would be an honest overall no-fit for the stated acceptance criteria, even if the local semantic result is promising.

## 2. Targeted internal inspection

The initial hypothesis was confirmed for the relevant APIs:

- `Bm25Reader::search(query, limit)` accumulates all lexical matches, sorts by score only, and then truncates to `limit`. It accepts no metadata predicate or allow-list. Equal scores have no stable-ID/revision-ID tie-break.
- `HnswReader::search(vector)` returns up to its compile-time `TOP_K`; it accepts no metadata predicate or allow-list.
- Fold's `Filter` is an ingest-time predicate captured in the pipeline. It cannot vary with the query's aircraft snapshot and effective time.
- A Fold write transaction commits all sinks together, and a read transaction pins one consistent database snapshot. This is useful within one store, but it is not the brief's separately validated staging generation plus atomic active-generation switch.
- The HNSW reader shares mutable graph state through `Rc<RefCell<...>>`; the inspected surface does not establish a sixteen-worker, updater-concurrent access model.

Therefore, the public search skeleton cannot safely reuse a bounded global BM25 or HNSW candidate set for this workload. The next step is an exact 21-record reproducer: 20 ineligible records deliberately outrank one eligible record. The baseline will retrieve 20 then filter to zero; an exhaustive all-matches scan can recover the eligible record and return exact evidence, but is retained only as a bounded safety proof, not a scalable solution.

## 3. Test-first RED evidence

The acceptance test was written before the trial library existed. The first offline run failed to compile because the requested crate API did not exist. After adding only the public data shapes and an empty result stub, the meaningful RED run completed the real Fold fixture and reported:

- 1 passing characterization: bounded BM25 top 20 contained no eligible record after filtering.
- 6 failing desired behaviors: the complete-ranking path returned zero instead of document 21, did not reject duplicate revision IDs, did not reject two active revisions, did not reject an unknown attribute, did not reject missing metadata, and did not enforce deterministic tie ordering.

This is the expected failure: tests reached real behavior and failed because the safety selection was not implemented, rather than because of a typo or fixture error.

## 4. ESE packaging check and GREEN

The ESE runtime is local once compiled, but its build script downloads model and tokenizer files from mutable `.../resolve/main/...` URLs when they are absent. The inspected build path does not pin or verify checksums. That does not meet this brief's clean-build requirement for locally bundled, fixed-version, checksummed model files, so ESE was not added to the prototype. This is a packaging/reproducibility issue independent of semantic quality.

After implementing the initially declared validation subset, eligibility/effective-time filtering, exact evidence, and the required tie-break, all seven original library acceptance tests passed. A separate test was then written before the executable existed; it failed because no binary target was available, and passed after the minimal demo was added. The demo uses real Fold BM25 data and removes its temporary database on exit. The later skeptical review found that the original boundary validation was incomplete, as recorded next.

## 5. Skeptical review round 1: rejected, reproduced, fixed

The same skeptical reviewer marked Trial 2 `REJECTED_UNTIL_FIXED` with one Important prototype correctness finding. Two canonical metadata rows could share a `doc_id`; the later row silently replaced the earlier row in the lookup map, so reversing rows changed eligibility and evidence. A ranked list could also repeat one `doc_id` and return the same evidence twice. The reviewer accepted the no-fit product conclusion but rejected the retained prototype as archive evidence until identity validation was repaired.

Three regressions were added before the fix:

- reversed duplicate metadata rows must both return `DuplicateDocumentId` rather than an order-dependent result;
- a repeated ranked key must return `DuplicateRankedDocumentId` rather than duplicate evidence;
- reversing valid unique metadata rows must preserve the full eligible result and exact evidence.

After adding only the two error shapes, the meaningful fix-round RED run executed 10 acceptance tests: 8 passed and the two defect reproducers failed. The duplicate metadata case returned `Ok([])` instead of `DuplicateDocumentId`, while the duplicate ranking case returned the same `EligibleHit` twice instead of `DuplicateRankedDocumentId`.

The implementation now checks all canonical `doc_id` values for uniqueness before constructing the lookup map and checks all ranked `doc_id` values before selection. The GREEN run passed all 10 acceptance tests. This closes the review finding for the compact API while leaving the evidence-backed no-fit product decision unchanged.

## 6. Fix-round verification

Fresh offline verification after the repair passed formatting, 10 acceptance tests, the runnable demo test, strict Clippy with warnings plus `clippy::all` and `clippy::pedantic` denied, the standalone exact Fold BM25 cutoff reproducer, and the runnable demonstration. The demonstration still reports 20 bounded candidates, 0 eligible bounded results, 21 complete matches, 1 eligible exhaustive result, and the same exact evidence. The no-fit decision is unchanged.
