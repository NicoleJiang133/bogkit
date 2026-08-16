# Trial 2 brief: Transparency-checkpoint verifier

## Developer role and Rust experience

- You maintain a certificate-transparency monitoring service for a hosting provider.
- You have eight years of production Go and security operations experience.
- You finished the Rust Book and have four months of part-time Rust experience.
- You can review unsafe code but do not want it in a new verification boundary.

## Existing system and baseline implementation

- The monitor follows 24 independently operated public append-only logs.
- Each log publishes signed checkpoints containing log identity, tree size, root hash, and time.
- A consistency proof must show that a newer root extends the last verified tree.
- The service currently verifies signatures and Merkle proofs in Go.
- SQLite stores the last verified checkpoint, input cursor, raw proof, and alert records.
- One transaction advances a log's checkpoint and its durable archive cursor.
- Leaf hashes are streamed from compressed local archives to rebuild a compact tree frontier.
- The baseline is trusted but restart replay and diagnostics are slow and difficult to test.
- Signed checkpoints and archived leaf hashes remain authoritative inputs.

## Concrete pain

- Restarting after an unclean exit can spend 45 minutes rebuilding tree frontiers.
- A past cursor bug advanced beyond a checkpoint whose proof had not been durably recorded.
- Proof failures are logged inconsistently, complicating security review and incident comparison.
- Malformed proof fixtures have triggered excessive allocation before validation.
- The team wants a small verifier and checkpoint store with exhaustive recovery tests.
- It must not weaken the baseline's fail-closed behavior to gain speed.

## Workload and data shape

- Production receives up to 8,000,000 new leaf hashes per day across the 24 logs.
- The prototype corpus contains 8 logs, 8,000,000 total leaf hashes, and 4,000 checkpoints.
- Leaf hashes and internal nodes are exactly 32 bytes.
- A checkpoint envelope is at most 4 KiB before signature validation.
- A normal consistency proof contains 1 to 64 hashes.
- The hard admission limit is 128 proof hashes and 16 pending checkpoints per log.
- Checkpoint tree sizes range from 1 to more than 2^34 leaves.
- The corpus includes empty-to-first, adjacent, skipped-size, and equal-size transitions.
- Exactly 10,000 labeled adversarial proof cases accompany the valid corpus.
- Archive records are length-prefixed and include log identity, leaf index, and leaf hash.

## Operational constraints

- Verification follows the declared binary Merkle-tree hashing rules exactly.
- Existing log public keys and signature algorithms must remain supported.
- A new checkpoint is visible only with its proof, archive cursor, and decision record.
- A process exit may cause replay but must never cause a leaf or proof to be skipped.
- Checkpoints may arrive out of order independently across logs.
- Each proof names its base checkpoint, which must be the current verified checkpoint.
- A candidate anchored to a pending base must wait; no unverified base can be skipped.
- Two different roots at the same tree size produce a durable equivocation alert.
- Unknown logs, algorithms, or keys fail closed and cannot create state.
- The prototype runs offline on two CPU cores with 256 MiB of memory.
- No diagnostic may include certificate contents or private operational tokens.
- Verification output must not depend on worker scheduling or hash-map iteration order.

## Measurable acceptance criteria

- Match the Go baseline on every valid checkpoint and proof in the prototype corpus.
- Match known-answer vectors for sizes 0, 1, 2, 3, 4, 7, 8, 15, 16, 65,535, and 65,536.
- Reject all 10,000 labeled adversarial cases with zero false acceptance.
- Never advance visible state on an invalid signature, proof, root, size, or log identity.
- Detect every planted same-size different-root equivocation and retain both checkpoint hashes.
- Process the 8,000,000-leaf replay in at most 90 seconds on the declared machine.
- Peak measured resident memory must remain below 224 MiB.
- Verify an ordinary incremental checkpoint at p95 below 5 milliseconds after warm-up.
- Reopen all eight logs after a clean stop in under 500 milliseconds.
- Twenty cross-log delivery interleavings must yield byte-identical per-log decisions.
- Duplicated checkpoints and proofs must not create duplicate alerts or cursor advances.
- For every injected publication exit, recovery exposes the old or new complete checkpoint.
- Retrying after any modeled disk-full result must converge to the same durable bytes.
- Error records must use stable codes and bounded metadata, never raw certificate data.
- No valid input may panic, and each malformed input must finish within 50 milliseconds.

