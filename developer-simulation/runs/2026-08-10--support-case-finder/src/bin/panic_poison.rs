use std::panic::{AssertUnwindSafe, catch_unwind};

use fold::pipeline::terminal;
use fold::stream::KeyedStream;

fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/run-data")
        .join(format!("bogkit-panic-poison-repro-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);

    let mut store = KeyedStream::new(&path, terminal::Table::new("rows"));
    store.wtx(|tx| {
        tx.upsert(&1_u64, &"committed".to_owned());
    });

    let caught = catch_unwind(AssertUnwindSafe(|| {
        store.wtx(|tx| {
            tx.upsert(&1_u64, &"uncommitted".to_owned());
            panic!("simulated refresh failure");
        });
    }));
    assert!(caught.is_err());
    assert_eq!(store.get(&1), Some("committed".to_owned()));
    println!("read after caught panic still sees committed state");
    println!("attempting a second write in the same process");

    // On the evaluated revision this panics with `poisoned tx lock`, even though the
    // `KeyedStream::wtx` documentation says a panic rolls the transaction back.
    store.wtx(|tx| {
        tx.upsert(&2_u64, &"next write".to_owned());
    });
}
