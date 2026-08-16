# Preserved RED/GREEN evidence

Every prototype behavior was introduced by a behavioral test before its implementation. Expected values are literal or hand-derived; tests do not call the production reducer to build their expected snapshots.

## Initial causal replay

Command:

```console
cargo test --offline --test core causally_buffered_scalar_history_has_literal_canonical_snapshot -- --exact
```

RED first failed with `left: Applied { drained: 0 }`, `right: Applied { drained: 1 }`. GREEN passed after causal readiness, buffering, drain, and canonical-state materialization were implemented.

## Identity safety

Command:

```console
cargo test --offline --test core duplicate_is_a_no_op_and_conflicting_id_reuse_is_rejected_atomically -- --exact
```

RED failed to compile because `ApplyError`, `Duplicate`, `Rejected`, `snapshot_digest`, and `accepted_count` did not exist. GREEN passed after exact identity bytes were checked before any mutation.

## Schedule-independent LWW

Command:

```console
cargo test --offline --test core concurrent_scalar_winner_is_independent_of_delivery_order -- --exact
```

RED produced `fill:red` for the reverse schedule instead of the literal `fill:blue`. GREEN passed after causal dominance and the declared concurrent writer tuple were separated from arrival order.

## Tombstones and deleted anchors

Command:

```console
cargo test --offline --test core tombstones_block_resurrection_and_deleted_list_neighbors_still_order_children -- --exact
```

The first RED lacked four operation variants. After the smallest six-kind implementation, a second test-first case added delayed object recreation; RED visibly recreated an ellipse. GREEN passed after object tombstones blocked recreation. The existing test stayed green while recursive list traversal was later refactored into an iterative ordered traversal.

## Bounded adversarial input

Commands:

```console
cargo test --offline --test validation raw_input_is_size_bounded_before_utf8_or_json_parsing -- --exact
cargo test --offline --test validation truncated_json_and_unknown_operation_kind_have_stable_errors -- --exact
cargo test --offline --test validation payload_and_sequence_limits_reject_valid_json -- --exact
cargo test --offline --test core pending_work_is_bounded_and_dependency_cycles_are_reported -- --exact
```

Each RED first lacked the named validation/error API. GREEN covers raw-size-before-UTF-8 priority, invalid UTF-8, truncated JSON, unknown kind, payload cap, zero/overflow sequence, bounded pending work, and cycle diagnosis.

## Offline window and compaction

Commands:

```console
cargo test --offline --test compaction reconnect_window_has_deterministic_full_thirty_day_boundary -- --exact
cargo test --offline --locked --test compaction compaction_counts_permanent_identity_metadata_in_retained_byte_gate -- --exact --nocapture
cargo test --offline --test compaction compacted_state_reopens_with_tombstones_and_accepts_in_window_pending_edit -- --exact
cargo test --offline --test compaction compaction_refuses_a_causally_incomplete_state -- --exact
```

The byte-reduction test had a meaningful RED after implementation: equivalent identity payloads were duplicated in compacted state and pushed the artifact above 40%. The initial GREEN removed that duplication by rebuilding identity state from retained operations. Reviewer fix round 1/5 proved that repair was incorrect across the full lifecycle because IDs pruned from catch-up history could then be reused. The corrected permanent fingerprints are intentionally counted by the renamed retained-byte test, and the acceptance gate now fails. The remaining tests first lacked their APIs, then passed at the exact 30-day boundary, after reopen, and on pending-state refusal.

## Generation publication

Commands:

```console
cargo test --offline --test publication in_process_returned_error_at_each_stage_never_exposes_partial_generation -- --exact
cargo test --offline --test publication corrupt_current_manifest_falls_back_to_prior_valid_generation -- --exact
```

The first RED lacked the publication API. GREEN passed an ordinary in-process returned-error model after generation creation, data flush, manifest flush, rename, and directory sync, plus idempotent retry. It did not exercise process termination or operating-system I/O faults. Fallback behavior was deliberately removed before its test; RED returned `InvalidGeneration` for a corrupt current manifest, and GREEN selected generation 1.

## Reviewer fix round 1/5

