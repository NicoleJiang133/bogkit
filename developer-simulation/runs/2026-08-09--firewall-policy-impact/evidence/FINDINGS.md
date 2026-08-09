# Categorized findings

Severity describes impact on this scenario, not a general assessment of BogKit.

## Reviewer fix round 3: preexisting temporary paths cannot overwrite inputs

- Category: correctness
- Scope: atomic report publication
- Severity: publication blocker
- Confidence: high on the declared host
- Reproduction: create the former first temporary candidate `.report.json.tmp-<pid>` as either a hard link or symlink to `old.json`, then analyze. The fixed publisher treats the path as an `AlreadyExists` collision, retries a fresh suffix, and leaves both policies and the collision path byte-identical.
- Smallest improvement: implemented with `OpenOptions::create_new(true)`, bounded distinct retries, full write plus file sync, atomic rename, and cleanup limited to a candidate exclusively created by the current run.

## Reviewer fix round 2: stale reused verdict removed

- Category: correctness
- Scope: CLI report publication
- Severity: important
- Confidence: high
- Reproduction: first analyze the valid baseline into path X, then run `analyze fixtures/baseline-old.json fixtures/invalid-proposed.json X`. The fixed command exits 2 and `test ! -e X` succeeds. The regression also covers a missing input and a report write failure.
- Smallest improvement: implemented by clearing an existing report before policy reads and clearing the requested path again on every returned read, validation, analysis/publication failure.

## Reviewer fix round 2: output aliases cannot destroy inputs

- Category: correctness
- Scope: stale-output cleanup safety
- Severity: important
- Confidence: high on the declared Unix host
- Reproduction: request the old path, proposed path, a relative/canonical form of the old path, a symlink to it, or a hard link to it as the report. All five calls are rejected before unlink/write, and both input files remain byte-identical.
- Smallest improvement: implemented with direct and canonical comparisons plus Unix device/inode identity before any output cleanup.

## Reviewer fix round 2: report verification now has a closed schema

- Category: correctness
- Scope: `verify`
- Severity: minor
- Confidence: high
- Reproduction: `verify` the included `baseline-report-unknown-field.json`; it exits 2 with an unknown-field schema error and never prints `exact_report=true`. Regression mutations cover the report root, summary, change, both witness locations, and reachability entry.
- Smallest improvement: implemented with unknown-field rejection on every report object.

## Reviewer fix: implicit marker collision removed

- Category: correctness
- Scope: prototype report identity
- Severity: important
- Confidence: high
- Reproduction: submit a policy containing a real rule whose ID is exactly `default-deny`. The fixed validator rejects it before analysis with `id is reserved for the implicit default-deny decision`; the adversarial regression `implicit_default_marker_cannot_be_used_as_a_real_rule_id` locks this behavior.
- Smallest improvement: implemented by reserving the marker at validation, so every `default-deny` report ID now unambiguously denotes the implicit fallback.

## 1. Sample replay can miss a real allow expansion

- Category: correctness
- Scope: stated preflight baseline
- Severity: high
- Confidence: high
- Reproduction: run `sample-replay` on the included baseline fixtures, then run `analyze`. The samples report zero changed decisions; exact analysis finds `10.0.1.0`-`10.0.1.255`, TCP ports 443-444, newly allowed.
- Smallest improvement: keep sample replay only as supplementary bug-finding evidence and require an exact modeled-space analysis before emitting `no semantic change`.

## 2. No discovered BogKit component supplies exact ordered policy geometry

- Category: missing capability
- Scope: public README and examples examined in the recorded order
- Severity: high
- Confidence: high for the inspected public surface; not a claim about undiscovered private APIs
- Reproduction: follow `evidence/DISCOVERY.md`. Fold examples materialize incremental streams, while ESE and ANNy serve text/vector search. None demonstrates exact first-match rectangle subtraction, exhaustive equivalence, or shadow reachability.
- Smallest improvement: add a public selection guide that explicitly identifies unsupported exact static-analysis workloads; only add a new primitive if multiple real workloads justify it.

## 3. ANNy is incompatible with proof-bearing results

- Category: poor fit
- Scope: ANNy
- Severity: critical if used for the verdict
- Confidence: high
- Reproduction: its public README identifies approximate nearest-neighbor search; the scenario requires exact no-change results and exact unreachable classifications.
- Smallest improvement: document that approximate indexes may generate probes or candidate regions but must never establish policy equivalence.

## 4. ESE does not model firewall predicates

- Category: poor fit
- Scope: ESE
- Severity: high if selected
- Confidence: high
- Reproduction: its public material exposes text-to-vector encoding. The modeled fields are canonical CIDRs, a two-value protocol enum, and inclusive integer port intervals.
- Smallest improvement: none in ESE; omit it from projects that do not process text semantics.

## 5. Fold adds persistence without the needed exact operation

- Category: poor fit
- Scope: Fold for this batch preflight
- Severity: medium
- Confidence: high for the public examples
- Reproduction: starter, timeseries, and chat use a durable stream to incrementally update counts, bags, keyed aggregates, tables, or snapshots. This trial receives two complete files and emits one report.
- Smallest improvement: show a decision table in the root documentation for when a standalone batch algorithm is preferable to a persistent Fold pipeline.

## 6. Closure-heavy pipeline types create helper-function friction

- Category: API friction
- Scope: Fold examples and Rust-beginner persona
- Severity: low to medium
- Confidence: high
- Reproduction: the public search example states that its pipeline type contains closures and therefore uses macros instead of ordinary helper functions.
- Smallest improvement: document a boxed/type-erased reader pattern or provide a small named wrapper example if the API supports one.

## 7. Starter dependency selection is unclear

- Category: documentation gap
- Scope: project bootstrap/starter
- Severity: low
- Confidence: high
- Reproduction: `examples/starter/Cargo.toml` declares Fold, ESE, and ANNy while `main.rs` uses Fold only. The root README says generated projects add all three even when not all are used.
- Smallest improvement: have the scaffold ask which components are needed or explain which dependency lines are safe to remove.

## 8. Exact analyzer passed the recorded resource fixture

- Category: performance
- Scope: this prototype and generated fixture only
- Severity: none observed
- Confidence: high for the declared host/fixture, low as a universal bound
- Reproduction: use seed `104373592904137` and 50,000 rules. Five release runs through reviewer round 3 observed 0.12-0.13 seconds and no more than 41,664,512 bytes peak RSS, with 25,000 reachable and 25,000 shadowed proposed rules.
- Smallest improvement: retain this fixture and add high-fragmentation/output-heavy fixtures before considering production use.

## 9. Exact output is inherently output-sensitive

- Category: performance
- Scope: prototype limitation
- Severity: medium
- Confidence: high
- Reproduction: construct many interleaved address and port boundaries with alternating first-match actions; the number of exact report rectangles can grow with the semantic partition.
- Smallest improvement: define an allowed maximum report size and add representative adversarial fragmentation benchmarks. Do not silently sample or truncate an exact verdict.
