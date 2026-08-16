# Transparency-checkpoint verifier characterization

This is a small offline Rust verifier, deterministic state machine, and
generation-based publication harness. It is intentionally independent of
BogKit components: the trial found no component that should own this security
boundary.

The package is useful as a runnable characterization and failure reproducer,
not as a drop-in production replacement. It preserves exact input envelope and
proof bytes, uses real SHA-256 and Ed25519 verification, and models atomic
publication, but its `CKP1` envelope is a prototype format because the actual
production wire formats, key set, Go implementation, SQLite schema, compressed
archives, and labeled corpus were not supplied.

## What is implemented

- A pre-allocation parser with a 4 KiB envelope limit, 128-hash proof limit,
  checked length arithmetic, bounded identities, exact raw-envelope retention,
  and exact raw-proof retention.
- SHA-256 binary Merkle frontier construction and RFC 6962-style consistency
  verification, including empty, equal, adjacent, and skipped transitions.
- Ed25519 verification through `ring`, with a fixed log/key registry and
  fail-closed unknown identity, algorithm, and key handling.
- Deterministic per-log advancement, a 16-item pending limit, base anchoring,
  duplicate idempotency, same-size equivocation alerts retaining both roots,
  total pending order keyed by canonical envelope identity, stable error codes,
  archive-cursor rollback rejection before mutation, and sorted JSON bytes.
- A disposable directory store. Each immutable generation contains
  `state.json`, exact `proofs.json`, `decisions.jsonl`, and a digest manifest;
  one atomically renamed `CURRENT` pointer selects the visible generation.
  Existing content-addressed generations are trusted only after every expected
  filename, digest, and payload byte matches the publication candidate.
- Checked snapshot resumption validates registry/log correspondence, signatures,
  identities, state invariants, pending order, and decision/equivocation records
  before reconstructing an engine.
- One generic in-process returned-error hook after each of nine completed
  publication stages: state/proof/decision writes, generation flush/rename,
  pointer write/flush/rename, and directory sync.
- CLI commands for a fixed-seed replay, a synthetic full verifier-path p95
  measurement, deterministic ledger comparison, a store demo, and inspection.

There is no unsafe code.

## Dependencies

Runtime dependencies are deliberately small and disclosed:

- `ring 0.17.14`: SHA-256 and Ed25519.
- `serde 1` and `serde_json 1`: deterministic typed state and JSONL output.
- `tempfile 3` is test-only.

The lab's single nested workspace and lockfile pin the exact resolved versions
while keeping the prototype isolated from BogKit's root workspace.

## Exact verification commands

Run from the repository root. Build and runtime state stays outside the
checkout:

```console
cargo fmt --manifest-path developer-simulation/Cargo.toml \
  --package transparency-checkpoint-verifier -- --check
CARGO_TARGET_DIR=/private/tmp/transparency-checkpoint-target \
  cargo test --offline --locked --manifest-path developer-simulation/Cargo.toml \
  --package transparency-checkpoint-verifier --all-targets
CARGO_TARGET_DIR=/private/tmp/transparency-checkpoint-target \
  cargo clippy --offline --locked --manifest-path developer-simulation/Cargo.toml \
  --package transparency-checkpoint-verifier --all-targets -- \
  -D warnings -D clippy::all -D clippy::pedantic
CARGO_TARGET_DIR=/private/tmp/transparency-checkpoint-target \
  cargo build --release --offline --locked \
  --manifest-path developer-simulation/Cargo.toml \
  --package transparency-checkpoint-verifier
/private/tmp/transparency-checkpoint-target/release/transparency-checkpoint-verifier replay 8000000
/private/tmp/transparency-checkpoint-target/release/transparency-checkpoint-verifier bench-incremental 10000
/private/tmp/transparency-checkpoint-target/release/transparency-checkpoint-verifier demo /private/tmp/transparency-demo
/private/tmp/transparency-checkpoint-target/release/transparency-checkpoint-verifier inspect /private/tmp/transparency-demo
```

The final trial run observed:

```text
27 tests passed
8,000,000 synthetic leaf hashes: 895 ms on the final fresh run
replay root: 6a0bc3488de6971dd01c38a7a6d2631ef9236fed300449e56510fef281ccbe0c
peak replay RSS: 1,818,624 bytes (1,776 KiB)
10,000 synthetic full verifier paths: p95 21 microseconds on the final fresh run
eight empty fixture logs: reopen_ms 0
strict Clippy: passed with no warnings
```

The RSS figure is macOS `/usr/bin/time -l`'s maximum resident set size. These
timings are from the local trial host, not the declared two-core/256 MiB
production-equivalent machine. The replay generates fixed-seed 32-byte leaf
hashes in memory one at a time; it does not decompress or parse the absent
production archives.

## CLI details

Replay a fixed-seed compact frontier:

```console
/private/tmp/transparency-checkpoint-target/release/transparency-checkpoint-verifier replay 8000000
```

Measure parse, registry lookup, Ed25519 signature verification, empty-to-first
consistency verification, and state advancement after 50 warmups:

```console
/private/tmp/transparency-checkpoint-target/release/transparency-checkpoint-verifier bench-incremental 10000
```

Publish, reopen, and validate resumption of a deterministic eight-log empty
snapshot:

```console
/private/tmp/transparency-checkpoint-target/release/transparency-checkpoint-verifier demo /private/tmp/transparency-demo
/private/tmp/transparency-checkpoint-target/release/transparency-checkpoint-verifier inspect /private/tmp/transparency-demo
```

Compare an external baseline ledger byte-for-byte with a candidate ledger:

```console
/private/tmp/transparency-checkpoint-target/release/transparency-checkpoint-verifier \
  compare baseline.jsonl candidate.jsonl
```

`compare` prints `MATCH` and exits 0 for identical bytes; otherwise it prints
`MISMATCH` and exits 2. No Go/SQLite baseline ledger was present in the
checkout, so actual baseline parity was not run.

## Evidence boundaries

- Known-answer Merkle roots were generated independently with Python's
  `hashlib` and stored as literals. The verifier does not generate its own
  consistency-proof expectations.
- Ed25519 uses RFC 8032 vector 2. Tests flip every signature bit; consistency
  tests flip every bit of representative proof nodes and the new root.
- The 10,000 malformed inputs are deterministic synthetic characterization
  cases, not the absent labeled adversarial corpus.
- The publication tests exercise one generic in-process returned error after a
  requested stage has completed. They do not create partial writes, distinguish
  process exit from disk-full or permission errors, kill a process, emulate a
  kernel/filesystem failure, pull power, or prove behavior on other filesystems.
- The store duplicates exact bytes into an auditable proof archive and verifies
  all payload digests on reopen and before reusing an existing generation. It
  is a small prototype, not a migration of the production SQLite transaction.
- Snapshot resumption does not reverify the current checkpoint's historical
  consistency proof because the snapshot has no prior-base history. It checks
  the retained current envelope/signature and cross-record invariants instead.
- Archive record parsing, compressed input streaming, stored-frontier repair,
  archive cursor-to-leaf/index correspondence, all existing signature
  algorithms, all production public keys, and Go parity remain
  unimplemented/unrun.

See the [daily report](../../reports/2026-08-16.md) for the fit decision and
complete audit.
