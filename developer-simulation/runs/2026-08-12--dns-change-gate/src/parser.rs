use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Component, Path, PathBuf};

use crate::{GateError, Limits, ZonePolicy};

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct RecordKey {
    pub owner: String,
    pub record_type: String,
    pub rdata: String,
}

#[derive(Debug)]
pub struct ZoneData {
    pub records: BTreeMap<RecordKey, u32>,
    pub input_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
struct Token {
    raw: Vec<u8>,
    quoted: bool,
    offset: usize,
}

#[derive(Debug)]
struct LogicalRecord {
    tokens: Vec<Token>,
    leading_whitespace: bool,
    offset: usize,
}

struct ParseContext<'a> {
    root: &'a Path,
    limits: &'a Limits,
    total_records: &'a mut usize,
    zone_records: usize,
    records: BTreeMap<RecordKey, u32>,
    include_stack: Vec<PathBuf>,
    input_paths: Vec<PathBuf>,
}

/// Parses one selected zone from one snapshot root.
///
/// # Errors
/// Returns a stable, offset-bearing input diagnostic on malformed or unsafe input.
pub fn parse_zone(
    root: &Path,
    selected: &ZonePolicy,
    limits: &Limits,
    total_records: &mut usize,
) -> Result<ZoneData, GateError> {
    let mut context = ParseContext {
        root,
        limits,
        total_records,
        zone_records: 0,
        records: BTreeMap::new(),
        include_stack: Vec::new(),
        input_paths: Vec::new(),
    };
    let origin = selected.zone.clone();
    context.parse_file(root, &selected.relative_path, origin, None, None, 0)?;
    Ok(ZoneData {
        records: context.records,
        input_paths: context.input_paths,
    })
}