## Explicit non-goals

- Do not poll public logs or make any network request.
- Do not parse certificates, validate certificate chains, or build a certificate search index.
- Do not validate submission receipts or browser certificate policy.
- Do not implement gossip between independent monitors.
- Do not prove that an operator omitted an unknown leaf from all observers.
- Do not replace the production alerting, metrics, or incident-response systems.
- Do not introduce a new signature algorithm or change checkpoint wire formats.

## Compact self-contained prototype boundary

- Build one Rust verification library and one offline command-line replay harness.
- Input is a read-only fixture directory of keys, checkpoints, proofs, and leaf archives.
- Expose parse, verify-signature, verify-consistency, advance, reopen, and inspect operations.
- Persist derived checkpoint state in one disposable local directory.
- The harness runs both implementations and compares decisions, cursors, and stable error codes.
- Use the existing eight fixture log identities and no dynamic key discovery.
- Generate additional Merkle trees from fixed seeds for property and crash tests.
- Publish only a complete JSONL decision ledger plus a digest manifest.
- The deliverable is source, tests, benchmark output, and a short fit decision.
- Time-box implementation and investigation to two developer days.

## Fault, crash, and adversarial cases

- Flip every bit position in representative roots, proof nodes, sizes, and signatures.
- Supply a valid proof under the wrong log identity or public key.
- Truncate envelopes and archive records at every field boundary.
- Declare proof counts of 0, 64, 128, 129, and the maximum unsigned integer.
- Use impossible tree-size transitions, rollback sizes, and arithmetic-overflow boundaries.
- Present two signed checkpoints with the same log and size but different roots.
- Duplicate checkpoints before and after advancement and reorder up to 16 pending items.
- Omit a declared proof base, then deliver it after dependent candidates are pending.
- Corrupt the stored frontier, newest manifest, decision tail, and archive cursor separately.
- Terminate after state write, proof write, decision write, flush, rename, and directory sync.
- Simulate disk-full and permission-denied results at every publication stage.
- Cancel a verification worker before decision submission, then resubmit the same proof.
- Feed 4 KiB envelopes containing invalid lengths designed to provoke large allocations.

## Evidence for the fit decision

### Keep the baseline

- Keep it if any adversarial case advances state or any valid corpus case disagrees.
- Keep it if recovery can expose a cursor without its matching verified checkpoint and proof.
- Keep it if the replay exceeds 90 seconds or measured memory exceeds 224 MiB.
- Keep it if diagnostics or on-disk state are harder for security engineers to inspect.
- Keep it if equivalent durability still requires most of the existing SQLite implementation.

### Adopt a component

- Adopt only the narrow verifier or durable-checkpoint component that passes every gate.
- Require stable typed failures, bounded parsing, and no panic across the entire corpus.
- Require old-or-new recovery at every tested exit boundary and deterministic decision bytes.
- Require a migration path that can shadow the Go baseline before it owns advancement.
- Require independent review of tree arithmetic, signature handling, and durability assumptions.

### Conclude no fit

- Conclude no fit if cryptographic proof bytes cannot be preserved and audited exactly.
- Conclude no fit if atomic checkpoint, proof, cursor, and decision publication is unavailable.
- Conclude no fit if storage abstractions obscure corruption or make repair ambiguous.
- Conclude no fit if performance comes from buffering beyond the 256 MiB limit.
- A no-fit result is acceptable and should retain failing vectors and recovery traces.
