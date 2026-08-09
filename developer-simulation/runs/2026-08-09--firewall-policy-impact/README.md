# Exact ordered firewall policy impact prototype

This standalone Rust command compares an old and proposed ordered firewall policy. It validates both complete inputs, applies authoritative first-match semantics with implicit default deny, and emits a deterministic JSON report containing:

- exact newly allowed and newly denied address/port regions;
- the deciding old and proposed rule IDs, including `default-deny`;
- one deterministic packet witness per changed region;
- reachable or fully unreachable classification for every proposed rule;
- one witness for every reachable proposed rule.

The implementation is deliberately read-only. It never rewrites either policy, connects to a firewall, uses packet logs, or makes a deployment decision.

## Why this is standalone

Public BogKit material was read in this order: root README, then the `starter`, `timeseries`, `chat`, and `search` examples. Fold is a persistent incremental materialized-view engine, while ESE creates static text embeddings and ANNy provides approximate nearest-neighbor search. The requested job is a one-shot, exact comparison of ordered geometric predicates; approximation is explicitly forbidden, and persistence or semantic search does not simplify the required proof. All three components were therefore considered and rejected as poor fits. See `evidence/DECISION_AUDIT.md` for the detailed audit.

## Input format

Each input is a JSON object with one ordered `rules` array:

```json
{
  "rules": [
    {
      "id": "allow-web",
      "action": "allow",
      "ip_family": "ipv4",
      "source_cidr": "10.0.0.0/24",
      "protocol": "tcp",
      "port_start": 443,
      "port_end": 443
    }
  ]
}
```

Only `allow`/`deny`, `ipv4`/`ipv6`, TCP/UDP, canonical CIDRs, and ports 0 through 65,535 are accepted. The rule ID `default-deny` is reserved exclusively for the implicit-default marker and is rejected as a real rule ID. Unknown fields and enum values, malformed or noncanonical CIDRs, family mismatches, reversed port ranges, and duplicate or empty IDs also fail before analysis.

At the start of every `analyze` request, any prior regular file or symbolic link at the requested report path is removed before either policy is read. Every returned read, validation, or write failure clears the path again. This prevents a report from an earlier successful run from surviving an invalid rerun as a stale semantic verdict. Before any unlink, direct, canonical/relative, symbolic-link, and (on Unix) hard-link aliases to either input are rejected so the analyzer cannot delete or overwrite a policy. Non-file output targets are refused rather than recursively removed.

Report publication uses an exclusively created fresh temporary file and atomic rename. Existing temporary-name candidates—including hard links or symlinks to an input—are never opened or truncated; they are treated as collisions and publication retries a suffixed name. A temporary file owned by the current run is cleaned up if writing, flushing, or renaming fails.

## Build and run

All commands below are offline. The external target directory keeps generated build output out of the deliverable.

```console
$ export CARGO_TARGET_DIR=/private/tmp/bogkit-firewall-impact-target
$ cargo build --offline --locked --release -p firewall-policy-impact
$ /private/tmp/bogkit-firewall-impact-target/release/firewall-policy-impact \
    analyze fixtures/baseline-old.json fixtures/baseline-proposed.json /private/tmp/report.json
outcome=newly-allowed change_regions=1 reachable_rules=2 unreachable_rules=0 report=/private/tmp/report.json

$ /private/tmp/bogkit-firewall-impact-target/release/firewall-policy-impact \
    verify fixtures/baseline-old.json fixtures/baseline-proposed.json /private/tmp/report.json
verified exact_report=true change_witnesses=1 reachability_witnesses=2
```

`verify` reruns the exact analysis and then evaluates every change and reachability witness with the separate linear first-match evaluator in `src/reference.rs`.

Report verification uses a closed schema: unknown fields at the report root or inside the summary, change region, witness, or reachability objects are rejected before `exact_report=true` can be printed.

The expected report for the included example is `fixtures/baseline-expected-report.json`.

## Baseline comparison

The included samples deliberately miss a newly allowed region:

