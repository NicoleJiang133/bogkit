# Fold

**Fold is our take on an incremental programming framework — iterator-like primitives for materializing a stream of ever-changing data into views. It's the engine that powers Bog.**

---

Fold is built around *deltas*: each incoming datum carries a signed multiplicity — positive for insertion, negative for removal. Deltas flow through a statically composed pipeline of operators (`Map`, `Filter`, `FlatMap`, `Distinct`, keyed/scored variants, ...) into persistent sinks (counts, bags, tables, ordered indexes, histograms, full-text search). Every sink is maintained incrementally: instead of recomputing a view from scratch, fold folds in only the actual change, so work is proportional to the size of the delta, not the dataset. Removing a previously inserted record retracts its effect everywhere.

State lives in an embedded [fjall](https://docs.rs/fjall) LSM store. Writes are transactional and crash-safe; reads observe one consistent snapshot across all sinks.

## Basic use

```rust
use fold::pipeline::{Filter, Map, terminal};
use fold::stream::Stream;

// Pipelines are built inside-out: each operator wraps its downstream.
// Tuples fan a stream out to multiple branches.
let mut st = Stream::new(
    "example.db",
    Filter::new(
        |s: &String| !s.is_empty(),
        (
            terminal::Count::new("total"),
            Map::new(|s: &String| s.len(), terminal::Bag::new("lengths")),
        ),
    ),
);

// Writes are atomic: all sinks observe the whole batch or none of it.
st.wtx(|tx| {
    tx.insert(&"hello".to_string());
    tx.insert(&"world".to_string());
});

// Reads span one snapshot; readers mirror the pipeline's sink structure.
st.rtx(|(count, lengths)| {
    assert_eq!(count.get(), 2);
    assert!(lengths.contains(&5));
});

// Removal retracts: sinks roll back as if the record was never inserted.
st.wtx(|tx| tx.remove(&"hello".to_string()));
st.rtx(|(count, _)| assert_eq!(count.get(), 1));
```

## More detail

The rustdocs cover the full set of operators, terminal sinks, and transaction APIs. Build and open them locally with:

```bash
cargo doc -p fold --open
```
