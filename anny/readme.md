**ANNy provides performance-oriented data structures for approximate nearest neighbor (ANN) search.**

[Rust 1.56]: https://blog.rust-lang.org/2021/10/21/Rust-1.56.0/

---

ANNy provides a fast, no-dependency [HNSW](https://arxiv.org/abs/1603.09320) (Hierarchical Navigable Small World) index for k-nearest-neighbor search over fixed-dimension vectors, with support for insertion, search, and removal. Index parameters are const generics, so the index tuning is fixed at compile time and search buffers live on the stack.

## Basic use

```rust
use anny::hnsw::Hnsw;
use anny::metric::L2;

// Hnsw<Dtype, Metric, DIM, M_0, K, EF_SEARCH, EF_BUILD, MAX_LEVEL>
//   Dtype     element type of the vectors (f32, i32, ...)
//   Metric    distance metric (smaller = closer)
//   DIM       vector dimensionality
//   M_0       max neighbors per node at layer 0 (upper layers use M_0 / 2)
//   K         number of results returned by search
//   EF_SEARCH search-time candidate list size (must be >= K)
//   EF_BUILD  build-time candidate list size
//   MAX_LEVEL max number of graph layers
let mut index: Hnsw<f32, L2, 4, 16, 3, 20, 40, 12> = Hnsw::new(L2, 42);

let a = index.insert([0.0, 0.0, 0.0, 0.0]);
let b = index.insert([1.0, 0.0, 0.0, 0.0]);
let c = index.insert([9.0, 9.0, 9.0, 9.0]);

// returns up to K (distance, id) pairs, ascending by distance
let results = index.search(&[0.1, 0.0, 0.0, 0.0]);
assert_eq!(results[0].1, a);

// removal keeps the graph healthy for future searches
index.remove(c);
```

Available metrics in `anny::metric`: `L2` (squared Euclidean), `Euclidean`, `L1`, `Chebyshev`, `Cosine`, `NegDot`, and `Hamming`. Custom metrics can be added by implementing the `Metric` trait.

