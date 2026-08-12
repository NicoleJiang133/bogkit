# MIDI scheduler real-time model

This standalone crate turns the trial brief into a runnable fake-host model. It deliberately has no BogKit dependency because Fold, ESE, and ANNy do not fit an exact hard-real-time scheduler callback.

Run the complete evidence suite from the BogKit repository root:

```sh
cargo fmt --manifest-path developer-simulation/Cargo.toml -p midi-scheduler-rt-model -- --check
cargo test --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p midi-scheduler-rt-model --all-targets
cargo clippy --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p midi-scheduler-rt-model --all-targets -- -D warnings -D clippy::all -D clippy::pedantic
```

Run the release demonstration and local timing histogram:

```sh
cargo run --manifest-path developer-simulation/Cargo.toml --locked --offline --release -p midi-scheduler-rt-model --bin scheduler-demo -- 10000000
```

The demonstration records the release binary digest, plan/event digest, compiler profile, CPU-model availability, affinity status, background-load policy, p99, maximum, and raw latency bins. It is a local observation, not a portable latency guarantee.

Callback tokens must be strictly increasing across every successfully scheduled block, including transport discontinuities. Equal or older tokens return the stable `NonMonotonicToken` error and write nothing. A call that fails validation or cannot fit even one channel reset does not consume its token, so the caller may retry that exact token with corrected inputs or a larger output slice. This bounded rule avoids an unbounded token-history structure.

Plan preparation binds each note instance to its `channel` and `key`. A `NoteOff` with the right instance ID but a different identity returns stable error code 16, `TerminationIdentityMismatch`, before publication. A replacement that reuses an instance ID for another channel or key terminates the old active identity.

If ordinary `NoteOff` demand exceeds output capacity, the callback emits exactly one synthetic `AllNotesOff` for each affected channel, clears every active note on those channels, suppresses ordinary note lifecycle events for that block, and sets sticky overflow. This prevents truncated NoteOff output from leaving active instances stuck.

The allocator trap covers 100,000 callback invocations and counts `alloc`, `alloc_zeroed`, and `realloc`; it does not count deallocation. Lock-attempt interception and forbidden-system-call interception are not available here. The two-slot handoff test is a deterministic single-thread instrumented model of before/after-read schedules, not a proof for an actual lock-free concurrent primitive.

The release timing and binary digest identify one locally rebuilt artifact. The digest is expected to change after rebuilding with different source, compiler, or environment inputs, and the timing remains an observation rather than a portable guarantee.
