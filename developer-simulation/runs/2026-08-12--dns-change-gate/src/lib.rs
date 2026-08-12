#![forbid(unsafe_code)]

mod parser;
mod report;

use std::collections::BTreeMap;
use std::env;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub use parser::{RecordKey, ZoneData, parse_zone};
use report::{Report, compare_zone};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const TEMP_CREATE_ATTEMPTS: u32 = 128;

#[derive(Debug)]
pub struct GateError {
    pub code: &'static str,
    pub path: PathBuf,
    pub offset: u64,
    pub detail: String,
}

impl GateError {
    #[must_use]
    pub fn new(
        code: &'static str,
        path: impl Into<PathBuf>,
        offset: u64,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code,
            path: path.into(),
            offset,
            detail: detail.into(),
        }
    }
}

impl Display for GateError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}:{} {}",
            self.code,
            self.path.display(),
            self.offset,
            self.detail
        )
    }
}

impl std::error::Error for GateError {}

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_records_total: usize,
    pub max_records_per_zone: usize,
    pub max_include_depth: usize,
    pub max_generate_records: usize,
    pub ttl_decrease_percent: u32,
}

#[derive(Debug, Clone)]
pub struct ZonePolicy {
    pub zone: String,
    pub relative_path: PathBuf,
    pub old_baseline: String,
    pub new_baseline: String,
}

#[derive(Debug)]
pub struct Policy {
    pub limits: Limits,
    pub zones: Vec<ZonePolicy>,
}

#[derive(Debug)]
struct Args {
    old_root: PathBuf,
    new_root: PathBuf,
    policy: PathBuf,
    output: PathBuf,
}

/// Runs the command and returns its intended process status.
///
/// # Errors
/// Returns a stable diagnostic for command, policy, input, or publication errors.
pub fn run_from_env() -> Result<i32, GateError> {
    let args = parse_args(env::args_os().skip(1))?;
    execute(&args)
}

fn parse_args(values: impl Iterator<Item = std::ffi::OsString>) -> Result<Args, GateError> {
    let mut values = values;
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    while let Some(flag) = values.next() {
        let flag = flag
            .into_string()
            .map_err(|_| GateError::new("E_ARGS", "<argv>", 0, "argument is not UTF-8"))?;
        let Some(value) = values.next() else {
            return Err(GateError::new(
                "E_ARGS",
                "<argv>",
                0,
                format!("missing value for {flag}"),
            ));
        };
        if !matches!(
            flag.as_str(),
            "--old-root" | "--new-root" | "--policy" | "--output"
        ) {
            return Err(GateError::new(
                "E_ARGS",
                "<argv>",
                0,
                format!("unknown argument {flag}"),
            ));
        }
        if found.insert(flag.clone(), PathBuf::from(value)).is_some() {
            return Err(GateError::new(
                "E_ARGS",
                "<argv>",
                0,
                format!("duplicate argument {flag}"),
            ));
        }
    }
    let take = |key: &str| {
        found
            .get(key)
            .cloned()
            .ok_or_else(|| GateError::new("E_ARGS", "<argv>", 0, format!("missing {key}")))
    };
    Ok(Args {
        old_root: take("--old-root")?,
        new_root: take("--new-root")?,
        policy: take("--policy")?,
        output: take("--output")?,
    })
}

fn execute(args: &Args) -> Result<i32, GateError> {
    let policy = parse_policy(&args.policy)?;
    let old_root = canonical_root(&args.old_root)?;
    let new_root = canonical_root(&args.new_root)?;
    validate_output_path(&args.output, &old_root, &new_root, &args.policy)?;
    let mut old_total_records = 0_usize;
    let mut new_total_records = 0_usize;
    let mut zones = Vec::with_capacity(policy.zones.len());
    let mut input_paths = Vec::new();

    for selected in &policy.zones {
        let old = parse_zone(&old_root, selected, &policy.limits, &mut old_total_records)?;
        let new = parse_zone(&new_root, selected, &policy.limits, &mut new_total_records)?;
        input_paths.extend(old.input_paths.iter().cloned());
        input_paths.extend(new.input_paths.iter().cloned());
        zones.push(compare_zone(selected, &old, &new, &policy.limits));
    }

    validate_output_identities(&args.output, &args.policy, &input_paths)?;
    let report = Report::new(zones);
    let body = report.to_json();
    atomic_publish(&args.output, body.as_bytes())?;
    Ok(if report.is_blocked() { 2 } else { 0 })
}

fn canonical_root(path: &Path) -> Result<PathBuf, GateError> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        GateError::new(
            "E_ROOT",
            path,
            0,
            format!("cannot open snapshot root: {error}"),
        )
    })?;
    if !canonical.is_dir() {
        return Err(GateError::new(
            "E_ROOT",
            path,
            0,
            "snapshot root is not a directory",
        ));
    }
    Ok(canonical)
}

