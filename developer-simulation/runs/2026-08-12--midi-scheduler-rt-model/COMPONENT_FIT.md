# BogKit component fit assessment

This was written after the required blind discovery note and after inspecting only the public entry points needed to decide callback fit.

## Fold — no fit at the callback boundary

The public `Stream` owns a persistent Fjall database. Construction opens a path. Every public read enters `store.read_tx()` and creates a pinned snapshot; public writes create/commit transactions, and checkpointing explicitly syncs storage. Public initialization and transaction paths contain `unwrap`, `assert`, panic-catching, and unwind resumption. The public API does not expose a pre-pinned immutable event slice with a fixed-state callback cursor or an audio-thread reclamation protocol.

The chat example reinforces the sane split: one ordinary thread owns Fold and copies snapshots toward consumers. Fold could plausibly be used by a future event editor or offline catalogue on the control side, but adding it there is outside this scheduler trial and would not prove callback safety. I did not build a Fold dependency into the prototype because its documented/public contract already fails the callback gates and there was no scheduling behaviour it could add.

## ESE — no fit

Its public operation maps text into embedding vectors. `encode` returns a `Vec`; `encode_single` allocates normalized and word-piece `String` buffers. Exact beat-to-frame conversion has no text or semantic-similarity step. It would add allocation and irrelevant work without satisfying any scheduler requirement.

## ANNy — no fit

ANNy is approximate nearest-neighbour search. Exact half-open time selection cannot tolerate approximate results. Its public `search` returns a `Vec`, uses a thread-local `RefCell<Vec<_>>`, and resizes that visited buffer. Its index is also heap-backed. Even if warmed allocation happened to disappear, the public contract neither guarantees exact selection nor bounded callback state.

## Dependency result

The standalone scheduler uses only Rust's standard library. `cargo tree` reports only `midi-scheduler-rt-model`. The release demonstration binary measured 505,712 bytes. There is no resident runtime or background thread. No BogKit component is used in the callback model.
