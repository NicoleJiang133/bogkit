# Water-meter billing repair prototype

This offline prototype reads a synthetic JSON export, validates each service
point independently, verifies the result against a separately structured
chronological oracle, and atomically publishes one canonical repair plan. It
uses exact integers throughout and never writes the input or any billing data.

Reading source IDs and prior-adjustment IDs are globally unique across one
immutable snapshot. Repeating the exact same row is an idempotent delivery
retry. Reusing an ID with any different field—including moving one adjustment
ID to another billed interval—rejects the whole snapshot before derivation or
publication with a stable `CONFLICTING_*_ID:<id>` error.

BogKit decision: no component was retained. Fold was evaluated for keyed
transactional replay and retraction, but the application would still own every
temporal rule, canonical sort, provenance edge, failure-isolation rule, and
report-file publication step. Its durable embedded state would be a second
authority without measured benefit for this offline repair. ESE and ANNy are
text/vector similarity components and do not fit exact structured billing.

## Exact reproduction

Run from the BogKit repository root. `--locked --offline` uses the dependency
versions fixed in the nested lab workspace lockfile.

```console
cargo fmt --manifest-path developer-simulation/Cargo.toml -p water-repair -- --check
cargo test --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p water-repair --all-targets
cargo test --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p water-repair --test subprocess_publication
cargo clippy --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p water-repair --all-targets --all-features -- -D warnings -D clippy::all -D clippy::pedantic
mkdir -p /tmp/bogkit-water-repair-demo
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p water-repair -- verify --input developer-simulation/runs/2026-08-13--municipal-water-meter-billing-repair/fixtures/disclosed.json --output /tmp/bogkit-water-repair-demo/unused.json
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p water-repair -- demo --input developer-simulation/runs/2026-08-13--municipal-water-meter-billing-repair/fixtures/disclosed.json --output /tmp/bogkit-water-repair-demo/plan.json --batch-size 7
cmp /tmp/bogkit-water-repair-demo/plan.json developer-simulation/runs/2026-08-13--municipal-water-meter-billing-repair/fixtures/disclosed-expected.json
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p water-repair -- scale --output /tmp/bogkit-water-repair-demo/scale-plan.json --batch-size 4096
```

`verify` checks the disclosed fixture against the independent oracle, then 100
deterministic generated seeds, then ten input permutations at batch sizes 1, 7,
and 64. `demo` verifies the input against the oracle before publishing. `scale`
generates the requested counts in memory without resident or meter data and
reports generation, validation/traversal, publication, and process peak RSS.
Its generator is deliberately best-case: records are already ordered, each
service point has only two billed intervals, there are no corrections or prior
adjustments, and the result has zero repairs/reviews. Its timing is not evidence
of representative repair performance.

To process another synthetic export:

```console
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p water-repair -- process --input developer-simulation/runs/2026-08-13--municipal-water-meter-billing-repair/fixtures/disclosed.json --output /tmp/bogkit-water-repair-demo/plan.json --batch-size 1024
```

Safe-publication fault points are `--inject before-temporary` and `--inject
before-final`. On validation, computation, or injected publication failure, the
existing output remains byte-for-byte unchanged. An input/output path identity
that can be resolved is rejected.

The separate test-only `--crash before-staging` and `--crash before-final`
modes terminate the process with `SIGABRT`. The second point occurs after a
complete same-directory temporary file is synced and immediately before atomic
rename. `tests/subprocess_publication.rs` launches real child processes and
proves an existing requested report is unchanged—or an absent report stays
absent—at both boundaries. A subsequent run removes the orphan stage and
publishes a complete report. These modes deliberately kill the calling process
and must not be used in ordinary operation. Publication assumes one writer per
requested output path.

The hand-written `fixtures/disclosed-expected.json` is the expected output for
`fixtures/disclosed.json`. The fixture includes a six-digit rollover, a late
correction, meter replacement, exact duplicate, ambiguous regression, prior
adjustment, unchanged interval, and deliberately scrambled input order. Other
adversarial cases live as literal permanent tests in `tests/acceptance.rs`.

## Deliberate boundary

This is a correctness and feasibility prototype, not regulatory or production
qualification. It does not connect to the vendor database, change bills, infer
missing consumption, calculate money, issue invoices, implement customer UI,
or replace the existing audit/approval flow. The rollover classifier is an
explicit prototype policy: a decrease is a rollover only when it crosses more
than half the register and goes from the top quarter to the bottom quarter;
other regressions fail closed.

Generated Cargo output belongs in an external `CARGO_TARGET_DIR` during corpus
verification, and the `/tmp/bogkit-water-repair-demo` runtime directory may be
deleted after inspection. Neither is part of the archive.
