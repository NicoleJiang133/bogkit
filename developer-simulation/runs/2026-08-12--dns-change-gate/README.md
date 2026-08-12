# DNS change gate trial prototype

This is an offline, advisory comparison tool for a declared subset of BIND master-file syntax. It parses selected zones from an old and proposed snapshot, compares canonical records and TTLs, applies explicit safety rules, and atomically publishes deterministic JSON. It neither queries DNS nor deploys files. Supplied `named-checkzone` results remain authoritative and any supplied failure blocks.

The input contract is narrower than the production brief: both snapshot directory trees and the policy file must be immutable and non-racing for the complete run. The standard-library implementation checks canonical paths and then opens them by pathname; it has no descriptor-relative, non-following open primitive and does not prove containment under concurrent filesystem mutation. If immutability cannot be guaranteed, do not run or trust this prototype; a caller or future integration must fail closed by declining the operation. Concurrent mutation is unsupported, its behavior is unverified, and the original race-safe “never read outside a root” requirement is not met.

This prototype intentionally uses no BogKit component. Fold's durable incremental database is a poor fit for a bounded one-shot parse/compare operation and adds dependencies and state without addressing path containment or DNS semantics. ESE embeddings and ANNy approximate search are incompatible with exact record identity and fail-closed policy decisions.

## Run the demonstration

From the BogKit repository root:

```sh
mkdir -p /tmp/dns-change-gate-demo
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p dns-change-gate --bin dns-change-gate -- \
  --old-root developer-simulation/runs/2026-08-12--dns-change-gate/fixtures/demo/old \
  --new-root developer-simulation/runs/2026-08-12--dns-change-gate/fixtures/demo/new \
  --policy developer-simulation/runs/2026-08-12--dns-change-gate/fixtures/demo/policy.conf \
  --output /tmp/dns-change-gate-demo/report.json
```

An allow or review report exits 0. A semantic or supplied-baseline block is still a successfully published report and exits 2. Input, containment, limit, policy, alias, and pre-rename publication errors exit 1 without replacing the output path. If the final parent-directory sync fails after a successful rename, the command exits 1 but the output path may already contain the complete new report; it never contains a partial document.

The complete archive verification commands are:

```sh
cargo fmt --manifest-path developer-simulation/Cargo.toml -p dns-change-gate -- --check
cargo test --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p dns-change-gate --all-targets
cargo clippy --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p dns-change-gate --all-targets -- -D warnings -D clippy::all -D clippy::pedantic
mkdir -p /tmp/dns-change-gate-demo
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p dns-change-gate --bin dns-change-gate -- \
  --old-root developer-simulation/runs/2026-08-12--dns-change-gate/fixtures/demo/old \
  --new-root developer-simulation/runs/2026-08-12--dns-change-gate/fixtures/demo/new \
  --policy developer-simulation/runs/2026-08-12--dns-change-gate/fixtures/demo/policy.conf \
  --output /tmp/dns-change-gate-demo/report.json
cmp /tmp/dns-change-gate-demo/report.json \
  developer-simulation/runs/2026-08-12--dns-change-gate/fixtures/demo/oracle-report.json
```

It checks formatting, runs all black-box tests, denies compiler warnings plus `clippy::all` and `clippy::pedantic`, builds release binaries, runs the demo, and compares its bytes with a separate checked-in oracle.

## Generate a corpus

The dependency-free fixture generator refuses to overwrite an existing root:

```sh
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p dns-change-gate --bin fixture-generator -- \
  --root /tmp/dns-change-gate-corpus \
  --zones 1000 \
  --records-per-zone 200 \
  --changed-zones 50
```

It creates `old/`, `new/`, and `policy.conf`. The generated policy lists every zone and supplies `PASS` as the old and new syntax-checker result. The first requested zones change their SOA serial and every generated A record; the rest are byte-identical. Adversarial behavior lives in the black-box test fixtures rather than this performance corpus.

## Policy format

The policy is UTF-8 line-oriented `key=value` data. Required numerical fields are `max_records_total`, `max_records_per_zone`, `max_include_depth`, `max_generate_records`, and `ttl_decrease_percent`. Total and per-zone record limits apply independently to each snapshot. Every selected zone is:

```text
zone=absolute-zone.|relative-master-file|old-named-checkzone-code|new-named-checkzone-code
```

Only the literal baseline code `PASS` is successful. This deliberately simple representation makes the baseline handoff explicit; it does not invoke `named-checkzone`.

## Declared parser and policy boundary

Accepted syntax covers `$ORIGIN`, `$TTL`, relative and absolute owners, omitted owners, three-digit decimal and ordinary one-byte escapes, comments, parentheses, decimal TTL units, `$INCLUDE` with optional origin, one simple `$` substitution in an inclusive `$GENERATE` numeric range, and SOA/NS/A/AAAA/CNAME/MX/TXT/SRV/CAA/DS records. TXT chunk boundaries remain distinct. Raw bytes above ASCII are rejected; escaped bytes are rendered canonically.

For immutable inputs, includes are relative to the including file. Absolute paths, lexical escapes, pre-existing resolved symlink escapes, missing files, cycles, and excessive nesting block. These checks do not close a path-replacement race between canonicalization and open. Expansion limits block before publication and never truncate. `$GENERATE` cardinality is calculated with checked wide arithmetic and compared with its cap before iteration, including maximum-`u32` endpoints.

The semantic policy checks:

- exactly one apex SOA and at least one apex NS;
- CNAME coexistence;
- TTL decreases beyond the policy percentage;
- RFC 1982 serial advance, equality, regression, wraparound, and half-range ambiguity;
- delegation additions/removals;
- loss of the last A/AAAA glue family for an in-bailiwick delegated server;
- retained second-family glue and out-of-bailiwick servers that require no parent glue;
- supplied baseline failures.

Delegation changes are review findings. Missing last glue, apex failure, alias conflict, disallowed TTL decrease, serial equality/regression, and baseline failure block. Half-range serial arithmetic is review. No repair is attempted.

## Deterministic output rules

Names and DNS types are ASCII-lowercased/uppercased as appropriate; IPv4 and IPv6 addresses, TTLs, integers, and name-bearing RDATA are canonicalized. Exact duplicate canonical records with the same TTL collapse. Conflicting TTLs on an otherwise duplicate tuple block as malformed.

Zones sort by absolute zone name. Changes sort by owner, type, canonical RDATA, then change kind. Findings sort by owner, stable code, cited record-type list, then detail. Record-type lists are sorted sets. JSON keys have fixed writer order, strings use a fixed escape routine, and there are no timestamps or host-dependent values. The output ends with one newline.

The output parent must already exist outside both snapshot roots. An output path that is identical to, resolves to, or on Unix has the same file identity as the policy or any parsed master/include file is rejected before any report write.

The report is written to a collision-resistant temporary sibling opened exclusively with `create_new`. Existing entries are never followed, opened, truncated, or removed; bounded collisions select a new name. Normal write/flush/rename failures remove only the temporary entry created by that run. The file is flushed and synced, renamed only after all selected zones parse and check, and the parent directory is synced. `DNS_GATE_FAULT=after-temp`, `DNS_GATE_FAULT=after-flush`, and `DNS_GATE_TEMP_NONCE` are test-only injection controls; forced process termination may leave an unreferenced owned temporary entry, which the test case's disposable directory removes.

## Serious prototype limits

- The parser is deliberately not a complete BIND parser and has not been compared against `named-checkzone`; unsupported syntax blocks.
- `$GENERATE` modifiers and multiple substitutions are unsupported.
- The generator's large valid corpus is synthetic and its adversarial cases are exercised separately, not mixed into the scale run.
- The prototype's black-box oracles cover representative boundary classes, not the requested exhaustive 1,000-change, 60-delegation, and 40-SOA fixture tables.
- Resource measurements in the trial report were taken on macOS with 14 hardware cores and 64 GiB available, not the declared four-core Linux host. The measured process stayed far below 256 MiB, but this is not proof of the Linux target.
- Path containment is proven only for immutable, non-racing snapshots. Concurrent path replacement is unsupported and unverified because the implementation uses canonicalize-then-open-by-name.
- Crash tests cover the two declared application fault points. They do not prove durability under kernel, filesystem, or power loss.
- Hard-link identity rejection is implemented and tested on Unix. Other platforms retain canonical-path/symlink alias rejection but do not claim hard-link identity detection.
