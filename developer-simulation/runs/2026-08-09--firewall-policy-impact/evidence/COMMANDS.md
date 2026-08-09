# Exact verification commands and observed output

The sections below preserve the developer and reviewer trial evidence from the isolated standalone copy. During archival, the child workspace, child lockfile, and package-level release profile were removed. The final normalized archive was then rebuilt using the single nested lab lockfile and default workspace release profile. Its focused formatting, 14 acceptance tests, strict release Clippy, baseline byte determinism, exact verifier replay, malformed-input no-output check, and 50,000-rule run all passed. The archived binary observed 0.13 seconds for analysis and 0.81 seconds for verification; peak RSS was unavailable in the archive sandbox, so only the earlier declared-host runs below carry RSS measurements.

Working directory for every command below:

`/private/tmp/bogkit-sim-2026-08-09-trial2/simulation-output/firewall-policy-impact`

Generated build output was directed to `/private/tmp/bogkit-sim-trial2-target`.

## Host

Command:

```console
$ sw_vers && uname -m && rustc --version && cargo --version
```

Observed:

```text
ProductName: macOS
ProductVersion: 26.5.2
BuildVersion: 25F84
arm64
rustc 1.95.0 (59807616e 2026-04-14)
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
```

## Formatting and strict lint

```console
$ cargo fmt --all --check
```

Exit 0, no output.

```console
$ CARGO_TARGET_DIR=/private/tmp/bogkit-sim-trial2-target cargo clippy --offline --all-targets --all-features -- -D warnings
```

Observed final line:

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.22s
```

Exit 0 with no warnings.

## Tests and differential acceptance workload

```console
$ CARGO_TARGET_DIR=/private/tmp/bogkit-sim-trial2-target cargo test --offline --test acceptance -- --nocapture
```

Observed:

```text
running 6 tests
test malformed_inputs_are_rejected_before_analysis ... ok
test shadowing_reachability_and_semantically_neutral_edits ... ok
test output_is_byte_deterministic_and_sorted ... ok
test families_protocols_and_extreme_ports_remain_separate ... ok
test hand_written_first_match_regressions ... ok
small_pairs=10000 exhaustive_packets=2560000 full_width_pairs=1000 boundary_probes=2000000 disagreements=0
test acceptance_differential_workloads ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.73s
```

The reduced universe contains 16 IPv4 addresses, both protocols, and ports 0 through 7: 256 tuples per pair, or 2,560,000 exhaustive tuple evaluations across 10,000 pairs. Each tuple's report membership, old/new action, and old/new deciding rule ID are compared to the separate linear evaluator. Proposed reachability is also exhaustively checked.

## Release build and demonstration

```console
$ CARGO_TARGET_DIR=/private/tmp/bogkit-sim-trial2-target cargo build --offline --release
```

Observed final line:

```text
Finished `release` profile [optimized] target(s) in 2.40s
```

Sample baseline:

```console
$ /private/tmp/bogkit-sim-trial2-target/release/firewall-policy-impact sample-replay fixtures/baseline-old.json fixtures/baseline-proposed.json fixtures/baseline-samples.json
```

Observed: 2 samples, 0 changed samples. The exact JSON is preserved in `evidence/baseline-sample-replay.json`.

Exact analysis:

```console
$ /private/tmp/bogkit-sim-trial2-target/release/firewall-policy-impact analyze fixtures/baseline-old.json fixtures/baseline-proposed.json /private/tmp/bogkit-trial2-baseline-report.json
outcome=newly-allowed change_regions=1 reachable_rules=2 unreachable_rules=0 report=/private/tmp/bogkit-trial2-baseline-report.json
```

Independent replay:

```console
$ /private/tmp/bogkit-sim-trial2-target/release/firewall-policy-impact verify fixtures/baseline-old.json fixtures/baseline-proposed.json /private/tmp/bogkit-trial2-baseline-report.json
verified exact_report=true change_witnesses=1 reachability_witnesses=2
```

A second identical run was compared with `cmp`; it exited 0 with no output, establishing byte-for-byte identical JSON for the fixture. The generated report matches `fixtures/baseline-expected-report.json`.

## Invalid input produces no verdict

```console
$ /private/tmp/bogkit-sim-trial2-target/release/firewall-policy-impact analyze fixtures/baseline-old.json fixtures/invalid-proposed.json /private/tmp/bogkit-trial2-invalid-report.json
```

Observed exit 2:

```text
error: input validation failed; no semantic verdict was produced:
proposed policy: rule[0] id="duplicate": reversed destination-port range 9000..8000
proposed policy: rule[0] id="duplicate": CIDR "10.0.0.1/24" is not canonical
proposed policy: rule[1] id="duplicate": duplicate rule id
```

`test ! -e /private/tmp/bogkit-trial2-invalid-report.json` exited 0 after the failure. Separate tests cover unknown actions, unsupported protocols, family mismatch, and malformed schemas.

## Recorded 50,000-rule benchmark

Generation:

```console
$ /private/tmp/bogkit-sim-trial2-target/release/firewall-policy-impact generate-benchmark /private/tmp/bogkit-trial2-benchmark-strong-0809 50000 104373592904137
generated old_rules=50000 proposed_rules=50000 seed=104373592904137 directory=/private/tmp/bogkit-trial2-benchmark-strong-0809
```

Timed exact release run:

```console
$ /usr/bin/time -l /private/tmp/bogkit-sim-trial2-target/release/firewall-policy-impact analyze /private/tmp/bogkit-trial2-benchmark-strong-0809/old.json /private/tmp/bogkit-trial2-benchmark-strong-0809/proposed.json /private/tmp/bogkit-trial2-benchmark-strong-0809/report.json
```

Observed:

```text
outcome=newly-allowed change_regions=1 reachable_rules=25000 unreachable_rules=25000 report=/private/tmp/bogkit-trial2-benchmark-strong-0809/report.json
        0.12 real         0.11 user         0.00 sys
            41664512  maximum resident set size