#[expect(
    clippy::too_many_lines,
    reason = "policy parsing is a linear validation pass and keeping its rules together aids audit"
)]
fn parse_policy(path: &Path) -> Result<Policy, GateError> {
    let text = fs::read_to_string(path).map_err(|error| {
        GateError::new("E_POLICY", path, 0, format!("cannot read policy: {error}"))
    })?;
    let mut values = BTreeMap::<String, String>::new();
    let mut zones = Vec::new();
    for (line_index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(GateError::new(
                "E_POLICY",
                path,
                line_index as u64,
                "expected key=value",
            ));
        };
        if key == "zone" {
            let fields: Vec<&str> = value.split('|').collect();
            if fields.len() != 4 || fields.iter().any(|field| field.is_empty()) {
                return Err(GateError::new(
                    "E_POLICY",
                    path,
                    line_index as u64,
                    "zone requires absolute-zone|relative-file|old-baseline|new-baseline",
                ));
            }
            if !fields[0].ends_with('.') {
                return Err(GateError::new(
                    "E_POLICY",
                    path,
                    line_index as u64,
                    "zone must be absolute",
                ));
            }
            zones.push(ZonePolicy {
                zone: fields[0].to_ascii_lowercase(),
                relative_path: PathBuf::from(fields[1]),
                old_baseline: fields[2].to_string(),
                new_baseline: fields[3].to_string(),
            });
        } else if values.insert(key.to_string(), value.to_string()).is_some() {
            return Err(GateError::new(
                "E_POLICY",
                path,
                line_index as u64,
                format!("duplicate key {key}"),
            ));
        }
    }
    zones.sort_by(|left, right| left.zone.cmp(&right.zone));
    for pair in zones.windows(2) {
        if pair[0].zone == pair[1].zone {
            return Err(GateError::new(
                "E_POLICY",
                path,
                0,
                format!("duplicate zone {}", pair[0].zone),
            ));
        }
    }
    if zones.is_empty() {
        return Err(GateError::new(
            "E_POLICY",
            path,
            0,
            "at least one zone is required",
        ));
    }

    let number = |key: &str| -> Result<usize, GateError> {
        let value = values
            .get(key)
            .ok_or_else(|| GateError::new("E_POLICY", path, 0, format!("missing {key}")))?;
        value
            .parse::<usize>()
            .map_err(|_| GateError::new("E_POLICY", path, 0, format!("invalid {key}")))
    };
    let ttl_percent = number("ttl_decrease_percent")?;
    if ttl_percent > 100 {
        return Err(GateError::new(
            "E_POLICY",
            path,
            0,
            "ttl_decrease_percent exceeds 100",
        ));
    }
    let limits = Limits {
        max_records_total: number("max_records_total")?,
        max_records_per_zone: number("max_records_per_zone")?,
        max_include_depth: number("max_include_depth")?,
        max_generate_records: number("max_generate_records")?,
        ttl_decrease_percent: u32::try_from(ttl_percent)
            .map_err(|_| GateError::new("E_POLICY", path, 0, "ttl percentage width"))?,
    };
    if limits.max_records_total == 0
        || limits.max_records_per_zone == 0
        || limits.max_include_depth == 0
        || limits.max_generate_records == 0
    {
        return Err(GateError::new(
            "E_POLICY",
            path,
            0,
            "all limits must be positive",
        ));
    }
    Ok(Policy { limits, zones })
}

fn atomic_publish(output: &Path, bytes: &[u8]) -> Result<(), GateError> {
    use std::io::Write;

    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let file_name = output
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| GateError::new("E_OUTPUT", output, 0, "output has no UTF-8 filename"))?;
    let (mut file, mut temporary) = create_owned_temp(parent, file_name, output)?;
    if env::var_os("DNS_GATE_FAULT").as_deref() == Some(std::ffi::OsStr::new("after-temp")) {
        std::process::exit(86);
    }
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            GateError::new(
                "E_OUTPUT",
                &temporary.path,
                0,
                format!("cannot flush temporary report: {error}"),
            )
        })?;
    if env::var_os("DNS_GATE_FAULT").as_deref() == Some(std::ffi::OsStr::new("after-flush")) {
        std::process::exit(87);
    }
    drop(file);
    fs::rename(&temporary.path, output).map_err(|error| {
        GateError::new(
            "E_OUTPUT",
            output,
            0,
            format!("cannot replace report: {error}"),
        )
    })?;
    temporary.owned = false;
    let directory = fs::File::open(parent).map_err(|error| {
        GateError::new(
            "E_OUTPUT",
            parent,
            0,
            format!("cannot open output directory for sync: {error}"),
        )
    })?;
    directory.sync_all().map_err(|error| {
        GateError::new(
            "E_OUTPUT",
            parent,
            0,
            format!("cannot sync output directory: {error}"),
        )
    })?;
    Ok(())
}