impl ParseContext<'_> {
    fn parse_file(
        &mut self,
        base: &Path,
        requested: &Path,
        mut origin: String,
        inherited_owner: Option<String>,
        inherited_ttl: Option<u32>,
        depth: usize,
    ) -> Result<(), GateError> {
        if depth > self.limits.max_include_depth {
            return Err(GateError::new(
                "E_INCLUDE_DEPTH",
                requested,
                0,
                format!("include depth exceeds {}", self.limits.max_include_depth),
            ));
        }
        let path = safe_path(self.root, base, requested)?;
        if !self.input_paths.contains(&path) {
            self.input_paths.push(path.clone());
        }
        if self.include_stack.contains(&path) {
            return Err(GateError::new(
                "E_INCLUDE_CYCLE",
                &path,
                0,
                "include cycle detected",
            ));
        }
        let bytes = fs::read(&path).map_err(|error| {
            GateError::new(
                "E_INCLUDE_MISSING",
                &path,
                0,
                format!("cannot read zone file: {error}"),
            )
        })?;
        let logical = tokenize(&path, &bytes)?;
        self.include_stack.push(path.clone());
        let mut default_ttl = inherited_ttl;
        let mut last_owner = inherited_owner;
        for record in logical {
            let result = if first_is(&record, b"$ORIGIN") {
                if record.tokens.len() == 2 {
                    origin = canonical_name(&record.tokens[1], &origin, &path)?;
                    Ok(())
                } else {
                    Err(at_record(
                        "E_DIRECTIVE",
                        &path,
                        &record,
                        "$ORIGIN requires one name",
                    ))
                }
            } else if first_is(&record, b"$TTL") {
                if record.tokens.len() == 2 {
                    default_ttl = Some(parse_ttl(&record.tokens[1], &path)?);
                    Ok(())
                } else {
                    Err(at_record(
                        "E_DIRECTIVE",
                        &path,
                        &record,
                        "$TTL requires one value",
                    ))
                }
            } else if first_is(&record, b"$INCLUDE") {
                self.parse_include(
                    &path,
                    &record,
                    &origin,
                    last_owner.clone(),
                    default_ttl,
                    depth,
                )
            } else if first_is(&record, b"$GENERATE") {
                self.parse_generate(&path, &record, &origin, &mut last_owner, default_ttl)
            } else if record
                .tokens
                .first()
                .is_some_and(|token| token.raw.first() == Some(&b'$'))
            {
                Err(at_record(
                    "E_DIRECTIVE",
                    &path,
                    &record,
                    "unsupported directive",
                ))
            } else {
                self.parse_resource(&path, &record, &origin, &mut last_owner, default_ttl)
            };
            if let Err(error) = result {
                self.include_stack.pop();
                return Err(error);
            }
        }
        self.include_stack.pop();
        Ok(())
    }

    fn parse_include(
        &mut self,
        including: &Path,
        record: &LogicalRecord,
        origin: &str,
        owner: Option<String>,
        default_ttl: Option<u32>,
        depth: usize,
    ) -> Result<(), GateError> {
        if !(2..=3).contains(&record.tokens.len()) {
            return Err(at_record(
                "E_DIRECTIVE",
                including,
                record,
                "$INCLUDE requires path and optional origin",
            ));
        }
        let include_text = ascii(&record.tokens[1], including)?;
        let include_path = PathBuf::from(include_text);
        let include_origin = if record.tokens.len() == 3 {
            canonical_name(&record.tokens[2], origin, including)?
        } else {
            origin.to_string()
        };
        let base = including.parent().unwrap_or(self.root);
        self.parse_file(
            base,
            &include_path,
            include_origin,
            owner,
            default_ttl,
            depth + 1,
        )
    }

    fn parse_generate(
        &mut self,
        path: &Path,
        record: &LogicalRecord,
        origin: &str,
        last_owner: &mut Option<String>,
        default_ttl: Option<u32>,
    ) -> Result<(), GateError> {
        if record.tokens.len() < 5 {
            return Err(at_record(
                "E_GENERATE",
                path,
                record,
                "$GENERATE is incomplete",
            ));
        }
        let (first_value, last_value, stride) = parse_range(&record.tokens[1], path)?;
        let span = u64::from(last_value)
            .checked_sub(u64::from(first_value))
            .ok_or_else(|| at_record("E_GENERATE", path, record, "range underflow"))?;
        let count = span
            .checked_div(u64::from(stride))
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| {
                at_record(
                    "E_GENERATE_LIMIT",
                    path,
                    record,
                    "range cardinality overflow",
                )
            })?;
        let count_usize = usize::try_from(count).map_err(|_| {
            at_record(
                "E_GENERATE_LIMIT",
                path,
                record,
                "range size exceeds platform width",
            )
        })?;
        if count_usize > self.limits.max_generate_records {
            return Err(at_record(
                "E_GENERATE_LIMIT",
                path,
                record,
                format!("directive expands to {count_usize} records"),
            ));
        }
        let mut substitution_seen = false;
        for token in &record.tokens[2..] {
            for byte in &token.raw {
                if *byte == b'$' {
                    if substitution_seen {
                        return Err(at_record(
                            "E_GENERATE_SUBSTITUTION",
                            path,
                            record,
                            "exactly one simple substitution token is required",
                        ));
                    }
                    substitution_seen = true;
                }
            }
        }
        if !substitution_seen {
            return Err(at_record(
                "E_GENERATE_SUBSTITUTION",
                path,
                record,
                "exactly one simple substitution token is required",
            ));
        }
        let mut value = first_value;
        for _ in 0..count_usize {
            let replacement = value.to_string();
            let tokens = record.tokens[2..]
                .iter()
                .map(|token| Token {
                    raw: replace_dollar(&token.raw, replacement.as_bytes()),
                    quoted: token.quoted,
                    offset: token.offset,
                })
                .collect();
            let generated = LogicalRecord {
                tokens,
                leading_whitespace: false,
                offset: record.offset,
            };
            self.parse_resource(path, &generated, origin, last_owner, default_ttl)?;
            value = value.checked_add(stride).unwrap_or(last_value);
        }
        Ok(())
    }

    fn parse_resource(
        &mut self,
        path: &Path,
        record: &LogicalRecord,
        origin: &str,
        last_owner: &mut Option<String>,
        default_ttl: Option<u32>,
    ) -> Result<(), GateError> {
        if record.tokens.is_empty() {
            return Ok(());
        }
        let mut index = 0;
        let owner = if record.leading_whitespace {
            last_owner.clone().ok_or_else(|| {
                at_record("E_OWNER", path, record, "omitted owner has no predecessor")
            })?
        } else {
            let owner = canonical_name(&record.tokens[0], origin, path)?;
            index += 1;
            *last_owner = Some(owner.clone());
            owner
        };
        let mut ttl = None;
        let mut class_seen = false;
        let record_type;
        loop {
            let Some(token) = record.tokens.get(index) else {
                return Err(at_record("E_FIELD", path, record, "missing record type"));
            };
            let upper = ascii(token, path)?.to_ascii_uppercase();
            if is_record_type(&upper) {
                record_type = upper;
                index += 1;
                break;
            }
            if upper == "IN" && !class_seen {
                class_seen = true;
                index += 1;
            } else if ttl.is_none() {
                ttl = Some(parse_ttl(token, path).map_err(|_| {
                    GateError::new(
                        "E_FIELD",
                        path,
                        token.offset as u64,
                        "unknown mandatory field before record type",
                    )
                })?);
                index += 1;
            } else {
                return Err(GateError::new(
                    "E_FIELD",
                    path,
                    token.offset as u64,
                    "unknown mandatory field before record type",
                ));
            }
        }
        let ttl = ttl.or(default_ttl).ok_or_else(|| {
            at_record(
                "E_TTL",
                path,
                record,
                "record has no TTL and no $TTL default",
            )
        })?;
        let rdata = canonical_rdata(&record_type, &record.tokens[index..], origin, path)?;
        self.add_record(
            path,
            record,
            RecordKey {
                owner,
                record_type,
                rdata,
            },
            ttl,
        )
    }

    fn add_record(
        &mut self,
        path: &Path,
        record: &LogicalRecord,
        key: RecordKey,
        ttl: u32,
    ) -> Result<(), GateError> {
        self.zone_records = self.zone_records.checked_add(1).ok_or_else(|| {
            at_record("E_ZONE_LIMIT", path, record, "zone record counter overflow")
        })?;
        *self.total_records = self.total_records.checked_add(1).ok_or_else(|| {
            at_record(
                "E_TOTAL_LIMIT",
                path,
                record,
                "total record counter overflow",
            )
        })?;
        if self.zone_records > self.limits.max_records_per_zone {
            return Err(at_record(
                "E_ZONE_LIMIT",
                path,
                record,
                "per-zone expansion limit exceeded",
            ));
        }
        if *self.total_records > self.limits.max_records_total {
            return Err(at_record(
                "E_TOTAL_LIMIT",
                path,
                record,
                "total expansion limit exceeded",
            ));
        }
        if let Some(existing) = self.records.get(&key) {
            if *existing != ttl {
                return Err(at_record(
                    "E_DUP_TTL",
                    path,
                    record,
                    "duplicate canonical record has conflicting TTL",
                ));
            }
            return Ok(());
        }
        self.records.insert(key, ttl);
        Ok(())
    }
}

