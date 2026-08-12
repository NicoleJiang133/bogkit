# Trial 2: Hard-real-time MIDI event scheduler

## Developer role and Rust experience

You are an audio-tools developer maintaining a desktop MIDI sequencer used in rehearsals and small live performances. You have ten years of production C++ audio experience. Your Rust experience is exact: you completed the Rust Book, built two hobby command-line crates totaling about 3,000 lines, and have never shipped Rust in an audio callback or used unsafe Rust.

## Existing system and baseline

The sequencer is an existing C++ desktop application. Its audio callback receives a host-provided block start frame, block length, sample rate, transport state, and immutable tempo-map pointer. A C++ scheduler converts queued beat-positioned MIDI events into integer offsets inside each callback block and writes them into a fixed-capacity output buffer. UI edits build a new immutable event plan and swap it at a block boundary.

The current scheduler and a slow, offline rational-arithmetic reference implementation are the baselines. Device I/O and the audio engine are not being replaced.

## Concrete pain

Users occasionally hear doubled notes, stuck notes, or a one-sample rhythmic shift when an event lands exactly on a block boundary, the host changes block size, tempo automation is dense, or the transport loops. The current code also has an undocumented queue-overflow path. The team wants to determine whether a small Rust scheduling core can make boundary and overload behavior explicit without violating the callback's real-time contract. If it cannot, keeping the C++ scheduler is the correct result.

## Workload and data shape

An event plan is an immutable array of at most 200,000 events sorted by `(beat, priority, stable_id)`. Beat positions and tempo-node positions are signed 64-bit integers in pulses, with exactly 960 pulses per quarter note. Each event is one of `NoteOn`, `NoteOff`, `ControlChange`, `ProgramChange`, or `AllNotesOff`, with channel and data bytes. Each accepted `NoteOn` has a unique `note_instance_id`; its `NoteOff` carries the same ID.

A tempo map has at most 50,000 nodes. Each segment has a constant microseconds-per-quarter value; ramps and floating-point tempo are out of scope. The host supplies integer sample rates of 44,100, 48,000, or 96,000 Hz. A callback block is the half-open frame interval `[start_frame, start_frame + length)`, with lengths varying independently from 16 through 2,048 frames.

The representative run is a 45-minute synthetic performance with 180,000 events, 20,000 tempo nodes, 250 plan replacements, 10,000 block-size changes, 100 forward seeks, 100 backward seeks, and 500 loop wraps. A separate saturation trace presents 4,096 events at one frame to an output buffer that holds 256.

## Operational constraints

- The callback-facing scheduling function must perform zero heap allocations, acquire no locks, perform no system calls, do no logging, and never panic or unwind. Initialization and plan construction may allocate off the audio thread.
- The callback may read only an immutable published plan plus fixed-size scheduler state. Plan replacement occurs by an explicit prepare/publish/reclaim protocol; the audio thread cannot free the old plan.
- Output capacity is fixed at initialization. Ordering at the same frame is `AllNotesOff`, `NoteOff`, `ControlChange`, `ProgramChange`, then `NoteOn`, followed by ascending `stable_id`.
- Frame conversion uses checked integer or rational arithmetic. Inputs whose conversion would overflow are rejected during plan construction; no saturating or wrapping timestamp conversion is allowed.
- A normal block uses `[start, end)`: an event at `end` belongs to the next block. A repeated callback for the same block token emits no event a second time.
- On seek, loop wrap, sample-rate change, or plan replacement that invalidates an active note, emit the minimum set of synthetic terminations before any ordinary event: one `NoteOff` per known active instance when capacity permits; if terminations exceed capacity, emit exactly one `AllNotesOff` per affected channel and suppress ordinary `NoteOn` events for that block.
- When an ordinary block exceeds output capacity, retain events in priority order. Never discard a termination to keep a `NoteOn`. Set a sticky overflow flag readable from the control thread; clearing the flag is an explicit control-thread operation.

## Fault and adversarial cases

The harness must exercise all of the following:

- events at the first frame, last included frame, and first excluded frame of blocks at every supported sample rate;
- a tempo change on the same pulse as a note, consecutive tempo nodes, the earliest and latest admitted pulse, and conversions one unit below and above overflow;
- host block sequences containing sizes 16, 17, 255, 256, 257, 1,024, and 2,048 with no alignment assumptions;
- duplicate callback tokens, a skipped block followed by a forward seek, a backward seek into an active note, loop start equal to an event pulse, and 100 consecutive loop wraps;
- plan publication immediately before and immediately after a callback reads the generation number, with delayed reclamation of the prior plan;
- a malformed plan with unsorted events, duplicate `stable_id`, unmatched `NoteOff`, duplicate termination for one instance, invalid MIDI data bytes, and an unsupported sample rate;
- the 4,096-event saturation frame, including more than 256 terminations across all 16 channels and a mixture of every event type;
- a callback invocation under an allocator trap, a lock-attempt trap, a syscall-counting harness, and a panic boundary.

## Measurable acceptance criteria

Each criterion is evaluated through public functions or the compiled black-box harness; source inspection is not accepted as proof:

1. **Frame oracle parity:** Across 100 fixed random seeds, every non-overflow trace emits exactly the same `(block_token, frame_offset, event, stable_id)` sequence as the rational-arithmetic reference. This includes all boundary and tempo-change fixtures; one mismatch fails the trial.
2. **Exactly-once block replay:** Repeating each of 1,000 selected callback tokens one to three times does not change the emitted sequence digest and produces no duplicate event IDs.
3. **Transport safety:** For every seek, loop, sample-rate change, and invalidating plan replacement, the emitted first-block termination set matches a checked-in oracle. After the discontinuity, the harness finds zero active note instances from the prior transport generation.
4. **Saturation safety:** In the 4,096-event frame, output length never exceeds 256. If per-instance terminations fit, every termination is present before any `NoteOn`; if they do not fit, the output contains exactly one `AllNotesOff` for each affected channel, contains no `NoteOn`, and sets the sticky overflow flag.
5. **Invalid-plan immutability:** Every malformed or overflowing plan is rejected before publication with a stable error code. The generation ID and digest of the active plan remain unchanged, and the next valid callback matches the pre-error oracle.
6. **Publication race:** A deterministic two-thread scheduler explores all instrumented handoff points for 10,000 replacements. Every callback observes one complete generation, never a mixture; every retired plan is reclaimed exactly once on the control thread after the harness proves no callback can reference it.
7. **Real-time contract:** Over 10,000,000 callback invocations, the instrumented allocator count, lock-attempt count, and forbidden-system-call count are all zero. Forced invalid inputs return an error flag without panic or unwind.
8. **Latency:** On the declared reference machine, 10,000,000 warmed callback invocations with blocks of up to 2,048 frames have p99 below 20 microseconds and maximum below 100 microseconds. The harness records the binary digest, compiler settings, CPU model, core affinity, background-load policy, and raw latency histogram so the claim is reproducible rather than portable by assertion.
9. **Determinism:** Five runs of the representative trace and every adversarial fixture produce byte-identical event-log and diagnostic digests.

## Explicit non-goals

- No audio rendering, synthesis, device drivers, host plug-in wrapper, user interface, recording, quantization, or network MIDI.
- No tempo ramps, swing, humanization, floating-point beat positions, time-stretching, or musical-expression inference.
- No replacement of the application's event editor, audio engine, or C++ device layer.
- No dynamic output growth, background logging from the callback, or attempt to hide overload.
- No unsafe Rust written solely to meet the latency target; any unsafe block introduced by a dependency must already be covered by that dependency's maintained tests and safety documentation.

## Smallest self-contained prototype boundary

Build a Rust library with three public operations: validate and prepare an immutable plan off-thread; publish a prepared generation through a fixed handoff object; and schedule one host block into a caller-provided fixed-capacity event slice. Add a deterministic trace runner, the rational-arithmetic oracle, and the black-box instrumentation needed by the acceptance criteria. The prototype integrates only with a fake host; it does not load into the desktop application.

Any new crate must be assessed separately for callback allocation, locking, panic behavior, integer conversion semantics, maintenance burden, binary-size effect, and benchmark impact. Reject a crate if it violates one acceptance gate, requires a resident runtime or control thread not already present, obscures the reclamation proof, or performs worse than the C++ baseline without a compensating correctness gain. Reject the entire Rust approach if the complete prototype cannot satisfy all safety gates with an operational story the audio team can support. A measured rejection is a valid outcome.
