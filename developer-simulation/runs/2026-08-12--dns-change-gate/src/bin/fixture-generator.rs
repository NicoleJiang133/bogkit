#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("fixture-generator: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments = parse_args(env::args_os().skip(1))?;
    let root = value(&arguments, "--root").map(PathBuf::from)?;
    let zones = number(&arguments, "--zones")?;
    let records_per_zone = number(&arguments, "--records-per-zone")?;
    let changed_zones = number(&arguments, "--changed-zones")?;
    if zones == 0 || zones > 10_000 {
        return Err("--zones must be in 1..=10000".to_string());
    }
    if !(3..=250_000).contains(&records_per_zone) {
        return Err("--records-per-zone must be in 3..=250000".to_string());
    }
    if changed_zones > zones {
        return Err("--changed-zones cannot exceed --zones".to_string());
    }
    if root.exists() {
        return Err(format!(
            "refusing to overwrite existing root {}",
            root.display()
        ));
    }
    let old_root = root.join("old");
    let new_root = root.join("new");
    fs::create_dir_all(&old_root).map_err(|error| error.to_string())?;
    fs::create_dir_all(&new_root).map_err(|error| error.to_string())?;

    let policy_path = root.join("policy.conf");
    let policy_file = fs::File::create(&policy_path).map_err(|error| error.to_string())?;
    let mut policy = io::BufWriter::new(policy_file);
    let total_limit = zones
        .checked_mul(records_per_zone)
        .ok_or_else(|| "record total overflow".to_string())?;
    writeln!(policy, "max_records_total={total_limit}").map_err(|error| error.to_string())?;
    writeln!(policy, "max_records_per_zone={records_per_zone}")
        .map_err(|error| error.to_string())?;
    writeln!(policy, "max_include_depth=32").map_err(|error| error.to_string())?;
    writeln!(policy, "max_generate_records=100000").map_err(|error| error.to_string())?;
    writeln!(policy, "ttl_decrease_percent=50").map_err(|error| error.to_string())?;

    for zone_index in 0..zones {
        let zone = format!("zone{zone_index:05}.example.");
        let file_name = format!("zone{zone_index:05}.zone");
        writeln!(policy, "zone={zone}|{file_name}|PASS|PASS").map_err(|error| error.to_string())?;
        write_zone(
            &old_root.join(&file_name),
            &zone,
            1,
            records_per_zone,
            zone_index,
            false,
        )?;
        write_zone(
            &new_root.join(&file_name),
            &zone,
            if zone_index < changed_zones { 2 } else { 1 },
            records_per_zone,
            zone_index,
            zone_index < changed_zones,
        )?;
    }
    policy.flush().map_err(|error| error.to_string())?;
    println!(
        "zones={zones} records_per_snapshot={} changed_zones={changed_zones} root={}",
        zones * records_per_zone,
        root.display()
    );
    Ok(())
}

fn parse_args(
    values: impl Iterator<Item = OsString>,
) -> Result<BTreeMap<String, OsString>, String> {
    let mut values = values;
    let mut arguments = BTreeMap::new();
    while let Some(flag) = values.next() {
        let flag = flag
            .into_string()
            .map_err(|_| "argument flag is not UTF-8".to_string())?;
        if !matches!(
            flag.as_str(),
            "--root" | "--zones" | "--records-per-zone" | "--changed-zones"
        ) {
            return Err(format!("unknown argument {flag}"));
        }
        let argument = values
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        if arguments.insert(flag.clone(), argument).is_some() {
            return Err(format!("duplicate argument {flag}"));
        }
    }
    Ok(arguments)
}

fn value<'a>(
    arguments: &'a BTreeMap<String, OsString>,
    name: &str,
) -> Result<&'a OsString, String> {
    arguments.get(name).ok_or_else(|| format!("missing {name}"))
}

fn number(arguments: &BTreeMap<String, OsString>, name: &str) -> Result<usize, String> {
    value(arguments, name)?
        .to_str()
        .ok_or_else(|| format!("{name} is not UTF-8"))?
        .parse::<usize>()
        .map_err(|_| format!("{name} is not an unsigned integer"))
}

fn write_zone(
    path: &Path,
    zone: &str,
    serial: u32,
    records: usize,
    zone_index: usize,
    changed: bool,
) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|error| error.to_string())?;
    let mut output = io::BufWriter::new(file);
    writeln!(output, "$ORIGIN {zone}").map_err(|error| error.to_string())?;
    writeln!(output, "$TTL 3600").map_err(|error| error.to_string())?;
    writeln!(output, "@ SOA ns hostmaster {serial} 3600 900 604800 300")
        .map_err(|error| error.to_string())?;
    writeln!(output, "@ NS ns").map_err(|error| error.to_string())?;
    writeln!(output, "ns A 192.0.2.1").map_err(|error| error.to_string())?;
    for record_index in 0..records - 3 {
        let address = if changed {
            format!(
                "198.{}.{}.{}",
                18 + zone_index % 2,
                (record_index / 250) % 250,
                record_index % 250 + 1
            )
        } else {
            let mut address = String::new();
            write!(
                address,
                "10.{}.{}.{}",
                zone_index % 250,
                (record_index / 250) % 250,
                record_index % 250 + 1
            )
            .expect("write to string");
            address
        };
        writeln!(output, "host{record_index:06} A {address}").map_err(|error| error.to_string())?;
    }
    output.flush().map_err(|error| error.to_string())
}