fn safe_path(root: &Path, base: &Path, requested: &Path) -> Result<PathBuf, GateError> {
    if requested.is_absolute() {
        return Err(GateError::new(
            "E_INCLUDE_ABSOLUTE",
            requested,
            0,
            "absolute paths are forbidden",
        ));
    }
    let mut lexical = base.to_path_buf();
    for component in requested.components() {
        match component {
            Component::Normal(part) => lexical.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !lexical.pop() || !lexical.starts_with(root) {
                    return Err(GateError::new(
                        "E_INCLUDE_ESCAPE",
                        requested,
                        0,
                        "path escapes snapshot root",
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(GateError::new(
                    "E_INCLUDE_ABSOLUTE",
                    requested,
                    0,
                    "absolute paths are forbidden",
                ));
            }
        }
    }
    if !lexical.starts_with(root) {
        return Err(GateError::new(
            "E_INCLUDE_ESCAPE",
            requested,
            0,
            "path escapes snapshot root",
        ));
    }
    let canonical = fs::canonicalize(&lexical).map_err(|error| {
        GateError::new(
            "E_INCLUDE_MISSING",
            &lexical,
            0,
            format!("cannot resolve file: {error}"),
        )
    })?;
    if !canonical.starts_with(root) {
        return Err(GateError::new(
            "E_INCLUDE_SYMLINK",
            &lexical,
            0,
            "resolved path escapes snapshot root",
        ));
    }
    if !canonical.is_file() {
        return Err(GateError::new(
            "E_INCLUDE_MISSING",
            &canonical,
            0,
            "zone input is not a regular file",
        ));
    }
    Ok(canonical)
}

#[expect(
    clippy::too_many_lines,
    reason = "the byte tokenizer is one explicit state machine so offsets and transitions stay auditable"
)]
fn tokenize(path: &Path, bytes: &[u8]) -> Result<Vec<LogicalRecord>, GateError> {
    let mut records = Vec::new();
    let mut tokens = Vec::new();
    let mut token = Vec::new();
    let mut token_start = 0;
    let mut token_quoted = false;
    let mut in_quote = false;
    let mut in_comment = false;
    let mut parentheses = 0_u32;
    let mut record_offset = 0;
    let mut leading_whitespace = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_comment {
            if byte == b'\n' {
                in_comment = false;
                if parentheses == 0 {
                    finish_record(
                        &mut records,
                        &mut tokens,
                        &mut token,
                        token_start,
                        &mut token_quoted,
                        leading_whitespace,
                        record_offset,
                    );
                    leading_whitespace = false;
                }
            }
            index += 1;
            continue;
        }
        if byte > 0x7f {
            return Err(GateError::new(
                "E_RAW_NON_ASCII",
                path,
                index as u64,
                "raw non-ASCII input byte",
            ));
        }
        if in_quote {
            if byte == b'\\' {
                let consumed = copy_escape(path, bytes, index, &mut token)?;
                index += consumed;
                continue;
            }
            if byte == b'"' {
                in_quote = false;
            } else if byte == b'\n' {
                return Err(GateError::new(
                    "E_UNTERMINATED_QUOTE",
                    path,
                    token_start as u64,
                    "quoted token crosses newline",
                ));
            } else {
                token.push(byte);
            }
            index += 1;
            continue;
        }
        match byte {
            b';' => {
                finish_token(&mut tokens, &mut token, token_start, &mut token_quoted);
                in_comment = true;
            }
            b'"' => {
                if token.is_empty() {
                    token_start = index;
                    token_quoted = true;
                    if tokens.is_empty() {
                        record_offset = index;
                    }
                }
                in_quote = true;
            }
            b'\\' => {
                if token.is_empty() {
                    token_start = index;
                    if tokens.is_empty() {
                        record_offset = index;
                    }
                }
                let consumed = copy_escape(path, bytes, index, &mut token)?;
                index += consumed;
                continue;
            }
            b'(' => {
                finish_token(&mut tokens, &mut token, token_start, &mut token_quoted);
                parentheses = parentheses.checked_add(1).ok_or_else(|| {
                    GateError::new(
                        "E_PAREN_DEPTH",
                        path,
                        index as u64,
                        "parenthesis depth overflow",
                    )
                })?;
            }
            b')' => {
                finish_token(&mut tokens, &mut token, token_start, &mut token_quoted);
                if parentheses == 0 {
                    return Err(GateError::new(
                        "E_UNEXPECTED_PAREN",
                        path,
                        index as u64,
                        "unexpected closing parenthesis",
                    ));
                }
                parentheses -= 1;
            }
            b'\n' => {
                finish_token(&mut tokens, &mut token, token_start, &mut token_quoted);
                if parentheses == 0 {
                    finish_record(
                        &mut records,
                        &mut tokens,
                        &mut token,
                        token_start,
                        &mut token_quoted,
                        leading_whitespace,
                        record_offset,
                    );
                    leading_whitespace = false;
                }
            }
            b' ' | b'\t' | b'\r' => {
                if tokens.is_empty() && token.is_empty() && parentheses == 0 {
                    leading_whitespace = true;
                }
                finish_token(&mut tokens, &mut token, token_start, &mut token_quoted);
            }
            _ => {
                if token.is_empty() {
                    token_start = index;
                    token_quoted = false;
                    if tokens.is_empty() {
                        record_offset = index;
                    }
                }
                token.push(byte);
            }
        }
        index += 1;
    }
    if in_quote {
        return Err(GateError::new(
            "E_UNTERMINATED_QUOTE",
            path,
            token_start as u64,
            "unterminated quoted token",
        ));
    }
    if parentheses != 0 {
        return Err(GateError::new(
            "E_UNTERMINATED_PAREN",
            path,
            record_offset as u64,
            "unterminated parenthesized record",
        ));
    }
    finish_token(&mut tokens, &mut token, token_start, &mut token_quoted);
    finish_record(
        &mut records,
        &mut tokens,
        &mut token,
        token_start,
        &mut token_quoted,
        leading_whitespace,
        record_offset,
    );
    Ok(records)
}

