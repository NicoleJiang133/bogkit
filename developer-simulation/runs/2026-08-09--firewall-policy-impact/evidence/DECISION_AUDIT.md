# Consequential decision audit and uncertainty

## Decisions

### Reject Fold, ESE, and ANNy for the prototype

- Alternatives considered: Fold persistent views; ESE text embeddings; ANNy approximate nearest-neighbor search; standalone exact algorithm.
- Decision: standalone exact Rust algorithm.
- Basis: the acceptance contract requires exact first-match equivalence and exact shadow reachability from two complete static policies. Approximation is forbidden, text embeddings are unrelated, and persistence does not supply the missing ordered geometric operation.
- Consequence: the result evaluates BogKit honestly as a no-fit rather than manufacturing component usage. It uses no BogKit dependency.
- Reversibility: high. A future exact interval component could replace the sweep implementation while preserving the JSON contract and linear verifier.

### Use a deterministic address sweep and port segment tree

- Alternatives considered: sampled replay; full packet enumeration; naive Cartesian coordinate grid; repeated rectangle subtraction; boundary sweep.
- Decision: exact boundary sweep with earliest-active-rule tracking over compressed port intervals.
- Basis: packet enumeration is impossible over IPv6, a Cartesian grid can multiply all address and port boundaries, and sampling cannot prove no change. The sweep evaluates only exact rule boundaries and preserves first-match rule identity.
- Consequence: exact results for the modeled fields, with cost dependent on boundaries, active rules, and emitted regions.

### Require a separate linear replay oracle

- Decision: keep `src/reference.rs` as an intentionally simple policy-order scan, used only for tests, probes, sample replay, and witness verification.
- Basis: a witness should be independently replayable without relying on the sweep's winner-selection logic.
- Consequence: verification is slower per packet but straightforward and isolated from the analyzer algorithm.

### Fully validate both policies before analysis

- Decision: parse and validate old and proposed inputs before calling the analyzer or writing a report.
- Basis: malformed input must not result in a partial verdict. The literal ID `default-deny` is also reserved so a real rule cannot collide with the implicit-decision marker used in reports and witness replay.
- Consequence: one run can report multiple semantic validation errors. Schema-deserialization failures stop validation of that individual document at Serde's first structural error, but the other policy is still read and validated; no report is emitted.

### Invalidate reused output before any verdict work

- Decision: the shared file-analysis entry point removes any prior regular-file/symlink report before reading policies, and removes the requested path again on every returned failure.
- Basis: a stale successful report is an observable semantic verdict even when the current invalid run exits nonzero.
- Consequence: valid-report-to-invalid-validation and valid-report-to-missing-input reruns leave no report path. A failed write also leaves no requested report.
- Safety constraint: direct, canonical/relative, symlink, and Unix hard-link aliases to either input are rejected before unlinking. A non-file output target is refused rather than recursively deleted.

### Treat report verification as a closed schema

- Decision: reject unknown fields at the report root and inside every summary, change, witness, and reachability object.
- Basis: `exact_report=true` should mean the verifier understood the entire submitted report, not only its known subset.
- Consequence: additive schema changes require an explicit schema-version/code update instead of being silently ignored.

### Exclusively create the publication temporary file

- Decision: open each temporary candidate with atomic create-new semantics, never with a truncating path write. Treat any existing file, directory, hard link, or symlink as a collision and retry a distinct suffix up to a fixed bound.
- Basis: a predictable preexisting temporary path could otherwise refer to an input policy and path-based writing would truncate that input before rename.
- Consequence: foreign collision paths are never opened, followed, truncated, or deleted. The publisher writes and syncs only the file it exclusively created, atomically renames it, and removes only that owned path on a returned write/rename failure.
- Remaining boundary: an attacker able to precreate all 256 candidates can cause a clean denial of publication, but cannot turn a candidate into an input overwrite through this path.

### Record deterministic generators, not large generated deliverables

- Decision: preserve generator code, exact counts, and seed `104373592904137`; put generated policies and build output under `/private/tmp`.
- Basis: the trial requires reproducibility and forbids generated build output in the deliverable.
- Consequence: the 50,000-rule fixture is reproducible locally but not committed as a large JSON pair.

## Uncertainty and evidence limits

- The analyzer is exact only for source address, IP family, TCP/UDP, and inclusive destination-port interval with ordered first match and implicit default deny. It says nothing about the explicit non-goal fields.
- The linear evaluator is independently implemented but shares validated rule and packet types. Exhaustive reduced-universe comparison and boundary probes reduce, but cannot mathematically eliminate, implementation risk.
- The 10,000-pair exhaustive suite proves agreement only for every generated reduced-universe case, not every possible policy.
- The 1,000-pair/2,000,000-probe suite is additional bug-finding evidence, not the basis for an exact no-change verdict.
- The five observed 0.12-0.13-second runs and 41,664,512-byte maximum are specific to macOS 26.5.2 on arm64, release builds, the recorded seed, and the generated 50,000/50,000 pair. No universal resource bound is claimed.
- Exact normalized output is output-sensitive. A hostile high-fragmentation policy can create a very large number of distinct regions even when rule count is fixed.
- The performance generator deliberately mixes 25,000 reachable disjoint predicates with 25,000 immediately shadowed duplicate predicates and changes one action. It stresses full input validation, reachability bookkeeping, IPv4/IPv6 boundaries, and report witness construction, but it is not a worst-case fragmentation workload.
- Public discovery was bounded to the sanitized checkout's README, examples, and component public readmes/manifest. The no-fit conclusion is scoped to that inspected surface.