```

This is 0.8% of the 15-second limit and 15.5% of the 256 MiB limit. It is an observed fixture/host result.

A final fresh run from a separately built target observed 0.13 seconds and 41,648,128 bytes maximum resident set. Across both measurements, the conservative observed maxima were 0.13 seconds and 41,664,512 bytes.

Witness replay:

```console
$ /private/tmp/bogkit-sim-trial2-target/release/firewall-policy-impact verify /private/tmp/bogkit-trial2-benchmark-strong-0809/old.json /private/tmp/bogkit-trial2-benchmark-strong-0809/proposed.json /private/tmp/bogkit-trial2-benchmark-strong-0809/report.json
verified exact_report=true change_witnesses=1 reachability_witnesses=25000
```

## Reviewer fix round 1: reserved implicit marker

The analyzer now rejects a real rule ID equal to `default-deny`, leaving that value unambiguous as the implicit fallback marker.

Focused regression:

```console
$ CARGO_TARGET_DIR=.reviewer-target cargo test --offline --test acceptance implicit_default_marker_cannot_be_used_as_a_real_rule_id -- --nocapture
test implicit_default_marker_cannot_be_used_as_a_real_rule_id ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out
```

Release CLI adversary:

```console
$ .reviewer-target/release/firewall-policy-impact analyze fixtures/baseline-old.json fixtures/reserved-id-proposed.json .reviewer-reserved-report.json
error: input validation failed; no semantic verdict was produced:
proposed policy: rule[0] id="default-deny": id is reserved for the implicit default-deny decision
```

The command exited 2. `test ! -e .reviewer-reserved-report.json` exited 0 both before and after it, proving that no semantic report was created.

Fresh complete quality pass:

```text
cargo fmt --all --check: exit 0
strict clippy with -D warnings: exit 0, no warnings
offline release build: exit 0
acceptance tests: 7 passed, 0 failed
small_pairs=10000 exhaustive_packets=2560000 full_width_pairs=1000 boundary_probes=2000000 disagreements=0
```

Fresh release demo: sample replay still found 0/2 changed samples; exact analysis still emitted 1 newly allowed region; independent replay verified 1 change witness and 2 reachability witnesses. Two reports matched each other and `fixtures/baseline-expected-report.json` byte for byte (`cmp` exit 0).

Fresh representative performance run, with all round-one generated files inside `simulation-output/firewall-policy-impact`:

```text
outcome=newly-allowed change_regions=1 reachable_rules=25000 unreachable_rules=25000
        0.12 real         0.12 user         0.00 sys
            34553856  maximum resident set size