fn copy_escape(
    path: &Path,
    bytes: &[u8],
    index: usize,
    token: &mut Vec<u8>,
) -> Result<usize, GateError> {
    if index + 1 >= bytes.len() {
        return Err(GateError::new(
            "E_ESCAPE",
            path,
            index as u64,
            "trailing escape",
        ));
    }
    token.push(b'\\');
    if index + 3 < bytes.len() && bytes[index + 1..index + 4].iter().all(u8::is_ascii_digit) {
        token.extend_from_slice(&bytes[index + 1..index + 4]);
        Ok(4)
    } else {
        token.push(bytes[index + 1]);
        Ok(2)
    }
}

fn finish_token(tokens: &mut Vec<Token>, token: &mut Vec<u8>, start: usize, quoted: &mut bool) {
    if !token.is_empty() || *quoted {
        tokens.push(Token {
            raw: std::mem::take(token),
            quoted: *quoted,
            offset: start,
        });
    }
    *quoted = false;
}

fn finish_record(
    records: &mut Vec<LogicalRecord>,
    tokens: &mut Vec<Token>,
    token: &mut Vec<u8>,
    token_start: usize,
    token_quoted: &mut bool,
    leading_whitespace: bool,
    record_offset: usize,
) {
    finish_token(tokens, token, token_start, token_quoted);
    if !tokens.is_empty() {
        records.push(LogicalRecord {
            tokens: std::mem::take(tokens),
            leading_whitespace,
            offset: record_offset,
        });
    }
}

