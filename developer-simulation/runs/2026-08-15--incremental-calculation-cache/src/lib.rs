//! Compact, discardable adapter for evaluating Fold as a calculated-snapshot publisher.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use fold::pipeline::terminal;
use fold::stream::KeyedStream;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Cell {
    sheet: u16,
    row: u32,
    column: u32,
}

impl Cell {
    #[must_use]
    pub const fn new(sheet: u16, row: u32, column: u32) -> Self {
        Self { sheet, row, column }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CellValue {
    Number(i128),
    Error(ErrorCode),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ErrorCode {
    Cycle,
    MissingReference,
    Overflow,
}

impl ErrorCode {
    const fn label(self) -> &'static str {
        match self {
            Self::Cycle => "CYCLE",
            Self::MissingReference => "MISSING_REFERENCE",
            Self::Overflow => "OVERFLOW",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Range {
    start: Cell,
    end: Cell,
}

impl Range {
    #[must_use]
    pub const fn new(start: Cell, end: Cell) -> Self {
        Self { start, end }
    }

    fn cells(self) -> Vec<Cell> {
        if self.start.sheet != self.end.sheet {
            return Vec::new();
        }
        let mut cells = Vec::new();
        for row in self.start.row.min(self.end.row)..=self.start.row.max(self.end.row) {
            for column in
                self.start.column.min(self.end.column)..=self.start.column.max(self.end.column)
            {
                cells.push(Cell::new(self.start.sheet, row, column));
            }
        }
        cells
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Formula {
    Sum(Range),
    Reference(Cell),
    Add(Cell, Cell),
    IfGreaterThenAdd {
        condition: Cell,
        threshold: i128,
        left: Cell,
        right: Cell,
    },
}

impl Formula {
    fn dependencies(&self) -> Vec<Cell> {
        match self {
            Self::Sum(range) => range.cells(),
            Self::Reference(cell) => vec![*cell],
            Self::Add(left, right) => vec![*left, *right],
            Self::IfGreaterThenAdd {
                condition,
                left,
                right,
                ..
            } => vec![*condition, *left, *right],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Edit {
    SetLiteral(Cell, i128),
    AdjustLiteral(Cell, i128),
    SetFormula(Cell, Formula),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Batch {
    sequence: u64,
    edits: Vec<Edit>,
}

impl Batch {
    #[must_use]
    pub fn new(sequence: u64, edits: Vec<Edit>) -> Self {
        Self { sequence, edits }
    }

    #[must_use]
    /// Encodes the batch in the compact deterministic trial wire format.
    ///
    /// # Panics
    ///
    /// Panics if the record or edit count exceeds the wire format's `u32` limits.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PBK1");
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&self.sequence.to_le_bytes());
        bytes.extend_from_slice(
            &u32::try_from(self.edits.len())
                .expect("edit count fits in the wire format")
                .to_le_bytes(),
        );
        for edit in &self.edits {
            encode_edit(&mut bytes, edit);
        }
        let total_len = u32::try_from(bytes.len() + 8).expect("record length fits in u32");
        bytes[4..8].copy_from_slice(&total_len.to_le_bytes());
        bytes.extend_from_slice(&checksum(&bytes).to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, Reject> {
        const HEADER_AND_CHECKSUM: usize = 28;
        if bytes.len() < HEADER_AND_CHECKSUM || bytes.get(..4) != Some(b"PBK1") {
            return Err(Reject::TruncatedRecord);
        }
        let declared = u32::from_le_bytes(bytes[4..8].try_into().expect("fixed width"));
        if usize::try_from(declared).expect("u32 fits usize") != bytes.len() {
            return Err(Reject::TruncatedRecord);
        }
        let payload_end = bytes.len() - 8;
        let expected_checksum = u64::from_le_bytes(
            bytes[payload_end..]
                .try_into()
                .expect("checksum has fixed width"),
        );
        if checksum(&bytes[..payload_end]) != expected_checksum {
            return Err(Reject::BadChecksum);
        }

        let sequence = u64::from_le_bytes(bytes[8..16].try_into().expect("fixed width"));
        let count = u32::from_le_bytes(bytes[16..20].try_into().expect("fixed width"));
        let mut cursor = Cursor::new(&bytes[20..payload_end]);
        let mut edits = Vec::with_capacity(usize::try_from(count).expect("u32 fits usize"));
        for _ in 0..count {
            edits.push(decode_edit(&mut cursor)?);
        }
        if !cursor.is_empty() {
            return Err(Reject::TrailingBytes);
        }
        Ok(Self { sequence, edits })
    }
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn encode_cell(bytes: &mut Vec<u8>, cell: Cell) {
    bytes.extend_from_slice(&cell.sheet.to_le_bytes());
    bytes.extend_from_slice(&cell.row.to_le_bytes());
    bytes.extend_from_slice(&cell.column.to_le_bytes());
}

fn encode_formula(bytes: &mut Vec<u8>, formula: &Formula) {
    match formula {
        Formula::Sum(range) => {
            bytes.push(10);
            encode_cell(bytes, range.start);
            encode_cell(bytes, range.end);
        }
        Formula::Reference(cell) => {
            bytes.push(11);
            encode_cell(bytes, *cell);
        }
        Formula::Add(left, right) => {
            bytes.push(12);
            encode_cell(bytes, *left);
            encode_cell(bytes, *right);
        }
        Formula::IfGreaterThenAdd {
            condition,
            threshold,
            left,
            right,
        } => {
            bytes.push(13);
            encode_cell(bytes, *condition);
            bytes.extend_from_slice(&threshold.to_le_bytes());
            encode_cell(bytes, *left);
            encode_cell(bytes, *right);
        }
    }
}

fn encode_edit(bytes: &mut Vec<u8>, edit: &Edit) {
    match edit {
        Edit::SetLiteral(cell, value) => {
            bytes.push(1);
            encode_cell(bytes, *cell);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        Edit::AdjustLiteral(cell, delta) => {
            bytes.push(2);
            encode_cell(bytes, *cell);
            bytes.extend_from_slice(&delta.to_le_bytes());
        }
        Edit::SetFormula(cell, formula) => {
            bytes.push(3);
            encode_cell(bytes, *cell);
            encode_formula(bytes, formula);
        }
    }
}

struct Cursor<'a> {
    remaining: &'a [u8],
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    const fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], Reject> {
        let Some((head, tail)) = self.remaining.split_at_checked(N) else {
            return Err(Reject::TruncatedRecord);
        };
        self.remaining = tail;
        Ok(head.try_into().expect("slice has requested width"))
    }

    fn byte(&mut self) -> Result<u8, Reject> {
        Ok(self.take::<1>()?[0])
    }

    fn cell(&mut self) -> Result<Cell, Reject> {
        Ok(Cell::new(
            u16::from_le_bytes(self.take()?),
            u32::from_le_bytes(self.take()?),
            u32::from_le_bytes(self.take()?),
        ))
    }

    fn integer(&mut self) -> Result<i128, Reject> {
        Ok(i128::from_le_bytes(self.take()?))
    }
}

fn decode_formula(cursor: &mut Cursor<'_>) -> Result<Formula, Reject> {
    match cursor.byte()? {
        10 => Ok(Formula::Sum(Range::new(cursor.cell()?, cursor.cell()?))),
        11 => Ok(Formula::Reference(cursor.cell()?)),
        12 => Ok(Formula::Add(cursor.cell()?, cursor.cell()?)),
        13 => Ok(Formula::IfGreaterThenAdd {
            condition: cursor.cell()?,
            threshold: cursor.integer()?,
            left: cursor.cell()?,
            right: cursor.cell()?,
        }),
        opcode => Err(Reject::UnknownFormulaOpcode(opcode)),
    }
}

fn decode_edit(cursor: &mut Cursor<'_>) -> Result<Edit, Reject> {
    match cursor.byte()? {
        1 => Ok(Edit::SetLiteral(cursor.cell()?, cursor.integer()?)),
        2 => Ok(Edit::AdjustLiteral(cursor.cell()?, cursor.integer()?)),
        3 => Ok(Edit::SetFormula(cursor.cell()?, decode_formula(cursor)?)),
        opcode => Err(Reject::UnknownEditOpcode(opcode)),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CellDefinition {
    Literal(i128),
    Formula(Formula),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Workbook {
    definitions: BTreeMap<Cell, CellDefinition>,
}

impl Workbook {
    #[must_use]
    pub fn deterministic_fixture() -> Self {
        let mut definitions = BTreeMap::new();
        let a1 = Cell::new(0, 0, 0);
        let a2 = Cell::new(0, 1, 0);
        let b1 = Cell::new(0, 0, 1);
        let c1 = Cell::new(0, 0, 2);
        definitions.insert(a1, CellDefinition::Literal(10));
        definitions.insert(a2, CellDefinition::Literal(20));
        definitions.insert(
            b1,
            CellDefinition::Formula(Formula::Sum(Range::new(a1, a2))),
        );
        definitions.insert(
            c1,
            CellDefinition::Formula(Formula::IfGreaterThenAdd {
                condition: b1,
                threshold: 15,
                left: b1,
                right: a1,
            }),
        );
        let cycle_a = Cell::new(1, 0, 0);
        let cycle_b = Cell::new(1, 0, 1);
        definitions.insert(
            cycle_a,
            CellDefinition::Formula(Formula::Reference(cycle_b)),
        );
        definitions.insert(
            cycle_b,
            CellDefinition::Formula(Formula::Reference(cycle_a)),
        );
        definitions.insert(
            Cell::new(1, 1, 0),
            CellDefinition::Formula(Formula::Reference(Cell::new(99, 0, 0))),
        );
        Self { definitions }
    }

    pub fn set_literal(&mut self, cell: Cell, value: i128) {
        self.definitions
            .insert(cell, CellDefinition::Literal(value));
    }

    fn apply_edits(&mut self, edits: &[Edit]) -> Result<BTreeSet<Cell>, Reject> {
        let mut changed = BTreeSet::new();
        for edit in edits {
            match edit {
                Edit::SetLiteral(cell, value) => {
                    self.definitions
                        .insert(*cell, CellDefinition::Literal(*value));
                    changed.insert(*cell);
                }
                Edit::AdjustLiteral(cell, delta) => {
                    let Some(CellDefinition::Literal(value)) = self.definitions.get_mut(cell)
                    else {
                        return Err(Reject::NotALiteral(*cell));
                    };
                    *value = value.checked_add(*delta).ok_or(Reject::IntegerOverflow)?;
                    changed.insert(*cell);
                }
                Edit::SetFormula(cell, formula) => {
                    self.definitions
                        .insert(*cell, CellDefinition::Formula(formula.clone()));
                    changed.insert(*cell);
                }
            }
        }
        Ok(changed)
    }

    fn reverse_dependencies(&self) -> BTreeMap<Cell, BTreeSet<Cell>> {
        let mut reverse = BTreeMap::<Cell, BTreeSet<Cell>>::new();
        for (dependent, definition) in &self.definitions {
            if let CellDefinition::Formula(formula) = definition {
                for dependency in formula.dependencies() {
                    reverse.entry(dependency).or_default().insert(*dependent);
                }
            }
        }
        reverse
    }

    fn affected(&self, changed: &BTreeSet<Cell>) -> Vec<Cell> {
        let reverse = self.reverse_dependencies();
        let mut affected = changed.clone();
        let mut queue = changed.iter().copied().collect::<VecDeque<_>>();
        while let Some(cell) = queue.pop_front() {
            if let Some(dependents) = reverse.get(&cell) {
                for dependent in dependents {
                    if affected.insert(*dependent) {
                        queue.push_back(*dependent);
                    }
                }
            }
        }
        affected
            .into_iter()
            .filter(|cell| self.definitions.contains_key(cell))
            .collect()
    }

    fn evaluate(&self) -> BTreeMap<Cell, CellValue> {
        let mut memo = BTreeMap::new();
        for cell in self.definitions.keys() {
            let mut visiting = BTreeSet::new();
            let value = self.evaluate_cell(*cell, &mut memo, &mut visiting);
            memo.insert(*cell, value);
        }
        memo.retain(|cell, _| self.definitions.contains_key(cell));
        memo
    }

    fn evaluate_cell(
        &self,
        cell: Cell,
        memo: &mut BTreeMap<Cell, CellValue>,
        visiting: &mut BTreeSet<Cell>,
    ) -> CellValue {
        if let Some(value) = memo.get(&cell) {
            return value.clone();
        }
        if !visiting.insert(cell) {
            return CellValue::Error(ErrorCode::Cycle);
        }
        let value = match self.definitions.get(&cell) {
            None => CellValue::Error(ErrorCode::MissingReference),
            Some(CellDefinition::Literal(value)) => CellValue::Number(*value),
            Some(CellDefinition::Formula(formula)) => {
                self.evaluate_formula(formula, memo, visiting)
            }
        };
        visiting.remove(&cell);
        memo.insert(cell, value.clone());
        value
    }

    fn evaluate_formula(
        &self,
        formula: &Formula,
        memo: &mut BTreeMap<Cell, CellValue>,
        visiting: &mut BTreeSet<Cell>,
    ) -> CellValue {
        match formula {
            Formula::Sum(range) => {
                let mut total = 0_i128;
                for cell in range.cells() {
                    let CellValue::Number(value) = self.evaluate_cell(cell, memo, visiting) else {
                        return self.evaluate_cell(cell, memo, visiting);
                    };
                    let Some(next) = total.checked_add(value) else {
                        return CellValue::Error(ErrorCode::Overflow);
                    };
                    total = next;
                }
                CellValue::Number(total)
            }
            Formula::Reference(cell) => self.evaluate_cell(*cell, memo, visiting),
            Formula::Add(left, right) => self.evaluate_add(*left, *right, memo, visiting),
            Formula::IfGreaterThenAdd {
                condition,
                threshold,
                left,
                right,
            } => match self.evaluate_cell(*condition, memo, visiting) {
                CellValue::Number(value) if value > *threshold => {
                    self.evaluate_add(*left, *right, memo, visiting)
                }
                CellValue::Number(_) => CellValue::Number(0),
                error @ CellValue::Error(_) => error,
            },
        }
    }

    fn evaluate_add(
        &self,
        left: Cell,
        right: Cell,
        memo: &mut BTreeMap<Cell, CellValue>,
        visiting: &mut BTreeSet<Cell>,
    ) -> CellValue {
        match (
            self.evaluate_cell(left, memo, visiting),
            self.evaluate_cell(right, memo, visiting),
        ) {
            (CellValue::Number(left), CellValue::Number(right)) => left
                .checked_add(right)
                .map_or(CellValue::Error(ErrorCode::Overflow), CellValue::Number),
            (CellValue::Error(error), _) | (_, CellValue::Error(error)) => CellValue::Error(error),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reject {
    BadChecksum,
    TruncatedRecord,
    TrailingBytes,
    UnexpectedSequence { expected: u64, actual: u64 },
    UnknownEditOpcode(u8),
    UnknownFormulaOpcode(u8),
    IntegerOverflow,
    NotALiteral(Cell),
    Io(String),
    IncompleteDemo,
    ReaderTaskPanicked,
    AlreadyInitialized,
    NotInitialized,
    RecoveryMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Outcome {
    pub recalculated: Vec<Cell>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublishedCell {
    pub cell: Cell,
    pub generation: u64,
    pub value: CellValue,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VisibleSnapshot {
    pub generation: u64,
    pub cells: Vec<PublishedCell>,
}

impl VisibleSnapshot {
    #[must_use]
    pub fn point(&self, cell: Cell) -> Option<&CellValue> {
        self.cells
            .binary_search_by_key(&cell, |published| published.cell)
            .ok()
            .map(|index| &self.cells[index].value)
    }

    #[must_use]
    pub fn rectangular(&self, range: Range) -> Vec<(Cell, &CellValue)> {
        range
            .cells()
            .into_iter()
            .filter_map(|cell| self.point(cell).map(|value| (cell, value)))
            .collect()
    }

    #[must_use]
    pub fn errors(&self) -> Vec<(Cell, ErrorCode)> {
        self.cells
            .iter()
            .filter_map(|published| match published.value {
                CellValue::Error(error) => Some((published.cell, error)),
                CellValue::Number(_) => None,
            })
            .collect()
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut output = format!("generation={}\n", self.generation);
        for published in &self.cells {
            let cell = published.cell;
            match published.value {
                CellValue::Number(value) => writeln!(
                    output,
                    "{}:{}:{}=N:{value}",
                    cell.sheet, cell.row, cell.column
                )
                .expect("writing to String cannot fail"),
                CellValue::Error(error) => writeln!(
                    output,
                    "{}:{}:{}=E:{}",
                    cell.sheet,
                    cell.row,
                    cell.column,
                    error.label()
                )
                .expect("writing to String cannot fail"),
            }
        }
        output.into_bytes()
    }
}

#[derive(Clone)]
pub struct TrialReader {
    visible: Arc<RwLock<Arc<VisibleSnapshot>>>,
}

impl TrialReader {
    #[must_use]
    /// Clones the currently published complete generation.
    ///
    /// # Panics
    ///
    /// Panics if another thread previously poisoned the publication lock.
    pub fn read(&self) -> VisibleSnapshot {
        self.visible
            .read()
            .expect("snapshot lock was poisoned")
            .as_ref()
            .clone()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
enum SnapshotKey {
    Generation,
    Cell(Cell),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum StoredValue {
    Generation(u64),
    Cell(PublishedCell),
}

type Store = KeyedStream<SnapshotKey, StoredValue, terminal::Table<SnapshotKey, StoredValue>>;

pub struct Trial {
    workbook: Workbook,
    store: Store,
    visible: Arc<RwLock<Arc<VisibleSnapshot>>>,
    initialized: bool,
}

impl Trial {
    /// Opens a new disposable calculated-state directory.
    ///
    /// # Errors
    ///
    /// Returns a rejection if trial setup cannot be completed.
    pub fn open(path: &Path, workbook: Workbook) -> Result<Self, Reject> {
        let store = KeyedStream::new(path, terminal::Table::new("calculated_snapshot"));
        if read_store(&store).is_some() {
            return Err(Reject::AlreadyInitialized);
        }
        Ok(Self {
            workbook,
            store,
            visible: Arc::new(RwLock::new(Arc::new(VisibleSnapshot::default()))),
            initialized: false,
        })
    }

    /// Recovers writable state by replaying the authoritative journal without publishing.
    ///
    /// # Errors
    ///
    /// Returns a rejection when the store is empty, the journal is invalid, or replayed
    /// workbook state does not exactly match the persisted calculated snapshot.
    pub fn recover_from_journal(
        path: &Path,
        mut workbook: Workbook,
        journal: &[Vec<u8>],
    ) -> Result<Self, Reject> {
        let store = KeyedStream::new(path, terminal::Table::new("calculated_snapshot"));
        let visible = read_store(&store).ok_or(Reject::NotInitialized)?;
        let mut expected_sequence = 1_u64;
        for record in journal {
            let batch = Batch::decode(record)?;
            if batch.sequence != expected_sequence {
                return Err(Reject::UnexpectedSequence {
                    expected: expected_sequence,
                    actual: batch.sequence,
                });
            }
            workbook.apply_edits(&batch.edits)?;
            expected_sequence += 1;
        }
        let replayed_generation = expected_sequence - 1;
        let replayed = make_snapshot(replayed_generation, workbook.evaluate());
        if replayed != visible {
            return Err(Reject::RecoveryMismatch);
        }
        Ok(Self {
            workbook,
            store,
            visible: Arc::new(RwLock::new(Arc::new(visible))),
            initialized: true,
        })
    }

    /// Evaluates and publishes generation zero.
    ///
    /// # Errors
    ///
    /// Returns a rejection if evaluation cannot be published.
    pub fn bootstrap(&mut self) -> Result<(), Reject> {
        if self.initialized || read_store(&self.store).is_some() {
            return Err(Reject::AlreadyInitialized);
        }
        let values = self.workbook.evaluate();
        self.publish(make_snapshot(0, values));
        self.initialized = true;
        Ok(())
    }

    /// Validates, evaluates, and atomically publishes one journal batch.
    ///
    /// # Errors
    ///
    /// Returns [`Reject`] for malformed, out-of-order, unknown, or overflowing input.
    pub fn apply(&mut self, record: &[u8]) -> Result<Outcome, Reject> {
        if !self.initialized {
            return Err(Reject::NotInitialized);
        }
        let batch = Batch::decode(record)?;
        let expected = self.reader().read().generation + 1;
        if batch.sequence != expected {
            return Err(Reject::UnexpectedSequence {
                expected,
                actual: batch.sequence,
            });
        }

        let mut candidate = self.workbook.clone();
        let changed = candidate.apply_edits(&batch.edits)?;
        let recalculated = candidate.affected(&changed);
        let snapshot = make_snapshot(batch.sequence, candidate.evaluate());
        self.publish(snapshot);
        self.workbook = candidate;
        Ok(Outcome { recalculated })
    }

    #[must_use]
    pub fn reader(&self) -> TrialReader {
        TrialReader {
            visible: Arc::clone(&self.visible),
        }
    }

    fn publish(&mut self, snapshot: VisibleSnapshot) {
        self.store.wtx(|tx| {
            tx.upsert(
                &SnapshotKey::Generation,
                &StoredValue::Generation(snapshot.generation),
            );
            for cell in &snapshot.cells {
                tx.upsert(
                    &SnapshotKey::Cell(cell.cell),
                    &StoredValue::Cell(cell.clone()),
                );
            }
        });
        *self.visible.write().expect("snapshot lock was poisoned") = Arc::new(snapshot);
    }
}

fn make_snapshot(generation: u64, values: BTreeMap<Cell, CellValue>) -> VisibleSnapshot {
    VisibleSnapshot {
        generation,
        cells: values
            .into_iter()
            .map(|(cell, value)| PublishedCell {
                cell,
                generation,
                value,
            })
            .collect(),
    }
}

fn read_store(store: &Store) -> Option<VisibleSnapshot> {
    store.rtx(|table| {
        let mut found = false;
        let mut generation = 0;
        let mut cells = Vec::new();
        for (key, value) in table.iter() {
            found = true;
            match (key, value) {
                (SnapshotKey::Generation, StoredValue::Generation(value)) => generation = value,
                (SnapshotKey::Cell(_), StoredValue::Cell(cell)) => cells.push(cell),
                _ => {}
            }
        }
        cells.sort_by_key(|cell| cell.cell);
        found.then_some(VisibleSnapshot { generation, cells })
    })
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum GateStatus {
    #[default]
    NotRun,
    Pass,
    Fail,
}

impl GateStatus {
    const fn from_passed(passed: bool) -> Self {
        if passed { Self::Pass } else { Self::Fail }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DemoManifest {
    pub fixture_populated_cells: u32,
    pub fixture_literal_cells: u32,
    pub fixture_formula_cells: u32,
    pub accepted_batches: u32,
    pub clean_builds: u32,
    pub chunk_sizes: Vec<usize>,
    pub local_exactness: GateStatus,
    pub local_determinism: GateStatus,
    pub invalid_record_gate: GateStatus,
    pub one_way_initialization_gate: GateStatus,
    pub recovery_continuation_gate: GateStatus,
    pub reader_tasks: u32,
    pub read_requests: u64,
    pub mixed_generation_responses: u64,
    pub initial_build_microseconds: u64,
    pub batch_p95_microseconds: u64,
    pub batch_p99_microseconds: u64,
    pub recovery_replay_microseconds: u64,
    pub observed_single_store_disk_bytes: u64,
    pub observed_100_batch_store_disk_bytes: u64,
    pub observed_total_test_disk_bytes: u64,
    pub production_scale_gates: GateStatus,
}

/// Runs the bounded local demonstration and removes its temporary data.
///
/// # Errors
///
/// Returns a rejection when a journal gate, local store, reader task, or cleanup fails.
///
/// # Panics
///
/// Panics if Fold cannot open or commit its embedded store, or if a publication lock is poisoned.
pub fn run_demo(data_path: &Path) -> Result<DemoManifest, Reject> {
    remove_if_present(data_path)?;
    std::fs::create_dir_all(data_path).map_err(|error| Reject::Io(error.kind().to_string()))?;
    let trace = demo_trace();
    let mut repeatability = run_repeatability(data_path, &trace)?;
    let local_exactness_passed = repeatability.reference
        == b"generation=3\n0:0:0=N:99\n0:0:1=N:5\n0:0:2=N:0\n0:1:0=N:5\n1:0:0=E:CYCLE\n1:0:1=E:CYCLE\n1:1:0=E:MISSING_REFERENCE\n";
    let invalid_record_gate_passed = run_invalid_gate(data_path)?;
    let one_way_initialization_gate_passed = run_initialization_gate(data_path)?;
    let (
        recovery_continuation_gate_passed,
        recovery_replay_microseconds,
        observed_single_store_disk_bytes,
    ) = run_recovery_continuation_gate(data_path, &trace)?;
    let (mixed_generation_responses, observed_100_batch_store_disk_bytes) =
        run_concurrency_gate(data_path)?;
    repeatability.batch_latencies.sort_unstable();
    let observed_total_test_disk_bytes = directory_size(data_path)?;
    let manifest = DemoManifest {
        fixture_populated_cells: 7,
        fixture_literal_cells: 2,
        fixture_formula_cells: 5,
        accepted_batches: 3,
        clean_builds: 5,
        chunk_sizes: repeatability.chunk_sizes,
        local_exactness: GateStatus::from_passed(local_exactness_passed),
        local_determinism: GateStatus::from_passed(repeatability.deterministic),
        invalid_record_gate: GateStatus::from_passed(invalid_record_gate_passed),
        one_way_initialization_gate: GateStatus::from_passed(one_way_initialization_gate_passed),
        recovery_continuation_gate: GateStatus::from_passed(recovery_continuation_gate_passed),
        reader_tasks: 12,
        read_requests: 24_000,
        mixed_generation_responses,
        initial_build_microseconds: repeatability.initial_build_microseconds,
        batch_p95_microseconds: percentile(&repeatability.batch_latencies, 95),
        batch_p99_microseconds: percentile(&repeatability.batch_latencies, 99),
        recovery_replay_microseconds,
        observed_single_store_disk_bytes,
        observed_100_batch_store_disk_bytes,
        observed_total_test_disk_bytes,
        production_scale_gates: GateStatus::NotRun,
    };
    remove_if_present(data_path)?;
    Ok(manifest)
}

struct RepeatabilityEvidence {
    reference: Vec<u8>,
    deterministic: bool,
    chunk_sizes: Vec<usize>,
    initial_build_microseconds: u64,
    batch_latencies: Vec<u64>,
}

fn run_repeatability(data_path: &Path, trace: &[Vec<u8>]) -> Result<RepeatabilityEvidence, Reject> {
    let mut builds = Vec::new();
    let mut initial_build_microseconds = 0;
    let mut batch_latencies = Vec::new();
    for build in 0..5 {
        let mut trial = Trial::open(
            &data_path.join(format!("clean-{build}")),
            Workbook::deterministic_fixture(),
        )?;
        let started = Instant::now();
        trial.bootstrap()?;
        if build == 0 {
            initial_build_microseconds = micros(started.elapsed());
        }
        for record in trace {
            let started = Instant::now();
            trial.apply(record)?;
            if build == 0 {
                batch_latencies.push(micros(started.elapsed()));
            }
        }
        builds.push(trial.reader().read().canonical_bytes());
    }
    let reference = builds.first().cloned().ok_or(Reject::IncompleteDemo)?;
    let mut deterministic = builds.iter().all(|bytes| *bytes == reference);
    let chunk_sizes = vec![1, 7, 64, 511, 4_096];
    for size in &chunk_sizes {
        let mut trial = Trial::open(
            &data_path.join(format!("chunk-{size}")),
            Workbook::deterministic_fixture(),
        )?;
        trial.bootstrap()?;
        for chunk in trace.chunks(*size) {
            for record in chunk {
                trial.apply(record)?;
            }
        }
        deterministic &= trial.reader().read().canonical_bytes() == reference;
    }
    Ok(RepeatabilityEvidence {
        reference,
        deterministic,
        chunk_sizes,
        initial_build_microseconds,
        batch_latencies,
    })
}

fn run_invalid_gate(data_path: &Path) -> Result<bool, Reject> {
    let mut trial = Trial::open(
        &data_path.join("invalid"),
        Workbook::deterministic_fixture(),
    )?;
    trial.bootstrap()?;
    let before = trial.reader().read();
    let rejected =
        trial.apply(&Batch::new(2, vec![Edit::SetLiteral(Cell::new(0, 0, 0), 9)]).encode());
    Ok(matches!(
        rejected,
        Err(Reject::UnexpectedSequence {
            expected: 1,
            actual: 2
        })
    ) && trial.reader().read() == before)
}

fn run_initialization_gate(data_path: &Path) -> Result<bool, Reject> {
    let path = data_path.join("one-way-initialization");
    let record = Batch::new(1, vec![Edit::SetLiteral(Cell::new(0, 1, 0), 5)]).encode();
    let (before, bootstrap_rejected, duplicate_rejected) = {
        let mut trial = Trial::open(&path, Workbook::deterministic_fixture())?;
        trial.bootstrap()?;
        trial.apply(&record)?;
        let before = trial.reader().read();
        let bootstrap_rejected =
            trial.bootstrap() == Err(Reject::AlreadyInitialized) && trial.reader().read() == before;
        let duplicate_rejected = trial.apply(&record)
            == Err(Reject::UnexpectedSequence {
                expected: 2,
                actual: 1,
            })
            && trial.reader().read() == before;
        (before, bootstrap_rejected, duplicate_rejected)
    };
    let open_rejected = matches!(
        Trial::open(&path, Workbook::deterministic_fixture()),
        Err(Reject::AlreadyInitialized)
    );
    let recovered = Trial::recover_from_journal(
        &path,
        Workbook::deterministic_fixture(),
        std::slice::from_ref(&record),
    )?;
    Ok(bootstrap_rejected
        && duplicate_rejected
        && open_rejected
        && recovered.reader().read() == before)
}

fn run_recovery_continuation_gate(
    data_path: &Path,
    trace: &[Vec<u8>],
) -> Result<(bool, u64, u64), Reject> {
    let (next, journal) = trace.split_last().ok_or(Reject::IncompleteDemo)?;
    let uninterrupted_path = data_path.join("uninterrupted");
    let expected = {
        let mut trial = Trial::open(&uninterrupted_path, Workbook::deterministic_fixture())?;
        trial.bootstrap()?;
        for record in trace {
            trial.apply(record)?;
        }
        trial.reader().read()
    };
    let recovered_path = data_path.join("recovered");
    {
        let mut trial = Trial::open(&recovered_path, Workbook::deterministic_fixture())?;
        trial.bootstrap()?;
        for record in journal {
            trial.apply(record)?;
        }
    }
    let started = Instant::now();
    let mut recovered =
        Trial::recover_from_journal(&recovered_path, Workbook::deterministic_fixture(), journal)?;
    let elapsed = micros(started.elapsed());
    recovered.apply(next)?;
    let passed = recovered.reader().read() == expected;
    drop(recovered);
    Ok((passed, elapsed, directory_size(&recovered_path)?))
}

fn run_concurrency_gate(data_path: &Path) -> Result<(u64, u64), Reject> {
    let path = data_path.join("concurrency");
    let mut trial = Trial::open(&path, Workbook::deterministic_fixture())?;
    trial.bootstrap()?;
    let reader = trial.reader();
    let barrier = Arc::new(std::sync::Barrier::new(13));
    let mixed = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..12 {
            let reader = reader.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(scope.spawn(move || count_mixed_reads(&reader, &barrier)));
        }
        barrier.wait();
        for sequence in 1..=100 {
            trial.apply(
                &Batch::new(
                    sequence,
                    vec![Edit::SetLiteral(Cell::new(0, 1, 0), i128::from(sequence))],
                )
                .encode(),
            )?;
        }
        handles.into_iter().try_fold(0_u64, |total, handle| {
            handle
                .join()
                .map(|mixed| total + mixed)
                .map_err(|_| Reject::ReaderTaskPanicked)
        })
    })?;
    Ok((mixed, directory_size(&path)?))
}

fn count_mixed_reads(reader: &TrialReader, barrier: &std::sync::Barrier) -> u64 {
    barrier.wait();
    let mut mixed = 0_u64;
    for _ in 0..2_000 {
        let visible = reader.read();
        if visible
            .cells
            .iter()
            .any(|cell| cell.generation != visible.generation)
        {
            mixed += 1;
        }
    }
    mixed
}

fn demo_trace() -> Vec<Vec<u8>> {
    let a1 = Cell::new(0, 0, 0);
    let a2 = Cell::new(0, 1, 0);
    let b1 = Cell::new(0, 0, 1);
    vec![
        Batch::new(1, vec![Edit::SetLiteral(a2, 5)]).encode(),
        Batch::new(
            2,
            vec![Edit::SetFormula(b1, Formula::Sum(Range::new(a2, a2)))],
        )
        .encode(),
        Batch::new(3, vec![Edit::SetLiteral(a1, 99)]).encode(),
    ]
}

fn micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let index = ((values.len() - 1) * percentile) / 100;
    values[index]
}

fn remove_if_present(path: &Path) -> Result<(), Reject> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Reject::Io(error.kind().to_string())),
    }
}

fn directory_size(path: &Path) -> Result<u64, Reject> {
    let mut total = 0_u64;
    for entry in std::fs::read_dir(path).map_err(|error| Reject::Io(error.kind().to_string()))? {
        let entry = entry.map_err(|error| Reject::Io(error.kind().to_string()))?;
        let metadata = entry
            .metadata()
            .map_err(|error| Reject::Io(error.kind().to_string()))?;
        if metadata.is_dir() {
            total = total
                .checked_add(directory_size(&entry.path())?)
                .ok_or(Reject::IntegerOverflow)?;
        } else {
            total = total
                .checked_add(metadata.len())
                .ok_or(Reject::IntegerOverflow)?;
        }
    }
    Ok(total)
}