The independent review rejected the initial handoff. These are the preserved RED/GREEN cycles for its two prototype correctness findings.

### T1-C1 — permanent operation identity

Command:

```console
cargo test --offline --locked --test compaction conflicting_operation_id_reuse_remains_rejected_after_compact_reopen -- --exact
```

RED reached the published `open_latest` path and failed its literal outcome assertion: `left: Applied { drained: 0 }`, `right: Rejected(ConflictingOperationId)`. The regression uses the next valid actor sequence and different payload, then checks the exact rejection code plus unchanged canonical bytes, snapshot digest, and accepted counter through both `CompactionArtifact::reopen` and compact → publish → `open_latest`.

GREEN passed after compact state began storing SHA-256 fingerprints of the canonical bytes for every accepted operation ID and reopen restored that complete map. The map is no longer reconstructed only from retained catch-up operations.

The required post-fix byte-gate command was:

```console
cargo test --offline --locked --test compaction compaction_counts_permanent_identity_metadata_in_retained_byte_gate -- --exact --nocapture
```

It prints `compacted=3719 uncompacted=8670 percent=42.90`. The test passes because it locks the observed acceptance-gate violation; the brief's at-most-40% criterion itself is **FAIL**, not PASS.

### T1-I1 — complete manifest integrity

Command:

```console
cargo test --offline --locked --test publication changed_manifest_metadata_falls_back_or_fails -- --exact
```

RED changed only `reconnect_decisions` from `catch_up` to `snapshot_required`; `open_latest` still accepted generation 2, failing `left: 2`, `right: 1`. GREEN passed after publication stored `manifest.sha256` separately and reopen verified it against every byte of `manifest.json` before trusting any semantic field. A one-file semantic change now falls back to generation 1 (or would hard-fail if no valid generation remained). This is an unkeyed integrity check, not protection from an adversary able to rewrite both files.

### T1-I2 — evidence-only correction

`FaultKind::Crash` and `FaultKind::DiskFull` were removed because both labels followed the same ordinary returned-error path. The remaining test and API say exactly what happens: an error is returned after one of five completed stages. No child exit, actual disk-full/permission error, partial write, pointer sub-stage interruption, process/host crash, or power loss is tested. `crash_schedules.json` is parsed and shape-validated only.

## Harness, determinism, and digest

Commands:

```console
cargo test --offline --test cli replay_command_reads_ndjson_compacts_publishes_and_reopens -- --exact
cargo test --offline --test cli largest_command_builds_publishes_and_times_a_requested_shape -- --exact
cargo test --offline --test determinism generated_histories_match_hand_derived_oracle_across_schedules -- --exact
cargo test --offline --test determinism twenty_input_batchings_produce_identical_snapshot_and_manifest_bytes -- --exact
cargo test --offline --test determinism durable_digest_uses_sha256_known_vector -- --exact
```

The CLI tests first failed because the binary/commands did not exist. The batching test first failed because `apply_batch` did not exist. The digest RED returned the old 64-bit checksum `e71fa2190541574b` instead of the literal SHA-256 vector for `abc`; GREEN returned `ba7816bf...15ad` after adding `sha2`.

## Characterization, not a defect test

```console
cargo test --offline --test fold_comparison fold_keyed_materialization_is_arrival_ordered_without_a_causal_wrapper -- --exact
```

This narrow upstream characterization passed: forward and reverse upsert schedules leave different values. It documents why Fold needs a custom causal winner layer; it does not label Fold's generic upsert semantics a defect.

## Final fresh gate

```console
cargo fmt --manifest-path Cargo.toml --package causal-canvas-compaction -- --check
cargo test --offline --locked --all-targets
cargo clippy --offline --locked --all-targets -- -D warnings -D clippy::all -D clippy::pedantic
```

Fresh fix-round result before artifact cleanup: formatting clean; 22 tests passed; strict Clippy passed with zero warnings. The first post-fix Clippy run rejected only the expanded byte-gate test's length and lossy floating-point display. Fixture construction was extracted and the percentage was changed to integer hundredths; the retained `42.90` result and behavioral assertion were unchanged, and the strict rerun passed without warning suppressions.
