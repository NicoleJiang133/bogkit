# Return refund reconciler trial

This is an advisory-only command-line prototype over synthetic JSON snapshots.
It performs no network calls and has no carrier, warehouse, payment, or database
client. PostgreSQL exports remain authoritative; this program publishes only a
canonical proposed plan.

## Reproduce

Run from the BogKit repository root. The commands use the nested lab workspace
and keep generated runtime files under `/tmp`.

```console
cargo fmt --manifest-path developer-simulation/Cargo.toml -p return-reconciler-trial1 -- --check
cargo test --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p return-reconciler-trial1 --all-targets --all-features
cargo clippy --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p return-reconciler-trial1 --all-targets --all-features -- -D warnings -D clippy::all -D clippy::pedantic

mkdir -p /tmp/bogkit-return-reconciler-demo
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p return-reconciler-trial1 -- reconcile developer-simulation/runs/2026-08-13--multi-carrier-return-refund-reconciler/fixtures/disclosed.json /tmp/bogkit-return-reconciler-demo/disclosed-report.json
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p return-reconciler-trial1 -- verify developer-simulation/runs/2026-08-13--multi-carrier-return-refund-reconciler/fixtures/disclosed.json /tmp/bogkit-return-reconciler-demo/disclosed-report.json
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p return-reconciler-trial1 -- benchmark
```

`benchmark` constructs the disclosed shape in memory: 25,000 returns, 40,000
distinct parcels, 70,000 authorized lines, 250,000 events, and 500 labeled
adversarial returns. It requires exact candidate/reference equality and ten
byte-identical deterministic shuffles. It prints host-local timings; it does
not claim production equivalence.

To inspect or retain the generated snapshot explicitly:

```console
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p return-reconciler-trial1 -- generate-representative /tmp/bogkit-return-reconciler-demo/representative.json
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p return-reconciler-trial1 -- reconcile /tmp/bogkit-return-reconciler-demo/representative.json /tmp/bogkit-return-reconciler-demo/representative-report.json
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p return-reconciler-trial1 -- verify /tmp/bogkit-return-reconciler-demo/representative.json /tmp/bogkit-return-reconciler-demo/representative-report.json
```

The disclosed fixture contains only invented identifiers and integer values.
It includes a split-parcel two-unit return with a remainder cent and a reused
event ID with conflicting payloads. Publication fault and abrupt-exit cases are
covered by `tests/publication.rs`; the fault-injection command is intentionally
not part of the ordinary usage line.

## Output contract

Reports are one-line canonical JSON followed by a newline. Arrays and review
codes are deterministically ordered. Each source event ID is accounted as
accepted, accepted with exact retries, or quarantined conflict. Proposed lines
record accepted/review quantities, cent caps, and provenance IDs. The report is
written and synced to a sibling temporary file, independently verified, and
atomically renamed over the requested path. A relative output path is supported.

`proposed_quantity` always counts fully funded deterministic units. Unit cents
are allocated in stable line/unit order, including remainder cents. If a line or
return cap cannot fund the next whole unit, that unit contributes zero proposed
quantity and zero proposed cents, remains visible in `accepted_scan_quantity`,
and adds `insufficient_whole_unit_capacity` for operator review. Partial-unit
cent proposals are never emitted.

All line and return cents are `u64`, and report-wide cent totals use checked
addition. If otherwise valid return plans cannot fit the canonical report-wide
total, reconciliation fails before report publication; wrapped totals are never
serialized. Every raw payment result must name a line owned by its stated
return. Missing or wrong-return line identities are rejected before derivation,
including events that would later be duplicated or quarantined.

The program never executes a refund. Operators must still review every
`disposition: "review"` row, and a separate approved service would remain
responsible for execution.
