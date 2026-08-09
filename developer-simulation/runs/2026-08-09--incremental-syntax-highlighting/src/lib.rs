use serde::{Deserialize, Serialize};
use std::fmt;
use std::mem::size_of;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TokenKind {
    Identifier,
    Number,
    Punctuation,
    Whitespace,
    LineComment,
    String,
    BlockComment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DiagnosticKind {
    UnterminatedString,
    UnterminatedBlockComment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LexState {
    Normal,
    String { escaped: bool, start: usize },
    BlockComment { start: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LexResult {
    pub tokens: Vec<Token>,
    pub final_state: LexState,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct WorkCounters {
    pub edits: u64,
    pub lines_relexed: u64,
    pub bytes_scanned: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EditWork {
    pub lines_relexed: usize,
    pub bytes_scanned: usize,
    pub converged: bool,
    pub scanned_to_eof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    OutOfBounds,
    NotCharBoundary,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds => f.write_str("edit range exceeds document bounds"),
            Self::NotCharBoundary => f.write_str("edit range splits a UTF-8 scalar"),
        }
    }
}

impl std::error::Error for EditError {}

#[derive(Debug, Clone, Deserialize)]
pub struct Edit {
    pub start: usize,
    #[serde(alias = "delete")]
    pub deleted_bytes: usize,
    #[serde(alias = "insert")]
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RelativeToken {
    kind: TokenKind,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
struct CachedLine {
    content: Box<str>,
    fingerprint: u64,
    incoming: LexState,
    outgoing: LexState,
    tokens: Vec<RelativeToken>,
    valid: bool,
}

impl CachedLine {
    fn invalid(content: &str) -> Self {
        Self {
            content: content.into(),
            fingerprint: fingerprint(content.as_bytes()),
            incoming: LexState::Normal,
            outgoing: LexState::Normal,
            tokens: Vec::new(),
            valid: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IncrementalLexer {
    document: String,
    lines: Vec<CachedLine>,
    starts: Vec<usize>,
    counters: WorkCounters,
    max_retained_index_estimate_bytes: usize,
}

#[derive(Debug, Clone, Copy)]
struct EditTransform {
    start: usize,
    old_end: usize,
    delta: isize,
}

pub fn full_lex(document: &str) -> LexResult {
    let mut state = LexState::Normal;
    let mut tokens = Vec::new();
    for (line_start, line) in line_slices(document) {
        let (relative, outgoing) = scan_line(line, line_start, state);
        tokens.extend(relative.into_iter().map(|token| Token {
            kind: token.kind,
            start: line_start + token.start,
            end: line_start + token.end,
        }));
        state = outgoing;
    }
    LexResult {
        diagnostics: diagnostics(state, document.len()),
        final_state: state,
        tokens,
    }
}

impl IncrementalLexer {
    pub fn new(document: String) -> Self {
        let mut lines: Vec<_> = line_slices(&document)
            .map(|(_, line)| CachedLine::invalid(line))
            .collect();
        let starts = starts_for(&lines);
        let mut state = LexState::Normal;
        for (index, line) in lines.iter_mut().enumerate() {
            let (tokens, outgoing) = scan_line(&line.content, starts[index], state);
            line.incoming = state;
            line.outgoing = outgoing;
            line.tokens = tokens;
            line.valid = true;
            state = outgoing;
        }
        let mut this = Self {
            document,
            lines,
            starts,
            counters: WorkCounters::default(),
            max_retained_index_estimate_bytes: 0,
        };
        this.max_retained_index_estimate_bytes = this.retained_index_estimate_bytes();
        this
    }

    pub fn document(&self) -> &str {
        &self.document
    }

    pub fn counters(&self) -> WorkCounters {
        self.counters
    }

    pub fn max_retained_index_estimate_bytes(&self) -> usize {
        self.max_retained_index_estimate_bytes
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn result(&self) -> LexResult {
        let mut tokens = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            let line_start = self.starts[index];
            tokens.extend(line.tokens.iter().map(|token| Token {
                kind: token.kind,
                start: line_start + token.start,
                end: line_start + token.end,
            }));
        }
        let state = self
            .lines
            .last()
            .map_or(LexState::Normal, |line| line.outgoing);
        LexResult {
            diagnostics: diagnostics(state, self.document.len()),
            final_state: state,
            tokens,
        }
    }

    pub fn apply_edit(&mut self, edit: &Edit) -> Result<EditWork, EditError> {
        let old_end = edit
            .start
            .checked_add(edit.deleted_bytes)
            .ok_or(EditError::OutOfBounds)?;
        if edit.start > self.document.len() || old_end > self.document.len() {
            return Err(EditError::OutOfBounds);
        }
        if !self.document.is_char_boundary(edit.start) || !self.document.is_char_boundary(old_end) {
            return Err(EditError::NotCharBoundary);
        }

        let old_document_len = self.document.len();
        let start_line = self.line_at(edit.start);
        let delete_last_position = old_end.saturating_sub(1).max(edit.start);
        let touched_end_line = self.line_at(delete_last_position.min(old_document_len));
        let mut region_end_line = (touched_end_line + 1).min(self.lines.len() - 1);
        // A document ending in a newline has a zero-length terminal line. If the
        // edited region already reaches that byte boundary, it owns the terminal
        // line too; preserving the old sentinel would create a duplicate cache
        // entry at EOF and can split a cross-line token at the preserved newline.
        while region_end_line + 1 < self.lines.len()
            && self.lines[region_end_line + 1].content.is_empty()
            && self.starts[region_end_line + 1]
                == self.starts[region_end_line] + self.lines[region_end_line].content.len()
        {
            region_end_line += 1;
        }
        let region_start = self.starts[start_line];
        let old_region_end =
            self.starts[region_end_line] + self.lines[region_end_line].content.len();
        let delta = edit.replacement.len() as isize - edit.deleted_bytes as isize;
        let transform = EditTransform {
            start: edit.start,
            old_end,
            delta,
        };

        self.document
            .replace_range(edit.start..old_end, &edit.replacement);
        let new_region_end = shift_position(old_region_end, delta);
        let region = &self.document[region_start..new_region_end];
        let mut replacement_lines: Vec<_> = line_slices(region)
            .map(|(_, line)| CachedLine::invalid(line))
            .collect();
        if new_region_end < self.document.len()
            && replacement_lines
                .last()
                .is_some_and(|line| line.content.is_empty())
        {
            replacement_lines.pop();
        }
        self.lines
            .splice(start_line..=region_end_line, replacement_lines);
        self.starts = starts_for(&self.lines);

        let mut state = if start_line == 0 {
            LexState::Normal
        } else {
            self.lines[start_line - 1].outgoing
        };
        let mut lines_relexed = 0usize;
        let mut bytes_scanned = 0usize;
        let mut converged = false;
        let mut last_scanned = start_line.saturating_sub(1);

        for index in start_line..self.lines.len() {
            let start = self.starts[index];
            let was_valid = self.lines[index].valid;
            let cached_incoming = rebase_state(self.lines[index].incoming, transform);
            let cached_outgoing = rebase_state(self.lines[index].outgoing, transform);
            let exact_content_matches = self
                .document
                .get(start..start + self.lines[index].content.len())
                .is_some_and(|content| content == self.lines[index].content.as_ref());
            let (tokens, outgoing) = scan_line(&self.lines[index].content, start, state);
            lines_relexed += 1;
            bytes_scanned += self.lines[index].content.len();
            last_scanned = index;

            let boundary_matches = was_valid
                && exact_content_matches
                && self.lines[index].fingerprint
                    == fingerprint(self.lines[index].content.as_bytes())
                && cached_incoming == Some(state)
                && cached_outgoing == Some(outgoing)
                && self.lines[index].tokens == tokens;

            self.lines[index].incoming = state;
            self.lines[index].outgoing = outgoing;
            self.lines[index].tokens = tokens;
            self.lines[index].valid = true;
            state = outgoing;

            if boundary_matches && self.can_rebase_suffix(index + 1, transform) {
                self.rebase_suffix(index + 1, transform);
                converged = true;
                break;
            }
        }

        self.counters.edits += 1;
        self.counters.lines_relexed += lines_relexed as u64;
        self.counters.bytes_scanned += bytes_scanned as u64;
        self.max_retained_index_estimate_bytes = self
            .max_retained_index_estimate_bytes
            .max(self.retained_index_estimate_bytes());
        Ok(EditWork {
            lines_relexed,
            bytes_scanned,
            converged,
            scanned_to_eof: last_scanned + 1 == self.lines.len(),
        })
    }

    /// Estimates retained index allocations from owned capacities.
    ///
    /// This is not a process peak-memory measurement. In particular, it omits
    /// transient allocations used while applying an edit or assembling results,
    /// allocator fragmentation, code, and other process state.
    pub fn retained_index_estimate_bytes(&self) -> usize {
        // Add a heuristic 32-byte allowance for every retained backing allocation.
        const ALLOCATION_OVERHEAD_ALLOWANCE: usize = 32;
        let structural = self.lines.capacity() * size_of::<CachedLine>()
            + self.starts.capacity() * size_of::<usize>();
        let line_content: usize = self.lines.iter().map(|line| line.content.len()).sum();
        let line_tokens: usize = self
            .lines
            .iter()
            .map(|line| line.tokens.capacity() * size_of::<RelativeToken>())
            .sum();
        let backing_allocations = 2
            + self
                .lines
                .iter()
                .filter(|line| !line.content.is_empty())
                .count()
            + self
                .lines
                .iter()
                .filter(|line| line.tokens.capacity() > 0)
                .count();
        structural
            + line_content
            + line_tokens
            + backing_allocations * ALLOCATION_OVERHEAD_ALLOWANCE
    }

    fn line_at(&self, position: usize) -> usize {
        match self.starts.binary_search(&position) {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index - 1,
        }
    }

    fn can_rebase_suffix(&self, from: usize, transform: EditTransform) -> bool {
        self.lines[from..].iter().all(|line| {
            rebase_state(line.incoming, transform).is_some()
                && rebase_state(line.outgoing, transform).is_some()
        })
    }

    fn rebase_suffix(&mut self, from: usize, transform: EditTransform) {
        for line in &mut self.lines[from..] {
            line.incoming =
                rebase_state(line.incoming, transform).expect("checked before rebasing");
            line.outgoing =
                rebase_state(line.outgoing, transform).expect("checked before rebasing");
        }
    }
}

pub fn validate_coverage(document: &str, result: &LexResult) -> Result<(), String> {
    let mut cursor = 0;
    for token in &result.tokens {
        if token.start != cursor || token.end <= token.start || token.end > document.len() {
            return Err(format!(
                "invalid token range {}..{} after {cursor}",
                token.start, token.end
            ));
        }
        if !document.is_char_boundary(token.start) || !document.is_char_boundary(token.end) {
            return Err(format!(
                "token splits UTF-8 at {}..{}",
                token.start, token.end
            ));
        }
        let _covered_bytes = &document.as_bytes()[token.start..token.end];
        cursor = token.end;
    }
    if cursor != document.len() {
        return Err(format!("tokens cover {cursor} of {} bytes", document.len()));
    }
    Ok(())
}

pub fn result_digest(result: &LexResult, counters: WorkCounters) -> String {
    let bytes = serde_json::to_vec(&(result, counters)).expect("serializable result");
    format!("{:016x}", fingerprint(&bytes))
}

fn scan_line(
    line: &str,
    absolute_start: usize,
    incoming: LexState,
) -> (Vec<RelativeToken>, LexState) {
    let bytes = line.as_bytes();
    let mut cursor = 0;
    let mut state = incoming;
    let mut tokens = Vec::new();

    while cursor < bytes.len() {
        match state {
            LexState::String { escaped, start } => {
                let token_start = cursor;
                let (end, closed, next_escaped) = scan_string(bytes, cursor, escaped);
                tokens.push(RelativeToken {
                    kind: TokenKind::String,
                    start: token_start,
                    end,
                });
                cursor = end;
                state = if closed {
                    LexState::Normal
                } else {
                    LexState::String {
                        escaped: next_escaped,
                        start,
                    }
                };
            }
            LexState::BlockComment { start } => {
                let token_start = cursor;
                let (end, closed) = scan_block_comment(bytes, cursor);
                tokens.push(RelativeToken {
                    kind: TokenKind::BlockComment,
                    start: token_start,
                    end,
                });
                cursor = end;
                state = if closed {
                    LexState::Normal
                } else {
                    LexState::BlockComment { start }
                };
            }
            LexState::Normal => {
                if is_line_ending_start(bytes, cursor) || char_at(line, cursor).is_whitespace() {
                    let start = cursor;
                    while cursor < bytes.len() && char_at(line, cursor).is_whitespace() {
                        cursor += char_at(line, cursor).len_utf8();
                    }
                    tokens.push(RelativeToken {
                        kind: TokenKind::Whitespace,
                        start,
                        end: cursor,
                    });
                } else if bytes[cursor] == b'#' {
                    let start = cursor;
                    while cursor < bytes.len() && !is_line_ending_start(bytes, cursor) {
                        cursor += char_at(line, cursor).len_utf8();
                    }
                    tokens.push(RelativeToken {
                        kind: TokenKind::LineComment,
                        start,
                        end: cursor,
                    });
                } else if bytes[cursor] == b'/'
                    && cursor + 1 < bytes.len()
                    && bytes[cursor + 1] == b'*'
                {
                    let start = cursor;
                    let (end, closed) = scan_block_comment(bytes, cursor + 2);
                    tokens.push(RelativeToken {
                        kind: TokenKind::BlockComment,
                        start,
                        end,
                    });
                    cursor = end;
                    state = if closed {
                        LexState::Normal
                    } else {
                        LexState::BlockComment {
                            start: absolute_start + start,
                        }
                    };
                } else if bytes[cursor] == b'"' {
                    let start = cursor;
                    let (end, closed, escaped) = scan_string(bytes, cursor + 1, false);
                    tokens.push(RelativeToken {
                        kind: TokenKind::String,
                        start,
                        end,
                    });
                    cursor = end;
                    state = if closed {
                        LexState::Normal
                    } else {
                        LexState::String {
                            escaped,
                            start: absolute_start + start,
                        }
                    };
                } else {
                    let character = char_at(line, cursor);
                    let kind = if is_identifier_start(character) {
                        TokenKind::Identifier
                    } else if character.is_ascii_digit() {
                        TokenKind::Number
                    } else {
                        TokenKind::Punctuation
                    };
                    let start = cursor;
                    cursor += character.len_utf8();
                    match kind {
                        TokenKind::Identifier => {
                            while cursor < bytes.len()
                                && is_identifier_continue(char_at(line, cursor))
                            {
                                cursor += char_at(line, cursor).len_utf8();
                            }
                        }
                        TokenKind::Number => {
                            while cursor < bytes.len() && char_at(line, cursor).is_ascii_digit() {
                                cursor += 1;
                            }
                        }
                        _ => {}
                    }
                    tokens.push(RelativeToken {
                        kind,
                        start,
                        end: cursor,
                    });
                }
            }
        }
    }
    (tokens, state)
}

fn scan_string(bytes: &[u8], mut cursor: usize, mut escaped: bool) -> (usize, bool, bool) {
    while cursor < bytes.len() {
        let width = utf8_width(bytes[cursor]);
        if escaped {
            escaped = false;
            cursor += width;
        } else if bytes[cursor] == b'\\' {
            escaped = true;
            cursor += 1;
        } else if bytes[cursor] == b'"' {
            return (cursor + 1, true, false);
        } else {
            cursor += width;
        }
    }
    (cursor, false, escaped)
}

fn scan_block_comment(bytes: &[u8], mut cursor: usize) -> (usize, bool) {
    while cursor < bytes.len() {
        if bytes[cursor] == b'*' && cursor + 1 < bytes.len() && bytes[cursor + 1] == b'/' {
            return (cursor + 2, true);
        }
        cursor += utf8_width(bytes[cursor]);
    }
    (cursor, false)
}

fn diagnostics(state: LexState, document_len: usize) -> Vec<Diagnostic> {
    match state {
        LexState::Normal => Vec::new(),
        LexState::String { start, .. } => vec![Diagnostic {
            kind: DiagnosticKind::UnterminatedString,
            start,
            end: document_len,
        }],
        LexState::BlockComment { start } => vec![Diagnostic {
            kind: DiagnosticKind::UnterminatedBlockComment,
            start,
            end: document_len,
        }],
    }
}

fn line_slices(document: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut start = 0;
    let mut slices = Vec::new();
    for (index, byte) in document.bytes().enumerate() {
        if byte == b'\n' {
            slices.push((start, &document[start..=index]));
            start = index + 1;
        }
    }
    slices.push((start, &document[start..]));
    slices.into_iter()
}

fn starts_for(lines: &[CachedLine]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(lines.len());
    let mut cursor = 0;
    for line in lines {
        starts.push(cursor);
        cursor += line.content.len();
    }
    starts
}

fn is_line_ending_start(bytes: &[u8], cursor: usize) -> bool {
    bytes[cursor] == b'\n'
        || (bytes[cursor] == b'\r' && cursor + 1 < bytes.len() && bytes[cursor + 1] == b'\n')
}

fn char_at(text: &str, cursor: usize) -> char {
    text[cursor..].chars().next().expect("cursor within string")
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character.is_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn utf8_width(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn shift_position(position: usize, delta: isize) -> usize {
    if delta >= 0 {
        position + delta as usize
    } else {
        position - (-delta) as usize
    }
}

fn rebase_state(state: LexState, transform: EditTransform) -> Option<LexState> {
    let rebase = |position: usize| {
        if position < transform.start {
            Some(position)
        } else if position >= transform.old_end {
            Some(shift_position(position, transform.delta))
        } else {
            None
        }
    };
    match state {
        LexState::Normal => Some(LexState::Normal),
        LexState::String { escaped, start } => {
            rebase(start).map(|start| LexState::String { escaped, start })
        }
        LexState::BlockComment { start } => {
            rebase(start).map(|start| LexState::BlockComment { start })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_lexer_covers_utf8_and_states() {
        let doc = "alpha 12 λ; # hi\r\n\"escaped \\\" quote\" /* block\nend */ z";
        let result = full_lex(doc);
        validate_coverage(doc, &result).unwrap();
        assert_eq!(result.final_state, LexState::Normal);
    }

    #[test]
    fn unterminated_diagnostic_points_to_opener() {
        let doc = "x\n/* open\nstill";
        let result = full_lex(doc);
        assert_eq!(result.diagnostics[0].start, 2);
        assert_eq!(result.diagnostics[0].end, doc.len());
    }

    #[test]
    fn localized_and_propagating_edits_match_full_lexer() {
        let mut lexer = IncrementalLexer::new("head\nbody\ntail\n".to_string());
        for edit in [
            Edit {
                start: 6,
                deleted_bytes: 1,
                replacement: "λ".to_string(),
            },
            Edit {
                start: 5,
                deleted_bytes: 0,
                replacement: "/*".to_string(),
            },
            Edit {
                start: 5,
                deleted_bytes: 2,
                replacement: String::new(),
            },
        ] {
            lexer.apply_edit(&edit).unwrap();
            let actual = lexer.result();
            let expected = full_lex(lexer.document());
            assert_eq!(actual, expected);
            validate_coverage(lexer.document(), &actual).unwrap();
        }
    }

    #[test]
    fn rejected_edits_leave_state_unchanged() {
        let mut lexer = IncrementalLexer::new("aλz".to_string());
        let before_doc = lexer.document().to_string();
        let before_result = lexer.result();
        let before_counters = lexer.counters();
        assert_eq!(
            lexer.apply_edit(&Edit {
                start: 2,
                deleted_bytes: 0,
                replacement: "x".to_string(),
            }),
            Err(EditError::NotCharBoundary)
        );
        assert_eq!(
            lexer.apply_edit(&Edit {
                start: 99,
                deleted_bytes: 0,
                replacement: String::new(),
            }),
            Err(EditError::OutOfBounds)
        );
        assert_eq!(lexer.document(), before_doc);
        assert_eq!(lexer.result(), before_result);
        assert_eq!(lexer.counters(), before_counters);
    }

    #[test]
    fn cross_line_edit_does_not_split_string_token_at_preserved_newline() {
        let mut lexer = IncrementalLexer::new("/λa\n\n".to_string());
        lexer
            .apply_edit(&Edit {
                start: 3,
                deleted_bytes: 2,
                replacement: "\r\n\"🙂🙂".to_string(),
            })
            .unwrap();
        assert_eq!(lexer.document(), "/λ\r\n\"🙂🙂\n");
        assert_eq!(lexer.line_count(), 3);
        let result = lexer.result();
        assert_eq!(result, full_lex(lexer.document()));
        let string_ranges: Vec<_> = result
            .tokens
            .iter()
            .filter(|token| token.kind == TokenKind::String)
            .map(|token| token.start..token.end)
            .collect();
        assert_eq!(string_ranges, vec![5..15]);
    }
}
