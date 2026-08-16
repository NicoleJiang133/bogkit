use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use fold::pipeline::terminal;
use fold::stream::KeyedStream;

fn temporary_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("fold-causal-comparison-{label}-{nonce}"))
}

#[test]
fn fold_keyed_materialization_is_arrival_ordered_without_a_causal_wrapper() {
    // Narrow characterization, not a Fold defect: a generic keyed upsert does
    // not know the canvas operation's causal writer tuple. The two schedules
    // therefore need our custom resolution layer before materialization.
    let forward_path = temporary_path("forward");
    let reverse_path = temporary_path("reverse");
    let mut forward = KeyedStream::new(&forward_path, terminal::Table::new("values"));
    forward.wtx(|transaction| {
        transaction.upsert(&"shape.fill".to_string(), &"alice-red".to_string());
        transaction.upsert(&"shape.fill".to_string(), &"bob-blue".to_string());
    });
    let mut reverse = KeyedStream::new(&reverse_path, terminal::Table::new("values"));
    reverse.wtx(|transaction| {
        transaction.upsert(&"shape.fill".to_string(), &"bob-blue".to_string());
        transaction.upsert(&"shape.fill".to_string(), &"alice-red".to_string());
    });

    assert_eq!(
        forward.get(&"shape.fill".to_string()),
        Some("bob-blue".to_string())
    );
    assert_eq!(
        reverse.get(&"shape.fill".to_string()),
        Some("alice-red".to_string())
    );
    fs::remove_dir_all(forward_path).unwrap();
    fs::remove_dir_all(reverse_path).unwrap();
}
