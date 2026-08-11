use std::path::PathBuf;

use fold::pipeline::terminal;
use fold::stream::KeyedStream;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TaggedRecord {
    Reading {
        id: u64,
        #[serde(default)]
        replaces: Option<u64>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SkippedFieldRecord {
    Reading {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replaces: Option<u64>,
    },
}

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let mode = arguments.next().unwrap_or_else(|| "tagged".into());
    let path = arguments.next().map_or_else(
        || std::env::temp_dir().join("fold-postcard-repro"),
        PathBuf::from,
    );
    let _ = std::fs::remove_dir_all(&path);
    let result = std::panic::catch_unwind(|| match mode.to_str() {
        Some("tagged") => {
            let mut stream = KeyedStream::new(&path, terminal::Table::new("records"));
            stream.wtx(|transaction| {
                transaction.upsert(
                    &1_u64,
                    &TaggedRecord::Reading {
                        id: 1,
                        replaces: None,
                    },
                );
            });
            stream.rtx(|table| println!("records: {}", table.iter().count()));
        }
        Some("skipped") => {
            let mut stream = KeyedStream::new(&path, terminal::Table::new("records"));
            stream.wtx(|transaction| {
                transaction.upsert(
                    &1_u64,
                    &SkippedFieldRecord::Reading {
                        id: 1,
                        replaces: None,
                    },
                );
            });
            stream.rtx(|table| println!("records: {}", table.iter().count()));
        }
        _ => panic!("mode must be tagged or skipped"),
    });
    let _ = std::fs::remove_dir_all(&path);
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
