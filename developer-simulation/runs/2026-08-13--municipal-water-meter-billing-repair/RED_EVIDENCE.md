# Recorded RED evidence

The behavioral test was written before temporal implementation. An initial
online Cargo attempt was discarded as environmental evidence because the
restricted host could not resolve crates.io. The same test was rerun offline.

Command (from `simulation-output/water-repair`):

```console
cargo test --offline --test temporal_red
```

Observed meaningful failure (exit 101):

```text
running 1 test
test six_digit_maximum_to_zero_is_an_exact_rollover_repair ... FAILED

thread 'six_digit_maximum_to_zero_is_an_exact_rollover_repair' panicked at tests/temporal_red.rs:47:5:
assertion `left == right` failed
  left: 0
 right: 1

test result: FAILED. 0 passed; 1 failed
```

The minimal scaffold returned an empty plan. This failure proves the test
detects the missing register-width-aware rollover repair. The test remains in
the permanent suite and must turn GREEN through implementation.

## Fix round 1: abrupt publication lifecycle

Four black-box subprocess tests were written before adding crash modes. They
require Unix signal 6, rather than accepting an ordinary nonzero exit, and
inspect both an existing requested report and an absent requested report at
the two publication boundaries.

Command:

```console
cargo test --offline --test subprocess_publication
```

Meaningful RED result (exit 101):

```text
running 4 tests
... 4 FAILED
assertion `left == right` failed
  left: None
 right: Some(6)
test result: FAILED. 0 passed; 4 failed
```

The pre-fix CLI ignored `--crash`, completed publication normally, and exited
without a signal. The regression therefore detects precisely the skeptical
review finding: returned errors/tests were not evidence of abrupt termination
at process boundaries.

## Fix round 2: global adjustment identity

Three focused tests were written before changing adjustment validation. They
define adjustment IDs as globally unique across the immutable snapshot, allow
only an exact repeated row as an idempotent delivery retry, and require any
different reuse to reject before derivation/publication.

Command:

```console
cargo test --offline --test adjustment_identity
```

Meaningful RED result (exit 101):

```text
running 3 tests
... 3 FAILED
candidate returned a DUPLICATE_ADJUSTMENT_ID review instead of a global error
exact identical retry produced a review instead of the single-row plan
test result: FAILED. 0 passed; 3 failed
```

This reproduces the review finding and proves the new tests distinguish global
pre-derivation identity validation from the old per-service/per-interval
behavior.
