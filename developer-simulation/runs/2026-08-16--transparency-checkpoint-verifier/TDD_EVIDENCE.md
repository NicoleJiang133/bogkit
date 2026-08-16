# Test-first evidence

The tests were written before production behavior. Expectations are literal or
independently derived; no production helper generated expected Merkle roots or
proofs.

## Initial RED

After the test files were written, `cargo test --offline` first failed because
the public API did not exist. Minimal non-behaving stubs were then added so the
tests could exercise real calls and fail behaviorally rather than stop at name
resolution.

Observed behavioral RED commands and results:

```text
cargo test --offline
  crypto: 4 failed (zero root, consistency invalid, signature invalid,
  wrong size code)

cargo test --offline --test parser
  2 failed, 2 passed (valid envelope rejected; proof-count code wrong)

cargo test --offline --test engine
  4 failed, 1 passed (all submissions returned UNKNOWN_LOG)

cargo test --offline --test store
  3 failed (fixture could not advance, so publication tests could not start)

cargo test --offline --test cli
  3 failed (empty command-line program returned no output)

cargo test --offline --test cli incremental_benchmark
  1 failed (command absent; usage error)
```

The false-positive `cross_log_interleavings` pass against the initial stub was
noticed: both empty byte vectors compared equal. That test was retained only
after the real state engine was implemented, then strengthened to 20 delivery
interleavings across eight logs with a non-empty independently signed workload.

## GREEN and strengthening

After the minimum parser, verifier, state machine, store, and CLI behavior was
implemented, each focused suite passed. Coverage was then strengthened without
changing the implementation to include:

- every bit of a representative three-node proof and new root;
- every bit of an RFC 8032 Ed25519 signature;
- every bit of the signed checkpoint prefix, including size, root, and
  identity;
- 10,000 deterministic malformed envelope cases with a 50 ms per-case bound;
- 20 cross-log delivery interleavings;
- one generic in-process returned error after each of nine completed
  publication stages;
- separate state, proof, manifest, and decision corruption.

Final fresh GREEN:

```text
cargo test --offline
  cli: 4 passed
  crypto: 4 passed
  engine: 9 passed
  parser: 5 passed
  store: 5 passed
  total: 27 passed, 0 failed

cargo clippy --offline --all-targets -- -D warnings -D clippy::all -D clippy::pedantic
  passed with no warnings

cargo fmt --all -- --check
  passed
```

## Reviewer fix round 1: RED to GREEN

Five reviewer-reported gaps were turned into focused behavioral regressions
before their fixes. The exact RED observations were:

```text
cargo test --offline --test engine archive_cursor_rollback_is_rejected_without_mutation -- --exact
  FAILED: left Advanced, right ArchiveCursorRollback

cargo test --offline --test engine same_base_candidates_are_all_decided_after_advance -- --exact
  FAILED: pending["log-a"] was not empty

cargo test --offline --test engine equal_key_pending_delivery_orders_have_identical_bytes -- --exact
  FAILED: stable byte vectors differed

cargo test --offline --test store republish_rejects_corrupt_existing_generation_and_preserves_current -- --exact
  FAILED: left Ok(()), right Err(StoreCorrupt)
```

The resume regression initially failed to compile because `Engine::from_snapshot`
did not exist. A minimal method returning `StoreCorrupt` was added solely to
reach a behavioral RED:

```text
cargo test --offline --test store reopened_engine_continues_and_drains_pending_without_divergence -- --exact
  FAILED: called Result::unwrap() on Err(StoreCorrupt)
```

The smallest fixes added a pre-mutation cursor guard, canonical-envelope pending
identity and total ordering, complete stale-candidate draining, exact existing
generation validation, and checked snapshot reconstruction. Each exact command
then passed, followed by the 27-test all-target suite. Test expectations compare
complete snapshots or independently signed delivery variants; they do not use
production helpers to synthesize the expected outcome.

```text
cargo test --offline --locked --test engine archive_cursor_rollback_is_rejected_without_mutation -- --exact
  1 passed, 0 failed
cargo test --offline --locked --test engine same_base_candidates_are_all_decided_after_advance -- --exact
  1 passed, 0 failed
cargo test --offline --locked --test engine equal_key_pending_delivery_orders_have_identical_bytes -- --exact
  1 passed, 0 failed
cargo test --offline --locked --test store republish_rejects_corrupt_existing_generation_and_preserves_current -- --exact
  1 passed, 0 failed
cargo test --offline --locked --test store reopened_engine_continues_and_drains_pending_without_divergence -- --exact
  1 passed, 0 failed
```

The publication model was also corrected rather than expanded: named
process-exit, disk-full, and permission-denied variants were removed. The tests
now claim only the behavior they execute—one generic returned error after a
selected stage has completed. No child process, partial-write, OS error, or
power-loss behavior is characterized.

## Independent vector derivation

The Merkle literals use `leaf[i] = SHA256(0x00 || u64_be(i))` and
`node = SHA256(0x01 || left || right)`. A separate Python `hashlib` script
computed roots for sizes 0, 1, 2, 3, 4, 7, 8, 15, 16, 65,535, and 65,536,
plus consistency proofs for 1->2, 3->4, 4->7, and 7->8. Tests embed those
literals. The production verifier contains no proof generator.

The signature fixture is RFC 8032 test vector 2, not a signature produced by
the code under test. Random test-only keys are used only to drive the full
state-machine path after the RFC vector independently establishes the signature
verification boundary.