fn first_is(record: &LogicalRecord, expected: &[u8]) -> bool {
    record
        .tokens
        .first()
        .is_some_and(|token| token.raw.eq_ignore_ascii_case(expected))
}

fn at_record(
    code: &'static str,
    path: &Path,
    record: &LogicalRecord,
    detail: impl Into<String>,
) -> GateError {
    GateError::new(code, path, record.offset as u64, detail)
}

fn ascii(token: &Token, path: &Path) -> Result<String, GateError> {
    String::from_utf8(token.raw.clone()).map_err(|_| {
        GateError::new(
            "E_RAW_NON_ASCII",
            path,
            token.offset as u64,
            "token is not ASCII",
        )
    })
}

fn parse_ttl(token: &Token, path: &Path) -> Result<u32, GateError> {
    let text = ascii(token, path)?;
    if text.is_empty() {
        return Err(GateError::new(
            "E_TTL",
            path,
            token.offset as u64,
            "empty TTL",
        ));
    }
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut total = 0_u64;
    while index < bytes.len() {
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if start == index {
            return Err(GateError::new(
                "E_TTL",
                path,
                token.offset as u64,
                "invalid TTL",
            ));
        }
        let number = text[start..index].parse::<u64>().map_err(|_| {
            GateError::new("E_TTL", path, token.offset as u64, "TTL number overflow")
        })?;
        let multiplier = if index == bytes.len() {
            1
        } else {
            let unit = bytes[index].to_ascii_lowercase();
            index += 1;
            match unit {
                b'w' => 604_800,
                b'd' => 86_400,
                b'h' => 3_600,
                b'm' => 60,
                b's' => 1,
                _ => {
                    return Err(GateError::new(
                        "E_TTL",
                        path,
                        token.offset as u64,
                        "invalid TTL unit",
                    ));
                }
            }
        };
        total = total
            .checked_add(number.checked_mul(multiplier).ok_or_else(|| {
                GateError::new("E_TTL", path, token.offset as u64, "TTL overflow")
            })?)
            .ok_or_else(|| GateError::new("E_TTL", path, token.offset as u64, "TTL overflow"))?;
    }
    u32::try_from(total)
        .map_err(|_| GateError::new("E_TTL", path, token.offset as u64, "TTL exceeds u32"))
}

