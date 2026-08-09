# Incremental syntax highlighting prototype

This standalone Rust prototype keeps an authoritative full lexer and a line-oriented incremental cache for the same token rules. It was built for the Trial 1 scenario and does not modify or depend on BogKit.

The incremental path validates byte coordinates before mutation, relexes from the touched line, and stops only after scanning a preserved boundary line and exactly matching its content, incoming state, outgoing state, and relative tokens. The fingerprint is a secondary check, never the sole reason to reuse a suffix. Token ranges are UTF-8 byte ranges. The cache also maintains one canonical entry per logical line, including exactly one empty terminal line when the document ends in a newline.

## Requirements

- Rust 1.95.0 (edition 2024)
- Dependencies resolved by the archive's single `developer-simulation/Cargo.lock`; use `--offline --locked`
- No network, service, database, or private data

## Run

From this directory:

```console
cargo fmt --check -p incremental-syntax-highlighting
cargo test --offline --locked --release -p incremental-syntax-highlighting --all-targets
cargo clippy --offline --locked --release -p incremental-syntax-highlighting --all-targets --all-features -- -D warnings
cargo run --offline --locked --release -p incremental-syntax-highlighting -- correctness
cargo run --offline --locked --release -p incremental-syntax-highlighting -- adversarial
cargo run --offline --locked --release -p incremental-syntax-highlighting -- benchmark
cargo run --offline --locked --release -p incremental-syntax-highlighting -- reproducibility
```

`benchmark` builds the deterministic approximately 10 MiB / 200,000-line fixture, applies exactly 2,000 edits, and compares against a fresh full lex after every edit. It takes about 70 seconds on the trial machine. `benchmark --no-oracle` skips those full comparisons for a faster instrumentation run; it is not a substitute for the oracle run.

`reproducibility` runs the complete seeded benchmark twice, serializes each canonical `(LexResult, WorkCounters)` value, and requires the two byte vectors to be exactly equal. It separately compares the stable measurement fields. The 64-bit digests remain useful compact labels but are not treated as proof of equality.

Before the fix, the reviewer directly reproduced an exact differential failure on the cross-line edit from `"/λa\n\n"` to `"/λ\r\n\"🙂🙂\n"`: incremental lexing emitted String ranges `5..14` and `14..15`, while fresh full lex emitted the single range `5..15`. The permanent regression now checks the canonical three-line cache partition and the single `5..15` String token; the EOF splice fix removed the mismatch.

The same case can be exercised through the release CLI with the checked-in synthetic fixtures:

```console
cargo run --offline --locked --release -p incremental-syntax-highlighting -- apply tests/fixtures/cross-line-input.txt tests/fixtures/cross-line-edits.jsonl --check
```

## Apply a JSONL edit stream

```console
cargo run --offline --locked --release -p incremental-syntax-highlighting -- apply DOCUMENT EDITS.jsonl --check
```

Each non-empty JSONL line has this shape:

```json
{"start": 12, "delete": 3, "insert": "replacement"}
```

The long field names `deleted_bytes` and `replacement` are also accepted. `--check` compares the incremental result with a fresh full lex after every edit. The command emits a deterministic final digest, edit counters, scanned line/byte counters, and a maximum retained-index capacity estimate.

## Other commands

```console
cargo run --offline --locked --release -p incremental-syntax-highlighting -- baseline
```

This performs one full lex of a deterministic 10 MiB fixture and reports the scan cost and digest.

## What is counted

The locality counters count bytes and lines passed through the lexer. They intentionally do not claim that updating the contiguous `String` or rebuilding the line-start vector is free.

The retained-index estimate adds owned vector capacities, exact cached line text, cached relative tokens and states, plus a heuristic 32-byte allowance for every retained backing allocation. The maximum reported value is sampled after initialization and each completed edit. This is **not peak RSS or a complete peak-allocation measurement**: it omits transient allocations during edit reconstruction, line splitting, full-result assembly, oracle comparison, and JSON serialization, as well as allocator fragmentation and other process memory. The document and a separately assembled emitted-token result are also excluded to mirror the scenario's index-only boundary. A value below 64 MiB therefore shows only that the internally counted retained index is below that threshold; it does not demonstrate the full process-memory acceptance criterion.

Seeds are compiled into the harness and printed in every large result. Wall-clock time is informational and is excluded from reproducibility equality; canonical result bytes, exact counters, and all stable measurement fields are compared.

## Archive normalization verification

The standalone trial's child workspace, child lockfile, and package-level release profile were removed during archival. The package was rebuilt with the single nested lab lockfile and default workspace release profile. From that final archive shape, the full 10 MiB / 2,000-edit oracle run again passed after every edit in 72,696 ms, and exact reproducibility again compared two equal 20,576,672-byte canonical values. The memory result remains only the retained-index estimate described above.
