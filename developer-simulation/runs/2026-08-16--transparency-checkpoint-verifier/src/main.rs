#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use ring::digest::{SHA256, digest};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use transparency_checkpoint_verifier::{
    DirectoryStore, Engine, ErrorCode, KnownLog, Registry, TreeBuilder, hex,
};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [command, count] if command == "replay" => replay(count),
        [command, count] if command == "bench-incremental" => bench_incremental(count),
        [command, expected, actual] if command == "compare" => compare(expected, actual),
        [command, directory] if command == "demo" => demo(Path::new(directory)),
        [command, directory] if command == "inspect" => inspect(Path::new(directory)),
        _ => Err("usage: transparency-checkpoint-verifier replay <leaf-count> | bench-incremental <iterations> | compare <expected> <actual> | demo <directory> | inspect <directory>".to_string()),
    }
}

fn bench_incremental(count: &str) -> Result<ExitCode, String> {
    let count: usize = count
        .parse()
        .map_err(|_| "iterations must be an unsigned integer".to_string())?;
    if !(1..=100_000).contains(&count) {
        return Err("iterations must be between 1 and 100000".to_string());
    }
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| "generate benchmark key".to_string())?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|_| "parse benchmark key".to_string())?;
    let public_key: [u8; 32] = pair
        .public_key()
        .as_ref()
        .try_into()
        .map_err(|_| "benchmark public-key length".to_string())?;
    let registry = Registry::new(vec![KnownLog {
        log_id: "benchmark-log".to_string(),
        key_id: "benchmark-key".to_string(),
        algorithm: 1,
        public_key,
    }])
    .map_err(|code| format!("benchmark registry: {code:?}"))?;
    let envelope = signed_first_checkpoint(&pair);

    for _ in 0..50 {
        benchmark_once(registry.clone(), &envelope)?;
    }
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        let started = Instant::now();
        benchmark_once(registry.clone(), &envelope)?;
        samples.push(started.elapsed().as_micros());
    }
    samples.sort_unstable();
    let percentile_index = (count * 95).div_ceil(100).saturating_sub(1);
    println!(
        "{{\"iterations\":{count},\"p95_us\":{}}}",
        samples[percentile_index]
    );
    Ok(ExitCode::SUCCESS)
}

fn benchmark_once(registry: Registry, envelope: &[u8]) -> Result<(), String> {
    let outcome = Engine::new(registry).submit(envelope, 1);
    if outcome.code == ErrorCode::Advanced {
        Ok(())
    } else {
        Err(format!("benchmark verification: {:?}", outcome.code))
    }
}

fn signed_first_checkpoint(pair: &Ed25519KeyPair) -> Vec<u8> {
    let mut leaf_input = [0_u8; 9];
    leaf_input[1..].copy_from_slice(&0_u64.to_be_bytes());
    let root = digest(&SHA256, &leaf_input);
    let log_id = b"benchmark-log";
    let key_id = b"benchmark-key";
    let mut envelope = Vec::new();
    envelope.extend_from_slice(b"CKP1");
    envelope.push(1);
    envelope.extend_from_slice(&1_u64.to_be_bytes());
    envelope.extend_from_slice(root.as_ref());
    envelope.extend_from_slice(&1_u64.to_be_bytes());
    envelope.push(u8::try_from(log_id.len()).expect("constant log identity is short"));
    envelope.push(u8::try_from(key_id.len()).expect("constant key identity is short"));
    envelope.extend_from_slice(log_id);
    envelope.extend_from_slice(key_id);
    let signature = pair.sign(&envelope);
    envelope.extend_from_slice(&0_u64.to_be_bytes());
    envelope.extend_from_slice(&0_u64.to_be_bytes());
    envelope.extend_from_slice(&64_u16.to_be_bytes());
    envelope.extend_from_slice(signature.as_ref());
    envelope
}

fn replay(count: &str) -> Result<ExitCode, String> {
    let count: u64 = count
        .parse()
        .map_err(|_| "leaf-count must be an unsigned integer".to_string())?;
    let started = Instant::now();
    let mut tree = TreeBuilder::new();
    for index in 0..count {
        let mut input = [0_u8; 9];
        input[1..].copy_from_slice(&index.to_be_bytes());
        let leaf = digest(&SHA256, &input)
            .as_ref()
            .try_into()
            .map_err(|_| "SHA-256 returned the wrong length".to_string())?;
        tree.append_leaf_hash(leaf)
            .map_err(|code| format!("tree append failed: {code:?}"))?;
    }
    let elapsed = started.elapsed();
    println!(
        "{{\"leaves\":{},\"root\":\"{}\",\"elapsed_ms\":{}}}",
        tree.count(),
        hex(&tree.root()),
        elapsed.as_millis()
    );
    Ok(ExitCode::SUCCESS)
}

fn compare(expected: &str, actual: &str) -> Result<ExitCode, String> {
    let expected = fs::read(expected).map_err(|error| format!("read expected ledger: {error}"))?;
    let actual = fs::read(actual).map_err(|error| format!("read actual ledger: {error}"))?;
    if expected == actual {
        println!("MATCH");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("MISMATCH");
        Ok(ExitCode::from(2))
    }
}

fn demo(directory: &Path) -> Result<ExitCode, String> {
    let logs = (0..8)
        .map(|index| KnownLog {
            log_id: format!("fixture-log-{index}"),
            key_id: "fixture-key".to_string(),
            algorithm: 1,
            public_key: [0_u8; 32],
        })
        .collect();
    let registry = Registry::new(logs).map_err(|code| format!("registry: {code:?}"))?;
    let snapshot = Engine::new(registry.clone()).snapshot();
    let store = DirectoryStore::new(directory);
    store
        .publish(&snapshot, None)
        .map_err(|code| format!("publish: {code:?}"))?;
    let started = Instant::now();
    let reopened = store.reopen().map_err(|code| format!("reopen: {code:?}"))?;
    let resumed = Engine::from_snapshot(registry, reopened)
        .map_err(|code| format!("resume validation: {code:?}"))?;
    println!(
        "{{\"logs\":{},\"reopen_ms\":{},\"visible_digest\":\"{}\"}}",
        resumed.snapshot().logs.len(),
        started.elapsed().as_millis(),
        hex(digest(
            &SHA256,
            &store
                .visible_bytes()
                .map_err(|code| format!("visible bytes: {code:?}"))?
        )
        .as_ref())
    );
    Ok(ExitCode::SUCCESS)
}

fn inspect(directory: &Path) -> Result<ExitCode, String> {
    let snapshot = DirectoryStore::new(directory)
        .reopen()
        .map_err(|code| format!("reopen: {code:?}"))?;
    let json = serde_json::to_string(&snapshot).map_err(|error| format!("serialize: {error}"))?;
    println!("{json}");
    Ok(ExitCode::SUCCESS)
}