```console
$ /private/tmp/bogkit-firewall-impact-target/release/firewall-policy-impact \
    sample-replay fixtures/baseline-old.json fixtures/baseline-proposed.json fixtures/baseline-samples.json
```

Sample replay reports zero changed samples, while exact analysis reports `10.0.1.0` through `10.0.1.255`, TCP destination ports 443 through 444, newly allowed by `new-admin-range`. This is why the sample command explicitly warns that it cannot establish no semantic change.

## Acceptance and quality checks

```console
$ cargo fmt --check -p firewall-policy-impact
$ cargo clippy --offline --locked --release -p firewall-policy-impact --all-targets --all-features -- -D warnings
$ cargo test --offline --locked --release -p firewall-policy-impact --all-targets -- --nocapture
$ /private/tmp/bogkit-firewall-impact-target/release/firewall-policy-impact \
    validate-suite 10000 1000 2000000 104373592904137
```

The acceptance test performs exhaustive enumeration on 10,000 generated reduced-universe pairs (2,560,000 packet tuples), then 2,000,000 boundary-biased probes across 1,000 full-width IPv4/IPv6 pairs. It checks report membership and decisions against the separate linear evaluator, verifies exact proposed-rule reachability in the reduced universe, and replays every emitted witness.

The hand-written suite covers insertion before an allow, deletion of a deny, reordering, partial CIDR and port overlap, redundant same-action coverage, opposite-action shadowing, implicit default-deny transitions, family/protocol separation, ports 0 and 65,535, and a semantically neutral edit.

## Reproduce the 50,000-rule measurement

The generator creates two 50,000-rule policies from a recorded seed. The proposed policy changes one action field. Half the proposed rules are reachable and half are deliberately shadowed by the immediately preceding duplicate predicate.

```console
$ /private/tmp/bogkit-firewall-impact-target/release/firewall-policy-impact \
    generate-benchmark /private/tmp/bogkit-firewall-impact-benchmark 50000 104373592904137
$ /usr/bin/time -l /private/tmp/bogkit-firewall-impact-target/release/firewall-policy-impact \
    analyze /private/tmp/bogkit-firewall-impact-benchmark/old.json \
    /private/tmp/bogkit-firewall-impact-benchmark/proposed.json \
    /private/tmp/bogkit-firewall-impact-benchmark/report.json
$ /private/tmp/bogkit-firewall-impact-target/release/firewall-policy-impact \
    verify /private/tmp/bogkit-firewall-impact-benchmark/old.json \
    /private/tmp/bogkit-firewall-impact-benchmark/proposed.json \
    /private/tmp/bogkit-firewall-impact-benchmark/report.json
```

On the declared test host, five release measurements through reviewer round 3 took 0.12 to 0.13 seconds; the largest observed maximum resident set was 41,664,512 bytes. These are observed results for this recorded fixture and host, not a universal upper-bound proof for every possible 50,000-rule policy. See `evidence/COMMANDS.md` for exact output and `evidence/DEPENDENCIES_AND_CLEANUP.md` for host and cleanup details.

During archival, the standalone child workspace, child lockfile, and package-level `strip`/`lto` release settings were removed. The package was rebuilt with the single nested lab lockfile and default workspace release profile. The final archived binary re-ran the same 50,000/50,000 fixture in 0.13 seconds and verified its 25,001 witnesses in 0.81 seconds. The archive sandbox did not expose peak RSS, so the 41,664,512-byte figure above remains explicitly limited to the pre-archive declared-host runs and is not attributed to the normalized binary.

## Algorithm boundary

For each IP-family/protocol combination, the analyzer sweeps exact CIDR boundary intervals. A port segment tree maintains the earliest active rule over exact port boundary intervals. It compares the old and proposed winners, coalesces adjacent equivalent regions deterministically, and records the lower address/port corner as the witness. The linear evaluator is a separate, intentionally simple scan of rules in policy order.

The output can necessarily grow with the number of distinct semantic regions. The measured 50,000-rule fixture is comfortably within the target, but adversarial policies whose exact result itself is enormous may require more time or memory; this prototype makes no broader bound than the observed evidence.