fn canonical_name(token: &Token, origin: &str, path: &Path) -> Result<String, GateError> {
    let mut labels: Vec<Vec<u8>> = vec![Vec::new()];
    let mut index = 0;
    let mut absolute = false;
    while index < token.raw.len() {
        match token.raw[index] {
            b'.' => {
                if labels.last().is_some_and(Vec::is_empty) {
                    return Err(GateError::new(
                        "E_NAME",
                        path,
                        (token.offset + index) as u64,
                        "empty label",
                    ));
                }
                labels.push(Vec::new());
                index += 1;
                if index == token.raw.len() {
                    absolute = true;
                    labels.pop();
                }
            }
            b'\\' => {
                let (byte, consumed) = decode_one_escape(&token.raw, index, path, token.offset)?;
                labels.last_mut().expect("one label").push(byte);
                index += consumed;
            }
            byte => {
                labels
                    .last_mut()
                    .expect("one label")
                    .push(byte.to_ascii_lowercase());
                index += 1;
            }
        }
    }
    if token.raw == b"@" {
        return Ok(origin.to_string());
    }
    if labels
        .iter()
        .any(|label| label.is_empty() || label.len() > 63)
    {
        return Err(GateError::new(
            "E_LABEL_LENGTH",
            path,
            token.offset as u64,
            "label length is outside 1..=63",
        ));
    }
    let local_wire_length = labels.iter().map(|label| label.len() + 1).sum::<usize>();
    let wire_length = if absolute {
        local_wire_length + 1
    } else {
        local_wire_length
            .checked_add(canonical_wire_length(origin).ok_or_else(|| {
                GateError::new(
                    "E_NAME",
                    path,
                    token.offset as u64,
                    "internal origin is not canonical",
                )
            })?)
            .ok_or_else(|| {
                GateError::new(
                    "E_NAME_LENGTH",
                    path,
                    token.offset as u64,
                    "wire name length overflow",
                )
            })?
    };
    if wire_length > 255 {
        return Err(GateError::new(
            "E_NAME_LENGTH",
            path,
            token.offset as u64,
            "wire name exceeds 255 bytes",
        ));
    }
    let mut rendered = labels
        .iter()
        .map(|label| render_label(label))
        .collect::<Vec<_>>()
        .join(".");
    if absolute {
        rendered.push('.');
    } else {
        rendered.push('.');
        rendered.push_str(origin);
    }
    Ok(rendered)
}

fn canonical_wire_length(name: &str) -> Option<usize> {
    if !name.ends_with('.') {
        return None;
    }
    let mut total = 1_usize;
    for label in name[..name.len() - 1].split('.') {
        if label.is_empty() {
            return None;
        }
        let bytes = label.as_bytes();
        let mut octets = 0_usize;
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'\\' {
                if index + 3 >= bytes.len()
                    || !bytes[index + 1..index + 4].iter().all(u8::is_ascii_digit)
                {
                    return None;
                }
                index += 4;
            } else {
                index += 1;
            }
            octets = octets.checked_add(1)?;
        }
        total = total.checked_add(octets.checked_add(1)?)?;
    }
    Some(total)
}

fn render_label(label: &[u8]) -> String {
    let mut output = String::new();
    for byte in label {
        let lowered = byte.to_ascii_lowercase();
        if lowered.is_ascii_alphanumeric() || matches!(lowered, b'-' | b'_') {
            output.push(char::from(lowered));
        } else {
            write!(output, "\\{lowered:03}").expect("write to string");
        }
    }
    output
}

