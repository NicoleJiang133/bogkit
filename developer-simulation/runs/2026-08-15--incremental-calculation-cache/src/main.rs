use std::path::Path;

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let data_path = arguments
        .next()
        .expect("usage: financial-snapshot-trial <temporary-data-dir> [manifest-path]");
    let manifest = financial_snapshot_trial::run_demo(Path::new(&data_path))
        .expect("the local demonstration must complete");
    let json = serde_json::to_string_pretty(&manifest).expect("manifest serialization must work");
    if let Some(manifest_path) = arguments.next() {
        std::fs::write(manifest_path, format!("{json}\n")).expect("manifest path must be writable");
    }
    println!("{json}");
}
