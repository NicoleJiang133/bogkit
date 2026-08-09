use incremental_syntax_highlighting::{
    Edit, IncrementalLexer, LexResult, WorkCounters, full_lex, result_digest, validate_coverage,
};
use serde::Serialize;
use std::fs;
use std::io::{BufRead, BufReader};
use std::time::Instant;

const DOCUMENT_SEED: u64 = 0x6d61_7261_2d64_6f63;
const EDIT_SEED: u64 = 0x6d61_7261_2d65_6469;
const CORRECTNESS_DOCUMENTS: usize = 100;
const EDITS_PER_DOCUMENT: usize = 100;
const BENCHMARK_LINES: usize = 200_000;
const BENCHMARK_EDITS: usize = 2_000;
const MAX_INDEX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Serialize)]
struct BaselineSummary {
    document_bytes: usize,
    tokens: usize,
    elapsed_ms: u128,
    bytes_per_edit: usize,
    projected_bytes_for_2_000_edits: u64,
    digest: String,
}

#[derive(Debug, Serialize)]
struct CorrectnessSummary {
    documents: usize,
    edits: usize,
    document_seed: u64,
    edit_seed: u64,
    all_results_match_full_lexer: bool,
    all_token_ranges_cover_valid_utf8: bool,
    final_digest: String,
}

