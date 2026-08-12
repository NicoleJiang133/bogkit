# Blind discovery: hard-real-time MIDI scheduler

## What I think the repository offers

From the root README and public examples, BogKit is a workspace for database-style incremental materialization and search. Fold owns persistent streams, transactions, snapshots, and derived views. ESE turns text into static embeddings. ANNy supplies an HNSW nearest-neighbour index. The examples allocate owned strings and vectors, use filesystem-backed database paths, and in one case place a plain ingest thread in front of Fold while asynchronous clients receive copied snapshots.

That looks useful for an editor-side event catalogue or offline search tool. It does not yet look like a callback scheduler. The callback in this trial may touch only one immutable plan and fixed-size state, while Fold's advertised value is durable incremental state and ESE/ANNy's value is text/vector search.

## Baselines I am bringing from C++ audio work

The production comparison is the existing C++ scheduler, not a blank slate. My normal C++ callback design would be:

- validate and flatten the tempo map and event plan on the control thread;
- publish a pointer plus generation at a block boundary;
- keep the callback cursor, duplicate-token guard, active-note table, and sticky diagnostics in fixed-capacity storage;
- write directly into a caller-owned fixed-capacity output span;
- retire old plans outside the callback, after an explicit reader handoff proves they are no longer referenced;
- make overload ordering and transport-reset terminations part of the public contract.

The Rust experiment only earns its keep if its public black-box behaviour is clearer and at least as callback-safe. Merely moving the same uncertainty across a language boundary is not a win.

## Independent rational-arithmetic baseline

I will write a slow oracle separately from the scheduler. It will integrate each piecewise-constant tempo segment as an exact rational number:

`delta_pulses * microseconds_per_quarter * sample_rate / (960 * 1_000_000)`

The oracle will keep the numerator in a wider signed integer and floor the accumulated rational time to obtain the containing frame. It will select events with exact half-open comparisons against `[block_start, block_end)`, then apply the specified priority and stable-ID ordering with an ordinary allocating sort. It must not call scheduler conversion or selection helpers. Literal boundary fixtures will check that my interpretation is not simply shared code agreeing with itself.

The existing C++ scheduler remains an external baseline described by the brief; this isolated trial has no C++ source or executable to compare, so I will not claim C++ parity or a speedup.

## Initial component-use hypothesis

- **Fold:** probably no fit in the audio callback because the public examples expose storage paths, transactions, mutable stream ownership, and materialized collections. I will check whether a narrowly separated, immutable read view can be prepared off-thread without callback allocation, locking, syscalls, or reclamation ambiguity. If not, I will not force it into the model.
- **ESE:** no sane fit. MIDI beat-to-frame scheduling has no text embedding step, and any embedding work would be unrelated to the correctness problem.
- **ANNy:** no sane fit. Events are already sorted on a one-dimensional musical timeline; approximate nearest-neighbour search would weaken exact boundary semantics and add an irrelevant index.

My expected smallest useful result is therefore a standalone public-function model in the assigned trial output directory, plus a minimal public-surface Fold reproducer if its callback fit is ambiguous. A measured no-fit for all three BogKit components is acceptable; the scheduler model is still needed to turn the callback contract into executable evidence.

## Questions I need source inspection to answer

1. Does Fold have an immutable reader that can be held and queried without a transaction, allocation, locking, filesystem access, or panic paths?
2. Does its read/publication model expose a bounded, audio-thread-safe reclamation proof?
3. Do ESE or ANNy document a property that surprisingly applies to exact timeline lookup? I expect not, so I will inspect only enough to avoid an assumption-based rejection.
4. Can the standalone API keep every callback result caller-owned, fixed-capacity, and error-returning while retaining exact rational preparation off-thread?