fn validate_output_path(
    output: &Path,
    old_root: &Path,
    new_root: &Path,
    policy: &Path,
) -> Result<(), GateError> {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = fs::canonicalize(parent).map_err(|error| {
        GateError::new(
            "E_OUTPUT",
            parent,
            0,
            format!("output parent must already exist: {error}"),
        )
    })?;
    if canonical_parent.starts_with(old_root) || canonical_parent.starts_with(new_root) {
        return Err(GateError::new(
            "E_OUTPUT_ALIAS",
            output,
            0,
            "output must be outside both immutable snapshot roots",
        ));
    }
    let file_name = output
        .file_name()
        .ok_or_else(|| GateError::new("E_OUTPUT", output, 0, "output must name a file"))?;
    let candidate = canonical_parent.join(file_name);
    let canonical_policy = fs::canonicalize(policy).map_err(|error| {
        GateError::new(
            "E_POLICY",
            policy,
            0,
            format!("cannot resolve policy path: {error}"),
        )
    })?;
    if candidate == canonical_policy {
        return Err(GateError::new(
            "E_OUTPUT_ALIAS",
            output,
            0,
            "output must not replace the policy file",
        ));
    }
    if let Ok(existing_output) = fs::canonicalize(output) {
        if existing_output.starts_with(old_root) || existing_output.starts_with(new_root) {
            return Err(GateError::new(
                "E_OUTPUT_ALIAS",
                output,
                0,
                "existing output resolves inside an immutable snapshot root",
            ));
        }
        if existing_output == canonical_policy {
            return Err(GateError::new(
                "E_OUTPUT_ALIAS",
                output,
                0,
                "existing output resolves to the policy file",
            ));
        }
    }
    Ok(())
}

fn validate_output_identities(
    output: &Path,
    policy: &Path,
    input_paths: &[PathBuf],
) -> Result<(), GateError> {
    let Ok(output_metadata) = fs::metadata(output) else {
        return Ok(());
    };
    for candidate in std::iter::once(policy).chain(input_paths.iter().map(PathBuf::as_path)) {
        let metadata = fs::metadata(candidate).map_err(|error| {
            GateError::new(
                "E_INPUT_CHANGED",
                candidate,
                0,
                format!("selected immutable input changed during evaluation: {error}"),
            )
        })?;
        if same_file_identity(&output_metadata, &metadata) {
            return Err(GateError::new(
                "E_OUTPUT_ALIAS",
                output,
                0,
                format!("output aliases selected input {}", candidate.display()),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file_identity(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    false
}

struct OwnedTemp {
    path: PathBuf,
    owned: bool,
}

impl Drop for OwnedTemp {
    fn drop(&mut self) {
        if self.owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn create_owned_temp(
    parent: &Path,
    output_name: &str,
    output: &Path,
) -> Result<(fs::File, OwnedTemp), GateError> {
    let nonce = if let Some(value) = env::var_os("DNS_GATE_TEMP_NONCE") {
        value
            .to_str()
            .ok_or_else(|| GateError::new("E_OUTPUT", output, 0, "test temp nonce is not UTF-8"))?
            .parse::<u128>()
            .map_err(|_| GateError::new("E_OUTPUT", output, 0, "test temp nonce is invalid"))?
    } else {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                GateError::new(
                    "E_OUTPUT",
                    output,
                    0,
                    format!("system clock cannot seed temporary filename: {error}"),
                )
            })?
            .as_nanos();
        let seed = (
            std::process::id(),
            now,
            TEMP_COUNTER.load(Ordering::Relaxed),
        );
        let high = std::collections::hash_map::RandomState::new().hash_one(seed);
        let low = std::collections::hash_map::RandomState::new().hash_one(seed);
        (u128::from(high) << 64) | u128::from(low)
    };
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    for attempt in 0..TEMP_CREATE_ATTEMPTS {
        let path = parent.join(format!(
            ".{output_name}.tmp-{}-{nonce:032x}-{sequence:016x}-{attempt:03}",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((file, OwnedTemp { path, owned: true })),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(GateError::new(
                    "E_OUTPUT",
                    &path,
                    0,
                    format!("cannot create exclusive temporary report: {error}"),
                ));
            }
        }
    }
    Err(GateError::new(
        "E_OUTPUT_TEMP_COLLISIONS",
        output,
        0,
        format!(
            "could not create an exclusive temporary report after {TEMP_CREATE_ATTEMPTS} attempts"
        ),
    ))
}