#[derive(Debug, Serialize)]
struct AdversarialSummary {
    named_cases: usize,
    accepted_edits: usize,
    invalid_edits: usize,
    all_results_match_full_lexer: bool,
    invalid_edits_left_state_unchanged: bool,
    final_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct StableBenchmarkSummary {
    document_seed: u64,
    edit_seed: u64,
    document_bytes: usize,
    document_lines: usize,
    edits: usize,
    localized_edits: usize,
    propagating_edits: usize,
    localized_at_most_500_lines: usize,
    localized_at_most_2_percent_bytes: usize,
    incremental_lines_relexed: u64,
    incremental_bytes_scanned: u64,
    full_relex_bytes: u64,
    max_retained_index_estimate_bytes: usize,
    canonical_result_and_counters_bytes: usize,
    final_result_digest: String,
    counter_digest: String,
}

#[derive(Debug, Serialize)]
struct BenchmarkSummary {
    stable: StableBenchmarkSummary,
    localized_at_most_500_lines_percent: f64,
    localized_at_most_2_percent_bytes_percent: f64,
    incremental_to_full_bytes_percent: f64,
    retained_index_estimate_mib: f64,
    retained_index_estimate_under_64_mib: bool,
    process_peak_memory_constraint_demonstrated: bool,
    oracle_checked_after_every_edit: bool,
    elapsed_ms: u128,
}

struct BenchmarkRun {
    summary: BenchmarkSummary,
    canonical_result_and_counters: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct ApplySummary {
    input_bytes: usize,
    edits: usize,
    checked_after_every_edit: bool,
    counters: WorkCounters,
    max_retained_index_estimate_bytes: usize,
    final_digest: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_string());
    match command.as_str() {
        "baseline" => print_json(&run_baseline()),
        "correctness" => print_json(&run_correctness()?),
        "adversarial" => print_json(&run_adversarial()?),
        "benchmark" => {
            let check = !args.any(|arg| arg == "--no-oracle");
            print_json(&run_benchmark(check)?.summary);
        }
        "reproducibility" => {
            let first = run_benchmark(false)?;
            let second = run_benchmark(false)?;
            let exact_serialized_result_and_counters_equal =
                first.canonical_result_and_counters.as_slice()
                    == second.canonical_result_and_counters.as_slice();
            let stable_measurement_fields_equal = first.summary.stable == second.summary.stable;
            if !exact_serialized_result_and_counters_equal || !stable_measurement_fields_equal {
                return Err(
                    "same seeds produced different canonical results, counters, or measurements"
                        .to_string(),
                );
            }
            print_json(&serde_json::json!({
                "exact_serialized_result_and_counters_equal": true,
                "stable_measurement_fields_equal": true,
                "canonical_serialized_bytes": first.canonical_result_and_counters.len(),
                "document_seed": DOCUMENT_SEED,
                "edit_seed": EDIT_SEED,
                "final_result_digest": first.summary.stable.final_result_digest,
                "counter_digest": first.summary.stable.counter_digest,
            }));
        }
        "apply" => {
            let document_path = args
                .next()
                .ok_or_else(|| "apply requires DOCUMENT and EDITS_JSONL paths".to_string())?;
            let edits_path = args
                .next()
                .ok_or_else(|| "apply requires DOCUMENT and EDITS_JSONL paths".to_string())?;
            let check = args.any(|arg| arg == "--check");
            print_json(&run_apply(&document_path, &edits_path, check)?);
        }
        _ => {
            return Err(
                "usage: incremental-syntax-highlighting <baseline|correctness|adversarial|benchmark [--no-oracle]|reproducibility|apply DOCUMENT EDITS_JSONL [--check]>"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn print_json(value: &impl Serialize) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("summary is serializable")
    );
}

fn run_baseline() -> BaselineSummary {
    let document = baseline_fixture(10 * 1024 * 1024);
    let started = Instant::now();
    let result = full_lex(&document);
    BaselineSummary {
        document_bytes: document.len(),
        tokens: result.tokens.len(),
        elapsed_ms: started.elapsed().as_millis(),
        bytes_per_edit: document.len(),
        projected_bytes_for_2_000_edits: document.len() as u64 * 2_000,
        digest: result_digest(&result, WorkCounters::default()),
    }
}

fn run_correctness() -> Result<CorrectnessSummary, String> {
    let mut document_rng = Rng::new(DOCUMENT_SEED);
    let mut edit_rng = Rng::new(EDIT_SEED);
    let mut digest_accumulator = 0xcbf29ce484222325u64;
    for document_index in 0..CORRECTNESS_DOCUMENTS {
        let document = correctness_document(&mut document_rng, document_index);
        let mut incremental = IncrementalLexer::new(document);
        compare_with_full(&incremental, document_index, 0)?;
        for edit_index in 0..EDITS_PER_DOCUMENT {
            let edit = generated_edit(incremental.document(), &mut edit_rng, edit_index);
            incremental
                .apply_edit(&edit)
                .map_err(|error| format!("generated valid edit rejected: {error}"))?;
            compare_with_full(&incremental, document_index, edit_index + 1)?;
        }
        digest_accumulator = mix_digest(
            digest_accumulator,
            &result_digest(&incremental.result(), incremental.counters()),
        );
    }
    Ok(CorrectnessSummary {
        documents: CORRECTNESS_DOCUMENTS,
        edits: CORRECTNESS_DOCUMENTS * EDITS_PER_DOCUMENT,
        document_seed: DOCUMENT_SEED,
        edit_seed: EDIT_SEED,
        all_results_match_full_lexer: true,
        all_token_ranges_cover_valid_utf8: true,
        final_digest: format!("{digest_accumulator:016x}"),
    })
}

fn run_adversarial() -> Result<AdversarialSummary, String> {
    let cases: Vec<(&str, &str, Vec<Edit>)> = vec![
        (
            "block delimiter insertion and deletion",
            "alpha xx beta\nstill normal\nend\n",
            vec![
                edit(6, 2, "/*"),
                edit(6, 2, "xx"),
                edit(16, 0, "*/"),
                edit(16, 2, ""),
            ],
        ),
        (
            "nested-looking block comment text",
            "/* outer /* inner */ tail\nnormal\n",
            vec![edit(9, 2, "zz"), edit(9, 2, "/*")],
        ),
        (
            "escaped quote insertion and deletion",
            "\"one \\\" two\" after\n",
            vec![edit(5, 1, ""), edit(5, 0, "\\")],
        ),
        (
            "empty replacement at byte zero and end",
            "start end",
            vec![edit(0, 1, ""), edit(8, 0, "!")],
        ),
        (
            "CRLF boundary",
            "one\r\ntwo\r\nthree",
            vec![edit(3, 2, "\n"), edit(3, 1, "\r\n")],
        ),
        (
            "multibyte text next to edit",
            "aλb\n雪x",
            vec![edit(3, 1, "q"), edit(8, 0, "🙂")],
        ),
        (
            "unterminated constructs",
            "head\n\"open\nstill",
            vec![edit(5, 1, "/*"), edit(5, 2, "\"")],
        ),
        (
            "newline insertion changes following state",
            "\"left right\"\ntail",
            vec![edit(6, 1, "\n"), edit(6, 1, " ")],
        ),
        (
            "cross-line UTF-8 replacement before final newline",
            "/λa\n\n",
            vec![edit(3, 2, "\r\n\"🙂🙂")],
        ),
    ];
    let named_cases = cases.len();
    let mut accepted_edits = 0;
    let mut digest_accumulator = 0xcbf29ce484222325u64;
    for (case_index, (name, document, edits)) in cases.into_iter().enumerate() {
        let mut incremental = IncrementalLexer::new(document.to_string());
        compare_with_full(&incremental, case_index, 0)
            .map_err(|error| format!("{name}: {error}"))?;
        for (edit_index, edit) in edits.iter().enumerate() {
            incremental
                .apply_edit(edit)
                .map_err(|error| format!("{name}: edit {}: {error}", edit_index + 1))?;
            accepted_edits += 1;
            compare_with_full(&incremental, case_index, edit_index + 1)
                .map_err(|error| format!("{name}: {error}"))?;
        }
        digest_accumulator = mix_digest(
            digest_accumulator,
            &result_digest(&incremental.result(), incremental.counters()),
        );
    }

    let mut invalid = IncrementalLexer::new("aλz\r\n".to_string());
    let before_document = invalid.document().to_string();
    let before_result = invalid.result();
    let before_counters = invalid.counters();
    let invalid_edits = [edit(2, 0, "x"), edit(1, 1, ""), edit(99, 0, "")];
    for candidate in &invalid_edits {
        if invalid.apply_edit(candidate).is_ok() {
            return Err(format!("invalid edit was accepted: {candidate:?}"));
        }
        if invalid.document() != before_document
            || invalid.result() != before_result
            || invalid.counters() != before_counters
        {
            return Err("rejected edit changed document or cache result".to_string());
        }
        compare_with_full(&invalid, usize::MAX, 0)?;
    }

    Ok(AdversarialSummary {
        named_cases,
        accepted_edits,
        invalid_edits: invalid_edits.len(),
        all_results_match_full_lexer: true,
        invalid_edits_left_state_unchanged: true,
        final_digest: format!("{digest_accumulator:016x}"),
    })
}

fn run_benchmark(check_with_oracle: bool) -> Result<BenchmarkRun, String> {
    let started = Instant::now();
    let fixture = benchmark_fixture();
    let original_bytes = fixture.document.len();
    let mut incremental = IncrementalLexer::new(fixture.document);
    if incremental.line_count() != BENCHMARK_LINES {
        return Err(format!(
            "large fixture has {} lines, expected {BENCHMARK_LINES}",
            incremental.line_count()
        ));
    }
    let mut rng = Rng::new(EDIT_SEED);
    let mut localized_edits = 0;
    let mut propagating_edits = 0;
    let mut localized_at_most_500_lines = 0;
    let mut localized_at_most_2_percent_bytes = 0;
    let mut toggle_open = false;
    let mut full_relex_bytes = 0u64;

    for edit_index in 0..BENCHMARK_EDITS {
        let is_propagating = edit_index % 20 == 19;
        let edit = if is_propagating {
            let replacement = if toggle_open { "zz" } else { "/*" };
            toggle_open = !toggle_open;
            edit(fixture.toggle_position, 2, replacement)
        } else {
            let mut line = 1 + rng.index(BENCHMARK_LINES - 2);
            if line == fixture.toggle_line || fixture.body_lengths[line] < 8 {
                line = (line + 7).min(BENCHMARK_LINES - 2);
            }
            let position = fixture.starts[line] + 6.min(fixture.body_lengths[line] - 1);
            let replacement = if edit_index % 2 == 0 { "q" } else { "r" };
            edit(position, 1, replacement)
        };
        let work = incremental
            .apply_edit(&edit)
            .map_err(|error| format!("benchmark edit {edit_index} rejected: {error}"))?;
        if is_propagating {
            propagating_edits += 1;
            if !work.scanned_to_eof {
                return Err(format!(
                    "propagating edit {edit_index} unexpectedly converged before EOF"
                ));
            }
        } else {
            localized_edits += 1;
            if work.lines_relexed <= 500 {
                localized_at_most_500_lines += 1;
            }
            if work.bytes_scanned * 100 <= incremental.document().len() * 2 {
                localized_at_most_2_percent_bytes += 1;
            }
        }
        full_relex_bytes += incremental.document().len() as u64;
        if check_with_oracle {
            compare_with_full(&incremental, 0, edit_index + 1)
                .map_err(|error| format!("large benchmark: {error}"))?;
        }
    }

    let result = incremental.result();
    validate_coverage(incremental.document(), &result)?;
    let counters = incremental.counters();
    let canonical_result_and_counters = canonical_result_and_counters_bytes(&result, counters)?;
    let stable = StableBenchmarkSummary {
        document_seed: DOCUMENT_SEED,
        edit_seed: EDIT_SEED,
        document_bytes: original_bytes,
        document_lines: BENCHMARK_LINES,
        edits: BENCHMARK_EDITS,
        localized_edits,
        propagating_edits,
        localized_at_most_500_lines,
        localized_at_most_2_percent_bytes,
        incremental_lines_relexed: counters.lines_relexed,
        incremental_bytes_scanned: counters.bytes_scanned,
        full_relex_bytes,
        max_retained_index_estimate_bytes: incremental.max_retained_index_estimate_bytes(),
        canonical_result_and_counters_bytes: canonical_result_and_counters.len(),
        final_result_digest: result_digest(&result, WorkCounters::default()),
        counter_digest: result_digest(&result, counters),
    };
    let localized_denominator = localized_edits as f64;
    Ok(BenchmarkRun {
        summary: BenchmarkSummary {
            localized_at_most_500_lines_percent: 100.0 * localized_at_most_500_lines as f64
                / localized_denominator,
            localized_at_most_2_percent_bytes_percent: 100.0
                * localized_at_most_2_percent_bytes as f64
                / localized_denominator,
            incremental_to_full_bytes_percent: 100.0 * counters.bytes_scanned as f64
                / full_relex_bytes as f64,
            retained_index_estimate_mib: incremental.max_retained_index_estimate_bytes() as f64
                / (1024.0 * 1024.0),
            retained_index_estimate_under_64_mib: incremental.max_retained_index_estimate_bytes()
                <= MAX_INDEX_BYTES,
            process_peak_memory_constraint_demonstrated: false,
            oracle_checked_after_every_edit: check_with_oracle,
            elapsed_ms: started.elapsed().as_millis(),
            stable,
        },
        canonical_result_and_counters,
    })
}

fn canonical_result_and_counters_bytes(
    result: &LexResult,
    counters: WorkCounters,
) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&(result, counters))
        .map_err(|error| format!("cannot serialize canonical reproducibility value: {error}"))
}

fn run_apply(document_path: &str, edits_path: &str, check: bool) -> Result<ApplySummary, String> {
    let document = fs::read_to_string(document_path)
        .map_err(|error| format!("cannot read {document_path}: {error}"))?;
    let input_bytes = document.len();
    let file =
        fs::File::open(edits_path).map_err(|error| format!("cannot read {edits_path}: {error}"))?;
    let mut incremental = IncrementalLexer::new(document);
    let mut edits = 0;
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|error| format!("cannot read edit line: {error}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let edit: Edit = serde_json::from_str(&line)
            .map_err(|error| format!("invalid JSONL at line {}: {error}", line_index + 1))?;
        incremental
            .apply_edit(&edit)
            .map_err(|error| format!("invalid edit at line {}: {error}", line_index + 1))?;
        edits += 1;
        if check {
            compare_with_full(&incremental, 0, edits)?;
        }
    }
    let result = incremental.result();
    validate_coverage(incremental.document(), &result)?;
    Ok(ApplySummary {
        input_bytes,
        edits,
        checked_after_every_edit: check,
        counters: incremental.counters(),
        max_retained_index_estimate_bytes: incremental.max_retained_index_estimate_bytes(),
        final_digest: result_digest(&result, incremental.counters()),
    })
}

fn compare_with_full(
    incremental: &IncrementalLexer,
    document_index: usize,
    edit_index: usize,
) -> Result<(), String> {
    let canonical_line_count = incremental
        .document()
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    if incremental.line_count() != canonical_line_count {
        return Err(format!(
            "line-cache partition mismatch in document {document_index}, after edit {edit_index}: cached {}, canonical {canonical_line_count}",
            incremental.line_count()
        ));
    }
    let actual = incremental.result();
    let expected = full_lex(incremental.document());
    validate_coverage(incremental.document(), &actual)?;
    validate_coverage(incremental.document(), &expected)?;
    if actual != expected {
        return Err(format!(
            "differential mismatch in document {document_index}, after edit {edit_index}"
        ));
    }
    Ok(())
}

fn correctness_document(rng: &mut Rng, document_index: usize) -> String {
    let mut document = String::new();
    let patterns = [
        "alpha_1 2048 ; ,",
        "# line comment with /* inert text",
        "\"string with \\\" escape\" tail",
        "/* block */ after",
        "λambda 雪 42",
        "adjacent/**/delimiters",
        "\"cross-line",
        "string end\"",
        "/* cross-line",
        "block end */",
        "",
    ];
    let line_count = 20 + rng.index(40);
    for line in 0..line_count {
        let pattern = patterns[(rng.next_u64() as usize + line + document_index) % patterns.len()];
        document.push_str(pattern);
        if line + 1 < line_count {
            if (line + document_index).is_multiple_of(7) {
                document.push_str("\r\n");
            } else {
                document.push('\n');
            }
        }
    }
    if document_index.is_multiple_of(3) {
        document.push_str("\n/* intentionally unterminated");
    } else if document_index % 3 == 1 {
        document.push_str("\n\"intentionally unterminated\\");
    }
    document
}

fn generated_edit(document: &str, rng: &mut Rng, edit_index: usize) -> Edit {
    let boundaries: Vec<usize> = (0..=document.len())
        .filter(|position| document.is_char_boundary(*position))
        .collect();
    let start_boundary = rng.index(boundaries.len());
    let max_advance = (boundaries.len() - 1 - start_boundary).min(8);
    let end_boundary = start_boundary + rng.index(max_advance + 1);
    let replacements = [
        "alpha",
        "7",
        ";",
        "",
        "λ",
        "雪",
        "/*",
        "*/",
        "\"",
        "\\",
        "\n",
        "\r\n",
        "\"x\\\"y\"",
        "# local",
    ];
    let replacement = replacements[(rng.next_u64() as usize + edit_index) % replacements.len()];
    Edit {
        start: boundaries[start_boundary],
        deleted_bytes: boundaries[end_boundary] - boundaries[start_boundary],
        replacement: replacement.to_string(),
    }
}

struct BenchmarkFixture {
    document: String,
    starts: Vec<usize>,
    body_lengths: Vec<usize>,
    toggle_line: usize,
    toggle_position: usize,
}

fn benchmark_fixture() -> BenchmarkFixture {
    let toggle_line = 120_001;
    let mut document = String::with_capacity(10 * 1024 * 1024);
    let mut starts = Vec::with_capacity(BENCHMARK_LINES);
    let mut body_lengths = Vec::with_capacity(BENCHMARK_LINES);
    for line in 0..BENCHMARK_LINES {
        starts.push(document.len());
        let ending = if line + 1 == BENCHMARK_LINES {
            ""
        } else if line % 97 == 0 {
            "\r\n"
        } else {
            "\n"
        };
        let body_target = 52 - ending.len();
        let body = if line == toggle_line {
            padded("zzpropagation_anchor", body_target, 'a')
        } else if line + 1 == BENCHMARK_LINES {
            padded("last_identifier_\"", body_target, 'u')
        } else if line % 1000 == 0 {
            String::new()
        } else if line < 100_000 {
            match line % 6 {
                0 => padded("generated_identifier_", body_target, 'a'),
                1 => padded("1234567890", body_target, '7'),
                2 => padded("# local comment ", body_target, 'c'),
                3 => padded("\"escaped \\\" quote ", body_target - 1, 's') + "\"",
                4 => padded("/* closed block ", body_target - 2, 'b') + "*/",
                _ => padded("λunicode_identifier_", body_target, 'm'),
            }
        } else if line % 3 == 0 {
            padded("generated_identifier_", body_target, 'g')
        } else if line % 3 == 1 {
            padded("9988776655", body_target, '4')
        } else {
            padded("# suffix comment ", body_target, 'h')
        };
        body_lengths.push(body.len());
        document.push_str(&body);
        document.push_str(ending);
    }
    let toggle_position = starts[toggle_line];
    assert_eq!(&document[toggle_position..toggle_position + 2], "zz");
    BenchmarkFixture {
        document,
        starts,
        body_lengths,
        toggle_line,
        toggle_position,
    }
}

fn padded(prefix: &str, target_bytes: usize, fill: char) -> String {
    assert!(prefix.len() <= target_bytes);
    let mut value = String::with_capacity(target_bytes);
    value.push_str(prefix);
    value.extend(std::iter::repeat_n(fill, target_bytes - prefix.len()));
    value
}

fn baseline_fixture(target_bytes: usize) -> String {
    let mut document = String::with_capacity(target_bytes);
    let line = "generated_identifier_012345678901234567890123456789\n";
    while document.len() + line.len() <= target_bytes {
        document.push_str(line);
    }
    document.extend(std::iter::repeat_n('x', target_bytes - document.len()));
    document
}

fn edit(start: usize, deleted_bytes: usize, replacement: &str) -> Edit {
    Edit {
        start,
        deleted_bytes,
        replacement: replacement.to_string(),
    }
}

fn mix_digest(mut accumulator: u64, value: &str) -> u64 {
    for byte in value.bytes() {
        accumulator ^= u64::from(byte);
        accumulator = accumulator.wrapping_mul(0x100000001b3);
    }
    accumulator
}

#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn index(&mut self, upper: usize) -> usize {
        assert!(upper > 0);
        self.next_u64() as usize % upper
    }
}

#[cfg(test)]
mod binary_tests {
    use super::*;

    #[test]
    fn canonical_reproducibility_bytes_compare_exact_values() {
        let result = full_lex("/λ\r\n\"🙂🙂\n");
        let counters = WorkCounters {
            edits: 1,
            lines_relexed: 3,
            bytes_scanned: 15,
        };
        let first = canonical_result_and_counters_bytes(&result, counters).unwrap();
        let second = canonical_result_and_counters_bytes(&result, counters).unwrap();
        assert_eq!(first, second);

        let changed = canonical_result_and_counters_bytes(
            &result,
            WorkCounters {
                bytes_scanned: 16,
                ..counters
            },
        )
        .unwrap();
        assert_ne!(first, changed);
    }
}