verified exact_report=true change_witnesses=1 reachability_witnesses=25000
```

The round-one build target, generated benchmark, and temporary reports were removed after verification.

## Reviewer fix round 2: stale report, strict verification, and alias safety

Focused stale-output regression:

```console
$ CARGO_TARGET_DIR=.reviewer2-target cargo test --offline --test acceptance reused_output_is_removed_on_validation_read_and_write_failures -- --nocapture
test reused_output_is_removed_on_validation_read_and_write_failures ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out
```

The test creates a valid report and then reuses its path with an invalid proposed policy; it also repeats with a missing input and exercises a write failure. It asserts the requested report path is absent after each returned error.

Release CLI reuse demonstration:

```console
$ .reviewer2-target/release/firewall-policy-impact analyze fixtures/baseline-old.json fixtures/baseline-proposed.json .reviewer2-report.json
outcome=newly-allowed change_regions=1 reachable_rules=2 unreachable_rules=0 report=.reviewer2-report.json
$ .reviewer2-target/release/firewall-policy-impact analyze fixtures/baseline-old.json fixtures/invalid-proposed.json .reviewer2-report.json
error: input validation failed; no semantic verdict was produced:
proposed policy: rule[0] id="duplicate": reversed destination-port range 9000..8000
proposed policy: rule[0] id="duplicate": CIDR "10.0.0.1/24" is not canonical
proposed policy: rule[1] id="duplicate": duplicate rule id
$ test ! -e .reviewer2-report.json
```

The invalid command exited 2; the final absence check exited 0. Recreating the valid report and rerunning with missing `fixtures/missing-proposed.json` also exited 2, and the same absence check passed. A valid analysis targeting missing parent `.reviewer2-write-failure/report.json` exited 2 with `cannot write temporary report`; its absence check passed.

Input alias preservation:

```console
$ CARGO_TARGET_DIR=.reviewer2-target cargo test --offline --test acceptance output_aliases_are_rejected_before_either_input_is_changed -- --nocapture
test output_aliases_are_rejected_before_either_input_is_changed ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out
```

This regression checks report=old, report=proposed, a relative/canonical old alias, a symlink alias, and a Unix hard-link alias. Every call is rejected before unlink/write and both input byte sequences are checked after every attempt.

Closed report-schema regression:

```console
$ CARGO_TARGET_DIR=.reviewer2-target cargo test --offline --test acceptance report_schema_rejects_unknown_fields_at_every_nested_level -- --nocapture
test report_schema_rejects_unknown_fields_at_every_nested_level ... ok
```

The test injects an unknown field separately at six JSON locations: root, summary, change, change witness, reachability entry, and reachability witness. All six are rejected. CLI evidence:

```console
$ .reviewer2-target/release/firewall-policy-impact verify fixtures/baseline-old.json fixtures/baseline-proposed.json fixtures/baseline-report-unknown-field.json
error: invalid report JSON or schema: unknown field `unexpected`, expected one of `schema_version`, `analysis`, `summary`, `changes`, `proposed_rule_reachability` at line 51 column 14
```

Exit 2; no `exact_report=true` line was printed.

Fresh complete round-two gates:

```text
cargo fmt --all --check: exit 0
strict clippy with -D warnings: exit 0, no warnings
offline release build: exit 0
acceptance tests: 10 passed, 0 failed
small_pairs=10000 exhaustive_packets=2560000 full_width_pairs=1000 boundary_probes=2000000 disagreements=0
```

Release demo and determinism remained unchanged: sample replay found 0/2 changes, exact analysis found one newly allowed region, `verify` replayed 1 change witness and 2 reachability witnesses, and two reports matched each other and the checked-in expected report byte for byte.

Round-two 50,000/50,000 release measurement:

```text
outcome=newly-allowed change_regions=1 reachable_rules=25000 unreachable_rules=25000
        0.12 real         0.11 user         0.00 sys
            34963456  maximum resident set size