fn decode_one_escape(
    raw: &[u8],
    index: usize,
    path: &Path,
    token_offset: usize,
) -> Result<(u8, usize), GateError> {
    if index + 1 >= raw.len() {
        return Err(GateError::new(
            "E_ESCAPE",
            path,
            (token_offset + index) as u64,
            "trailing escape",
        ));
    }
    if index + 3 < raw.len() && raw[index + 1..index + 4].iter().all(u8::is_ascii_digit) {
        let value = u16::from(raw[index + 1] - b'0') * 100
            + u16::from(raw[index + 2] - b'0') * 10
            + u16::from(raw[index + 3] - b'0');
        let byte = u8::try_from(value).map_err(|_| {
            GateError::new(
                "E_ESCAPE",
                path,
                (token_offset + index) as u64,
                "decimal escape exceeds 255",
            )
        })?;
        Ok((byte, 4))
    } else {
        Ok((raw[index + 1], 2))
    }
}

fn canonical_rdata(
    record_type: &str,
    tokens: &[Token],
    origin: &str,
    path: &Path,
) -> Result<String, GateError> {
    match record_type {
        "A" => one(tokens, path)?
            .parse::<Ipv4Addr>()
            .map(|value| value.to_string())
            .map_err(|_| data_error(path, tokens, "invalid IPv4 address")),
        "AAAA" => one(tokens, path)?
            .parse::<Ipv6Addr>()
            .map(|value| value.to_string())
            .map_err(|_| data_error(path, tokens, "invalid IPv6 address")),
        "NS" | "CNAME" => {
            require_count(tokens, 1, path)?;
            canonical_name(&tokens[0], origin, path)
        }
        "MX" => {
            require_count(tokens, 2, path)?;
            Ok(format!(
                "{} {}",
                parse_u16(&tokens[0], path)?,
                canonical_name(&tokens[1], origin, path)?
            ))
        }
        "SRV" => {
            require_count(tokens, 4, path)?;
            Ok(format!(
                "{} {} {} {}",
                parse_u16(&tokens[0], path)?,
                parse_u16(&tokens[1], path)?,
                parse_u16(&tokens[2], path)?,
                canonical_name(&tokens[3], origin, path)?
            ))
        }
        "CAA" => {
            require_count(tokens, 3, path)?;
            Ok(format!(
                "{} {} {}",
                parse_u8(&tokens[0], path)?,
                ascii(&tokens[1], path)?.to_ascii_lowercase(),
                render_octets(&decode_token(&tokens[2], path)?)
            ))
        }
        "DS" => {
            require_count(tokens, 4, path)?;
            let digest = ascii(&tokens[3], path)?;
            if digest.is_empty() || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(data_error(path, tokens, "invalid DS digest"));
            }
            Ok(format!(
                "{} {} {} {}",
                parse_u16(&tokens[0], path)?,
                parse_u8(&tokens[1], path)?,
                parse_u8(&tokens[2], path)?,
                digest.to_ascii_uppercase()
            ))
        }
        "TXT" => {
            if tokens.is_empty() || tokens.iter().any(|token| !token.quoted) {
                return Err(data_error(
                    path,
                    tokens,
                    "TXT requires one or more quoted chunks",
                ));
            }
            let mut chunks = Vec::with_capacity(tokens.len());
            for token in tokens {
                let decoded = decode_token(token, path)?;
                if decoded.len() > 255 {
                    return Err(GateError::new(
                        "E_RDATA",
                        path,
                        token.offset as u64,
                        "TXT chunk exceeds 255 bytes",
                    ));
                }
                chunks.push(render_octets(&decoded));
            }
            Ok(chunks.join("|"))
        }
        "SOA" => {
            require_count(tokens, 7, path)?;
            Ok(format!(
                "{} {} {} {} {} {} {}",
                canonical_name(&tokens[0], origin, path)?,
                canonical_name(&tokens[1], origin, path)?,
                parse_u32(&tokens[2], path)?,
                parse_ttl(&tokens[3], path)?,
                parse_ttl(&tokens[4], path)?,
                parse_ttl(&tokens[5], path)?,
                parse_ttl(&tokens[6], path)?
            ))
        }
        _ => Err(data_error(path, tokens, "unsupported record type")),
    }
}

fn one(tokens: &[Token], path: &Path) -> Result<String, GateError> {
    require_count(tokens, 1, path)?;
    ascii(&tokens[0], path)
}

