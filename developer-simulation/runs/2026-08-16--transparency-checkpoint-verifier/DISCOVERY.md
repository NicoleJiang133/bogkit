# Discovery record

## Order and baseline-first hypothesis

I read the supplied brief first, then the required TDD guidance, and then only
the repository's public root README plus the four public examples and their
manifests. I had not inspected component implementation source when recording
this document.

The trusted Go verifier and SQLite transaction remain the baseline. My initial
hypothesis is that BogKit might contribute a narrow derived-state accelerator,
but it should not own cryptographic verification or durable publication unless
the public contract can preserve exact signed checkpoint/proof bytes and commit
checkpoint, proof, archive cursor, and decision atomically. Any prototype here
must therefore separate independently testable proof/identity/admission logic
from a storage experiment, and must default to retaining the baseline.

## What the public material led me to expect

- `fold` is described as a statically typed incremental programming framework
  that materializes changing streams into durable views. The examples show one
  write transaction updating a count, bag, keyed tables, and aggregates, with
  later reads observing one consistent snapshot. Reopening the same path is
  said to resume state.
- The starter and chat examples make `fold` look potentially relevant to
  atomically maintained derived views and quick restart. They do not publicly
  demonstrate exact raw-byte retention, stable serialization, durability at
  flush/rename/directory-sync boundaries, disk-full recovery, corruption
  detection, rollback, or proof-plus-cursor publication.
- The timeseries example suggests keyed incremental aggregation is useful when
  many inserts update queryable views. A checkpoint verifier instead has a
  small amount of security-critical latest-state plus an append-only audit
  record; this may be a poor match unless the atomic storage guarantees are
  explicit and inspectable.
- `ESE` is described as static text embedding. The search example uses it to
  turn document text into vectors. The brief explicitly excludes certificate
  search and requires exact cryptographic bytes, so I expect no fit.
- `ANNy` is described as an HNSW approximate-nearest-neighbor index. The search
  example uses it only for semantic retrieval. Approximation and vector search
  have no place in exact Merkle/signature decisions, so I expect no fit.

## Components considered

### Fold

Potentially relevant only as a narrow durable materialization/store candidate.
Concrete questions before retaining it:

1. Can one transaction durably publish the verified checkpoint, exact proof
   bytes, exact archive cursor, and stable decision record together?
2. Are the on-disk representation and recovery rules documented enough to
   audit old-or-new behavior at every requested publication exit?
3. Can fixed-key latest state and append-only decisions be reopened and
   inspected without rebuilding or reproducing the security decision?
4. Can duplicates and same-size equivocation be represented without losing
   either checkpoint hash, and are output bytes deterministic?
5. Is there a public/focused API for failure injection, disk-full handling,
   corruption diagnosis, and bounded resource use?

If these cannot be answered positively with small, observable tests, I will
exclude Fold from the implementation and record missing capability or poor
product fit rather than building security semantics on assumptions.

### ESE

Initial decision: no fit. Its published purpose is embedding text for semantic
search, while the verifier must compare identities, sizes, signatures, roots,
and proof nodes exactly. No implementation inspection is needed unless public
claims elsewhere contradict this boundary.

### ANNy

Initial decision: no fit. Approximate vector neighbors cannot establish an
exact append-only proof and would add state, memory, and nonessential failure
modes. No implementation inspection is needed unless public claims elsewhere
contradict this boundary.

## Initial fit/no-fit questions for the prototype

- Can a small independent parser reject oversized envelopes and proof counts
  before allocation, including 129 and maximum-unsigned declarations?
- Can Merkle consistency be tested against hand-derived known answers while
  preserving every supplied proof byte? A reduced or characterization hash is
  insufficient evidence for production compatibility and must be labeled.
- Can candidate ordering be deterministic across logs, require the current
  verified base, wait on missing bases, and make duplicates idempotent?
- Can invalid identity, algorithm/key, signature, size, root, or proof leave
  visible state unchanged with a stable bounded error record?
- Can same-size different-root inputs retain both hashes as one durable,
  idempotent equivocation alert?
- Can a publication layer demonstrate old-or-new complete state at modeled
  exit and storage-error boundaries without claiming OS/filesystem guarantees
  it did not test?
- Can a representative release replay stay inside the time and memory envelope
  without buffering the corpus? Full corpus parity is impossible unless the
  supplied external fixtures and Go baseline are actually present.

## Planned evidence boundary

I will use strict red/green cycles with hand-derived expectations. The smallest
meaningful deliverable should be an offline Rust library plus CLI harness that
characterizes bounded parsing, exact-byte retention, independently checked
Merkle consistency logic, deterministic decision/state transitions, duplicate
and equivocation behavior, and a deliberately explicit publication model. It
will not claim production signature compatibility, Go parity, crash durability,
or full-corpus performance without the corresponding real algorithms, inputs,
fault hooks, and measurements.

## Procedure note for reviewer adjudication

My initial public-document search used the case-sensitive pattern `README*`.
It found the root `README.md` but missed lowercase component files including
`fold/readme.md`, `ese/readme.md`, and `anny/readme.md`. This discovery record
was therefore written after the root README and all public examples, but before
those lowercase component READMEs were read. A later `rg` source-symbol search
printed small implementation snippets and revealed the missed paths. I stopped,
read every lowercase component README, and only then opened implementation
files with `sed`. The baseline-first hypotheses above did not change, but this
did not fully satisfy the requested documentation order and is explicitly
retained for reviewer adjudication.