verified exact_report=true change_witnesses=1 reachability_witnesses=25000
```

All round-two generated artifacts were removed after these results were recorded.

Final cleanup audit from the sanitized checkout root:

```console
$ find simulation-output -type d \( -name target -o -name '*target*' -o -name '.reviewer*' -o -name '.reused-output-test-*' -o -name '.output-alias-test-*' \) -print
$ find simulation-output -type f \( -name '.reviewer*' -o -name '*.tmp-*' \) -print
$ find simulation-output -type l -print
```

All three commands produced no output. `git status --short` showed only `?? simulation-output/`; no root/core/example/archive file was modified.

## Reviewer fix round 3: exclusive temporary creation

Focused adversarial publication run:

```console
$ CARGO_TARGET_DIR=.reviewer3-target cargo test --offline --test acceptance temporary_ -- --nocapture
running 4 tests
test preexisting_temporary_symlink_never_truncates_an_input ... ok
test temporary_collisions_are_retried_without_deleting_foreign_candidates ... ok
test preexisting_temporary_hard_link_never_truncates_an_input ... ok
test normal_atomic_publication_leaves_no_temporary_file ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out
```

The hard-link and symlink regressions precreate the former first candidate `.report.json.tmp-<current-pid>` pointing at `old.json`. Publication retries instead of opening it. Afterward, old and proposed bytes and the collision contents are unchanged. The multi-collision test preserves two foreign candidates, publishes through the next exclusively created candidate, and confirms that owned candidate disappears through rename. Normal publication is repeated twice with byte-identical reports and no temporary residue.

The first complete rerun after changing the publisher produced 13 passing tests and one failed assertion in `reused_output_is_removed_on_validation_read_and_write_failures`: the failure expected the old phrase `cannot write temporary report`, while exclusive creation correctly returned the new phrase `cannot create temporary report`. The implementation behavior and output-absence assertion were unchanged. The expectation was updated and the entire suite rerun fresh:

```text
running 14 tests
test implicit_default_marker_cannot_be_used_as_a_real_rule_id ... ok
test malformed_inputs_are_rejected_before_analysis ... ok
test shadowing_reachability_and_semantically_neutral_edits ... ok
test output_is_byte_deterministic_and_sorted ... ok
test families_protocols_and_extreme_ports_remain_separate ... ok
test report_schema_rejects_unknown_fields_at_every_nested_level ... ok
test hand_written_first_match_regressions ... ok
test preexisting_temporary_symlink_never_truncates_an_input ... ok
test output_aliases_are_rejected_before_either_input_is_changed ... ok
test temporary_collisions_are_retried_without_deleting_foreign_candidates ... ok
test preexisting_temporary_hard_link_never_truncates_an_input ... ok
test normal_atomic_publication_leaves_no_temporary_file ... ok
test reused_output_is_removed_on_validation_read_and_write_failures ... ok
small_pairs=10000 exhaustive_packets=2560000 full_width_pairs=1000 boundary_probes=2000000 disagreements=0
test acceptance_differential_workloads ... ok
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.76s
```

Other fresh gates:

```text
cargo fmt --all --check: exit 0
strict clippy with -D warnings: exit 0, no warnings
offline release build: exit 0
```

Release demonstration remained stable: sample replay found 0/2 changes, two exact reports matched each other and `fixtures/baseline-expected-report.json` byte for byte, and verification replayed 1 change witness plus 2 reachability witnesses. Reusing one of those reports with the invalid proposed fixture exited 2 and the report-absence check passed. The reserved-ID invalid fixture also exited 2 with no report. The unknown-field report still exited 2 without `exact_report=true`.

Round-three 50,000/50,000 release measurement:

```text
outcome=newly-allowed change_regions=1 reachable_rules=25000 unreachable_rules=25000
        0.13 real         0.11 user         0.00 sys
            34979840  maximum resident set size
verified exact_report=true change_witnesses=1 reachability_witnesses=25000
```

All round-three generated paths were inside `simulation-output/firewall-policy-impact` and were removed after measurement.

The intermediate assertion failure left `.reused-output-test-57807-0`; the cleanup scan detected it and it was explicitly removed with the round-three build, benchmark, and report artifacts. Final directory scan:

```text
simulation-output
simulation-output/firewall-policy-impact
simulation-output/firewall-policy-impact/evidence
simulation-output/firewall-policy-impact/fixtures
simulation-output/firewall-policy-impact/src
simulation-output/firewall-policy-impact/tests
```

`find simulation-output -name '.*' -print` and `find simulation-output -type l -print` both produced no output. `git status --short` again showed only `?? simulation-output/`.