fn require_count(tokens: &[Token], expected: usize, path: &Path) -> Result<(), GateError> {
    if tokens.len() == expected {
        Ok(())
    } else {
        Err(data_error(
            path,
            tokens,
            format!("expected {expected} RDATA fields, found {}", tokens.len()),
        ))
    }
}

fn data_error(path: &Path, tokens: &[Token], detail: impl Into<String>) -> GateError {
    GateError::new(
        "E_RDATA",
        path,
        tokens.first().map_or(0, |token| token.offset as u64),
        detail,
    )
}

fn parse_u8(token: &Token, path: &Path) -> Result<u8, GateError> {
    ascii(token, path)?.parse::<u8>().map_err(|_| {
        GateError::new(
            "E_INTEGER_WIDTH",
            path,
            token.offset as u64,
            "integer exceeds u8",
        )
    })
}

fn parse_u16(token: &Token, path: &Path) -> Result<u16, GateError> {
    ascii(token, path)?.parse::<u16>().map_err(|_| {
        GateError::new(
            "E_INTEGER_WIDTH",
            path,
            token.offset as u64,
            "integer exceeds u16",
        )
    })
}

fn parse_u32(token: &Token, path: &Path) -> Result<u32, GateError> {
    ascii(token, path)?.parse::<u32>().map_err(|_| {
        GateError::new(
            "E_INTEGER_WIDTH",
            path,
            token.offset as u64,
            "integer exceeds u32",
        )
    })
}

fn decode_token(token: &Token, path: &Path) -> Result<Vec<u8>, GateError> {
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < token.raw.len() {
        if token.raw[index] == b'\\' {
            let (byte, consumed) = decode_one_escape(&token.raw, index, path, token.offset)?;
            decoded.push(byte);
            index += consumed;
        } else {
            decoded.push(token.raw[index]);
            index += 1;
        }
    }
    Ok(decoded)
}

fn render_octets(bytes: &[u8]) -> String {
    let mut output = String::new();
    for byte in bytes {
        if (0x21..=0x7e).contains(byte) && !matches!(*byte, b'\\' | b'|' | b'"') {
            output.push(char::from(*byte));
        } else {
            write!(output, "\\{byte:03}").expect("write to string");
        }
    }
    output
}

fn is_record_type(value: &str) -> bool {
    matches!(
        value,
        "SOA" | "NS" | "A" | "AAAA" | "CNAME" | "MX" | "TXT" | "SRV" | "CAA" | "DS"
    )
}

fn parse_range(token: &Token, path: &Path) -> Result<(u32, u32, u32), GateError> {
    let text = ascii(token, path)?;
    let (range, stride_text) = text
        .split_once('/')
        .map_or((text.as_str(), None), |(left, right)| (left, Some(right)));
    let (first_text, last_text) = range.split_once('-').ok_or_else(|| {
        GateError::new(
            "E_GENERATE",
            path,
            token.offset as u64,
            "range must be start-stop[/step]",
        )
    })?;
    let first_value = first_text.parse::<u32>().map_err(|_| {
        GateError::new(
            "E_GENERATE",
            path,
            token.offset as u64,
            "invalid range start",
        )
    })?;
    let last_value = last_text.parse::<u32>().map_err(|_| {
        GateError::new(
            "E_GENERATE",
            path,
            token.offset as u64,
            "invalid range stop",
        )
    })?;
    let stride = stride_text.map_or(Ok(1), str::parse::<u32>).map_err(|_| {
        GateError::new(
            "E_GENERATE",
            path,
            token.offset as u64,
            "invalid range step",
        )
    })?;
    if first_value > last_value || stride == 0 {
        return Err(GateError::new(
            "E_GENERATE",
            path,
            token.offset as u64,
            "range must be increasing with positive step",
        ));
    }
    Ok((first_value, last_value, stride))
}

fn replace_dollar(raw: &[u8], replacement: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(raw.len() + replacement.len());
    for byte in raw {
        if *byte == b'$' {
            output.extend_from_slice(replacement);
        } else {
            output.push(*byte);
        }
    }
    output
}
