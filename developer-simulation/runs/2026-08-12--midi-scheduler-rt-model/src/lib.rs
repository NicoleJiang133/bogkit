//! Test-first model of a fixed-capacity MIDI callback scheduler.

use std::collections::{BTreeMap, BTreeSet};

pub mod oracle;

const PULSES_PER_QUARTER: i128 = 960;
const MICROS_PER_SECOND: i128 = 1_000_000;
const FRAME_DENOMINATOR: i128 = PULSES_PER_QUARTER * MICROS_PER_SECOND;
const MAX_EVENTS: usize = 200_000;
const MAX_TEMPO_NODES: usize = 50_000;

/// One piecewise-constant tempo segment, effective at `pulse`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TempoNode {
    pub pulse: i64,
    pub micros_per_quarter: u32,
}

/// MIDI data retained by the scheduling core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    NoteOn {
        channel: u8,
        key: u8,
        velocity: u8,
        instance_id: u32,
    },
    NoteOff {
        channel: u8,
        key: u8,
        velocity: u8,
        instance_id: u32,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
    AllNotesOff {
        channel: u8,
    },
}

impl EventKind {
    /// Lower values survive ordinary overload first.
    #[must_use]
    pub const fn priority(self) -> u8 {
        match self {
            Self::AllNotesOff { .. } => 0,
            Self::NoteOff { .. } => 1,
            Self::ControlChange { .. } => 2,
            Self::ProgramChange { .. } => 3,
            Self::NoteOn { .. } => 4,
        }
    }

    const fn midi_is_valid(self) -> bool {
        match self {
            Self::NoteOn {
                channel,
                key,
                velocity,
                ..
            }
            | Self::NoteOff {
                channel,
                key,
                velocity,
                ..
            } => channel < 16 && key < 128 && velocity < 128,
            Self::ControlChange {
                channel,
                controller,
                value,
            } => channel < 16 && controller < 128 && value < 128,
            Self::ProgramChange { channel, program } => channel < 16 && program < 128,
            Self::AllNotesOff { channel } => channel < 16,
        }
    }
}

/// Beat-positioned event supplied by the control thread.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event {
    pub pulse: i64,
    pub stable_id: u64,
    pub kind: EventKind,
}

/// Complete off-thread plan input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanSpec {
    pub origin_pulse: i64,
    pub origin_frame: i64,
    pub sample_rate: u32,
    pub tempo_nodes: Vec<TempoNode>,
    pub events: Vec<Event>,
}

/// Stable validation failures; values are intentionally independent of prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PlanError {
    UnsupportedSampleRate = 1,
    EmptyTempoMap = 2,
    TempoOriginMismatch = 3,
    TempoNodesUnsorted = 4,
    InvalidTempo = 5,
    TooManyTempoNodes = 6,
    TooManyEvents = 7,
    PulseBeforeOrigin = 8,
    EventsUnsorted = 9,
    DuplicateStableId = 10,
    DuplicateNoteInstance = 11,
    UnmatchedNoteOff = 12,
    DuplicateTermination = 13,
    InvalidMidiData = 14,
    FrameOverflow = 15,
    TerminationIdentityMismatch = 16,
}

/// An event after exact off-thread pulse-to-frame conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedEvent {
    pub frame: i64,
    pub stable_id: u64,
    pub kind: EventKind,
}

/// Immutable callback input. All vectors are built before publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedPlan {
    sample_rate: u32,
    events: Vec<PreparedEvent>,
    note_instances: Vec<NoteIdentity>,
    digest: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct NoteIdentity {
    instance_id: u32,
    channel: u8,
    key: u8,
}

impl PreparedPlan {
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[must_use]
    pub fn events(&self) -> &[PreparedEvent] {
        &self.events
    }

    #[must_use]
    pub fn digest(&self) -> u64 {
        self.digest
    }

    #[must_use]
    pub fn contains_note_instance(&self, instance_id: u32) -> bool {
        self.note_instances
            .binary_search_by_key(&instance_id, |identity| identity.instance_id)
            .is_ok()
    }

    fn contains_note_identity(&self, identity: NoteIdentity) -> bool {
        self.note_instances.binary_search(&identity).is_ok()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstanceState {
    Open,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InstanceRecord {
    state: InstanceState,
    channel: u8,
    key: u8,
}

/// Validate and prepare one immutable plan off the callback thread.
///
/// # Errors
///
/// Returns a stable [`PlanError`] for malformed MIDI, ordering, tempo, size,
/// sample-rate, note-instance, or checked-conversion input.
pub fn prepare_plan(spec: PlanSpec) -> Result<PreparedPlan, PlanError> {
    if !matches!(spec.sample_rate, 44_100 | 48_000 | 96_000) {
        return Err(PlanError::UnsupportedSampleRate);
    }
    if spec.tempo_nodes.is_empty() {
        return Err(PlanError::EmptyTempoMap);
    }
    if spec.tempo_nodes.len() > MAX_TEMPO_NODES {
        return Err(PlanError::TooManyTempoNodes);
    }
    if spec.events.len() > MAX_EVENTS {
        return Err(PlanError::TooManyEvents);
    }
    if spec.tempo_nodes[0].pulse != spec.origin_pulse {
        return Err(PlanError::TempoOriginMismatch);
    }
    if spec
        .tempo_nodes
        .iter()
        .any(|node| node.micros_per_quarter == 0)
    {
        return Err(PlanError::InvalidTempo);
    }
    if spec
        .tempo_nodes
        .windows(2)
        .any(|nodes| nodes[0].pulse >= nodes[1].pulse)
    {
        return Err(PlanError::TempoNodesUnsorted);
    }

    let instances = validate_events(&spec.events, spec.origin_pulse)?;

    let mut prepared = Vec::with_capacity(spec.events.len());
    let mut converter = FrameConverter::new(
        spec.origin_pulse,
        spec.origin_frame,
        spec.sample_rate,
        &spec.tempo_nodes,
    )?;
    for event in spec.events {
        prepared.push(PreparedEvent {
            frame: converter.convert(event.pulse)?,
            stable_id: event.stable_id,
            kind: event.kind,
        });
    }
    prepared.sort_unstable_by_key(|event| (event.frame, event.kind.priority(), event.stable_id));
    let note_instances = instances
        .into_iter()
        .map(|(instance_id, record)| NoteIdentity {
            instance_id,
            channel: record.channel,
            key: record.key,
        })
        .collect();
    let digest = digest_plan(spec.sample_rate, &prepared);

    Ok(PreparedPlan {
        sample_rate: spec.sample_rate,
        events: prepared,
        note_instances,
        digest,
    })
}

fn validate_events(
    events: &[Event],
    origin_pulse: i64,
) -> Result<BTreeMap<u32, InstanceRecord>, PlanError> {
    let mut stable_ids = BTreeSet::new();
    let mut instances = BTreeMap::new();
    let mut previous_key = None;
    for event in events {
        if event.pulse < origin_pulse {
            return Err(PlanError::PulseBeforeOrigin);
        }
        let key = (event.pulse, event.kind.priority(), event.stable_id);
        if previous_key.is_some_and(|previous| previous > key) {
            return Err(PlanError::EventsUnsorted);
        }
        previous_key = Some(key);
        if !stable_ids.insert(event.stable_id) {
            return Err(PlanError::DuplicateStableId);
        }
        if !event.kind.midi_is_valid() {
            return Err(PlanError::InvalidMidiData);
        }
        match event.kind {
            EventKind::NoteOn {
                instance_id,
                channel,
                key,
                ..
            } => {
                if instances
                    .insert(
                        instance_id,
                        InstanceRecord {
                            state: InstanceState::Open,
                            channel,
                            key,
                        },
                    )
                    .is_some()
                {
                    return Err(PlanError::DuplicateNoteInstance);
                }
            }
            EventKind::NoteOff {
                instance_id,
                channel,
                key,
                ..
            } => match instances.get_mut(&instance_id) {
                None => return Err(PlanError::UnmatchedNoteOff),
                Some(InstanceRecord {
                    state: InstanceState::Closed,
                    ..
                }) => return Err(PlanError::DuplicateTermination),
                Some(record) if record.channel != channel || record.key != key => {
                    return Err(PlanError::TerminationIdentityMismatch);
                }
                Some(record) => record.state = InstanceState::Closed,
            },
            EventKind::ControlChange { .. }
            | EventKind::ProgramChange { .. }
            | EventKind::AllNotesOff { .. } => {}
        }
    }

    Ok(instances)
}

struct FrameConverter<'a> {
    numerator: i128,
    cursor: i64,
    sample_rate: u32,
    micros_per_quarter: u32,
    tempo_nodes: &'a [TempoNode],
    next_tempo: usize,
}

impl<'a> FrameConverter<'a> {
    fn new(
        origin_pulse: i64,
        origin_frame: i64,
        sample_rate: u32,
        tempo_nodes: &'a [TempoNode],
    ) -> Result<Self, PlanError> {
        let numerator = i128::from(origin_frame)
            .checked_mul(FRAME_DENOMINATOR)
            .ok_or(PlanError::FrameOverflow)?;
        Ok(Self {
            numerator,
            cursor: origin_pulse,
            sample_rate,
            micros_per_quarter: tempo_nodes[0].micros_per_quarter,
            tempo_nodes,
            next_tempo: 1,
        })
    }

    fn convert(&mut self, target_pulse: i64) -> Result<i64, PlanError> {
        while let Some(node) = self.tempo_nodes.get(self.next_tempo)
            && node.pulse <= target_pulse
        {
            self.numerator = add_segment(
                self.numerator,
                self.cursor,
                node.pulse,
                self.micros_per_quarter,
                self.sample_rate,
            )?;
            self.cursor = node.pulse;
            self.micros_per_quarter = node.micros_per_quarter;
            self.next_tempo += 1;
        }
        self.numerator = add_segment(
            self.numerator,
            self.cursor,
            target_pulse,
            self.micros_per_quarter,
            self.sample_rate,
        )?;
        self.cursor = target_pulse;
        frame_from_numerator(self.numerator)
    }
}

fn add_segment(
    numerator: i128,
    from_pulse: i64,
    to_pulse: i64,
    micros_per_quarter: u32,
    sample_rate: u32,
) -> Result<i128, PlanError> {
    let pulse_delta = i128::from(to_pulse) - i128::from(from_pulse);
    let delta = pulse_delta
        .checked_mul(i128::from(micros_per_quarter))
        .and_then(|value| value.checked_mul(i128::from(sample_rate)))
        .ok_or(PlanError::FrameOverflow)?;
    numerator.checked_add(delta).ok_or(PlanError::FrameOverflow)
}

fn frame_from_numerator(numerator: i128) -> Result<i64, PlanError> {
    i64::try_from(numerator.div_euclid(FRAME_DENOMINATOR)).map_err(|_| PlanError::FrameOverflow)
}

fn digest_plan(sample_rate: u32, events: &[PreparedEvent]) -> u64 {
    let mut hash = fnv(u64::from(sample_rate), 0xcbf2_9ce4_8422_2325);
    for event in events {
        hash = fnv(event.frame.cast_unsigned(), hash);
        hash = fnv(event.stable_id, hash);
        hash = fnv(u64::from(event.kind.priority()), hash);
        hash = fnv(kind_bits(event.kind), hash);
    }
    hash
}

const fn fnv(value: u64, mut hash: u64) -> u64 {
    let bytes = value.to_le_bytes();
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

fn kind_bits(kind: EventKind) -> u64 {
    match kind {
        EventKind::NoteOn {
            channel,
            key,
            velocity,
            instance_id,
        }
        | EventKind::NoteOff {
            channel,
            key,
            velocity,
            instance_id,
        } => {
            u64::from(instance_id) << 24
                | u64::from(channel) << 16
                | u64::from(key) << 8
                | u64::from(velocity)
        }
        EventKind::ControlChange {
            channel,
            controller,
            value,
        } => u64::from(channel) << 16 | u64::from(controller) << 8 | u64::from(value),
        EventKind::ProgramChange { channel, program } => {
            u64::from(channel) << 8 | u64::from(program)
        }
        EventKind::AllNotesOff { channel } => u64::from(channel),
    }
}

/// Explicit transport state transition supplied by the host wrapper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Discontinuity {
    None,
    Seek,
    LoopWrap,
    SampleRateChange,
    PlanReplacement,
}

/// Callback metadata. Blocks are interpreted as `[start_frame, start_frame + length)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostBlock {
    /// Must be greater than the token of the last successful callback.
    /// Failed calls do not consume their token and may be retried.
    pub token: u64,
    pub start_frame: i64,
    pub length: u32,
    pub sample_rate: u32,
    pub discontinuity: Discontinuity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallbackError {
    EmptyBlock,
    BlockTooLarge,
    UnsupportedSampleRate,
    PlanSampleRateMismatch,
    FrameRangeOverflow,
    TerminationOutputTooSmall,
    NonMonotonicToken,
}

/// One caller-owned output entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduledEvent {
    pub absolute_frame: i64,
    pub frame_offset: u32,
    pub stable_id: u64,
    pub kind: EventKind,
    pub synthetic: bool,
}

impl ScheduledEvent {
    pub const EMPTY: Self = Self {
        absolute_frame: 0,
        frame_offset: 0,
        stable_id: 0,
        kind: EventKind::AllNotesOff { channel: 0 },
        synthetic: false,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduleResult {
    pub written: usize,
    pub synthetic_terminations: usize,
    /// True only when `NonMonotonicToken` was caused by equality.
    pub duplicate_token: bool,
    pub overflowed: bool,
    pub error: Option<CallbackError>,
}

impl ScheduleResult {
    const fn error(error: CallbackError) -> Self {
        Self {
            written: 0,
            synthetic_terminations: 0,
            duplicate_token: false,
            overflowed: false,
            error: Some(error),
        }
    }

    const fn non_monotonic_token(duplicate_token: bool) -> Self {
        Self {
            written: 0,
            synthetic_terminations: 0,
            duplicate_token,
            overflowed: false,
            error: Some(CallbackError::NonMonotonicToken),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveSlot {
    Empty,
    Live {
        instance_id: u32,
        channel: u8,
        key: u8,
    },
}

/// Fixed-capacity callback state. Construction and clearing are control-thread operations.
pub struct Scheduler {
    active: Box<[ActiveSlot]>,
    last_token: Option<u64>,
    sticky_overflow: bool,
}

impl Scheduler {
    /// Allocate the active-note table before the scheduler reaches the callback.
    #[must_use]
    pub fn new(max_active_notes: usize) -> Self {
        Self {
            active: vec![ActiveSlot::Empty; max_active_notes].into_boxed_slice(),
            last_token: None,
            sticky_overflow: false,
        }
    }

    /// Schedule one host block into `output` without growing callback-owned storage.
    pub fn schedule(
        &mut self,
        plan: &PreparedPlan,
        block: HostBlock,
        output: &mut [ScheduledEvent],
    ) -> ScheduleResult {
        if let Some(last_token) = self.last_token
            && block.token <= last_token
        {
            return ScheduleResult::non_monotonic_token(block.token == last_token);
        }
        if block.length == 0 {
            return ScheduleResult::error(CallbackError::EmptyBlock);
        }
        if block.length > 2_048 {
            return ScheduleResult::error(CallbackError::BlockTooLarge);
        }
        if !matches!(block.sample_rate, 44_100 | 48_000 | 96_000) {
            return ScheduleResult::error(CallbackError::UnsupportedSampleRate);
        }
        if block.sample_rate != plan.sample_rate {
            return ScheduleResult::error(CallbackError::PlanSampleRateMismatch);
        }
        let Some(end_frame) = block.start_frame.checked_add(i64::from(block.length)) else {
            return ScheduleResult::error(CallbackError::FrameRangeOverflow);
        };

        let events = plan.events();
        let begin = events.partition_point(|event| event.frame < block.start_frame);
        let end = events.partition_point(|event| event.frame < end_frame);
        let ordinary = &events[begin..end];
        let termination = if self.required_termination_count(plan, block.discontinuity, ordinary)
            > output.len()
        {
            self.emit_channel_fallback(plan, block, ordinary, output)
                .map(|written| (written, true))
        } else {
            self.emit_terminations(plan, block, output)
        };
        let (synthetic_terminations, channel_fallback) = match termination {
            Ok(value) => value,
            Err(error) => return ScheduleResult::error(error),
        };
        let available = output.len().saturating_sub(synthetic_terminations);
        let mut selected = 0_usize;
        let mut overflowed = false;

        for event in ordinary {
            if channel_fallback
                && matches!(
                    event.kind,
                    EventKind::NoteOn { .. }
                        | EventKind::NoteOff { .. }
                        | EventKind::AllNotesOff { .. }
                )
            {
                overflowed = true;
                continue;
            }
            let candidate = ScheduledEvent {
                absolute_frame: event.frame,
                frame_offset: u32::try_from(event.frame - block.start_frame).unwrap_or_default(),
                stable_id: event.stable_id,
                kind: event.kind,
                synthetic: false,
            };
            if selected < available {
                if let Some(slot) = output.get_mut(synthetic_terminations + selected) {
                    *slot = candidate;
                    selected += 1;
                }
            } else {
                overflowed = true;
                if available == 0 {
                    continue;
                }
                let ordinary = &mut output
                    [synthetic_terminations..synthetic_terminations.saturating_add(selected)];
                let mut worst = 0_usize;
                for index in 1..ordinary.len() {
                    if retention_key(ordinary[index]) > retention_key(ordinary[worst]) {
                        worst = index;
                    }
                }
                if retention_key(candidate) < retention_key(ordinary[worst]) {
                    ordinary[worst] = candidate;
                }
            }
        }

        let ordinary_end = synthetic_terminations + selected;
        insertion_sort_events(&mut output[synthetic_terminations..ordinary_end]);
        let written = self.apply_emitted_state(
            output,
            synthetic_terminations,
            ordinary_end,
            &mut overflowed,
        );
        if overflowed {
            self.sticky_overflow = true;
        }
        self.last_token = Some(block.token);
        ScheduleResult {
            written,
            synthetic_terminations,
            duplicate_token: false,
            overflowed,
            error: None,
        }
    }

    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active
            .iter()
            .filter(|slot| matches!(slot, ActiveSlot::Live { .. }))
            .count()
    }

    #[must_use]
    pub fn sticky_overflow(&self) -> bool {
        self.sticky_overflow
    }

    /// This is deliberately separate from the callback operation.
    pub fn clear_overflow_control(&mut self) {
        self.sticky_overflow = false;
    }

    fn emit_terminations(
        &mut self,
        plan: &PreparedPlan,
        block: HostBlock,
        output: &mut [ScheduledEvent],
    ) -> Result<(usize, bool), CallbackError> {
        if block.discontinuity == Discontinuity::None {
            return Ok((0, false));
        }
        let invalidate = |slot: ActiveSlot| match (block.discontinuity, slot) {
            (
                Discontinuity::PlanReplacement,
                ActiveSlot::Live {
                    instance_id,
                    channel,
                    key,
                },
            ) => !plan.contains_note_identity(NoteIdentity {
                instance_id,
                channel,
                key,
            }),
            (_, ActiveSlot::Live { .. }) => true,
            (_, ActiveSlot::Empty) => false,
        };
        let termination_count = self
            .active
            .iter()
            .copied()
            .filter(|slot| invalidate(*slot))
            .count();
        if termination_count == 0 {
            return Ok((0, false));
        }

        if termination_count > output.len() {
            let mut channels = [false; 16];
            for slot in self.active.iter().copied().filter(|slot| invalidate(*slot)) {
                if let ActiveSlot::Live { channel, .. } = slot {
                    channels[usize::from(channel)] = true;
                }
            }
            let affected_channels = channels.iter().filter(|affected| **affected).count();
            if affected_channels > output.len() {
                return Err(CallbackError::TerminationOutputTooSmall);
            }
            let mut written = 0_usize;
            for (channel, affected) in (0_u8..16).zip(channels) {
                if affected && let Some(slot) = output.get_mut(written) {
                    *slot = ScheduledEvent {
                        absolute_frame: block.start_frame,
                        frame_offset: 0,
                        stable_id: (1_u64 << 63) | u64::from(channel),
                        kind: EventKind::AllNotesOff { channel },
                        synthetic: true,
                    };
                    written += 1;
                }
            }
            for slot in &mut self.active {
                if invalidate(*slot) {
                    *slot = ActiveSlot::Empty;
                }
            }
            self.sticky_overflow = true;
            return Ok((written, true));
        }

        let mut written = 0_usize;
        for slot in &mut self.active {
            if !invalidate(*slot) {
                continue;
            }
            if let ActiveSlot::Live {
                instance_id,
                channel,
                key,
            } = *slot
                && let Some(destination) = output.get_mut(written)
            {
                *destination = ScheduledEvent {
                    absolute_frame: block.start_frame,
                    frame_offset: 0,
                    stable_id: (1_u64 << 63) | u64::from(instance_id),
                    kind: EventKind::NoteOff {
                        channel,
                        key,
                        velocity: 0,
                        instance_id,
                    },
                    synthetic: true,
                };
                written += 1;
            }
            *slot = ActiveSlot::Empty;
        }
        insertion_sort_events(&mut output[..written]);
        Ok((written, false))
    }

    fn required_termination_count(
        &self,
        plan: &PreparedPlan,
        discontinuity: Discontinuity,
        ordinary: &[PreparedEvent],
    ) -> usize {
        let discontinuity_count = self
            .active
            .iter()
            .copied()
            .filter(|slot| Self::slot_invalidated(*slot, plan, discontinuity))
            .count();
        let ordinary_count = self
            .active
            .iter()
            .copied()
            .filter(|slot| Self::ordinary_terminates_slot(*slot, ordinary))
            .count();
        discontinuity_count.saturating_add(ordinary_count)
    }

    fn emit_channel_fallback(
        &mut self,
        plan: &PreparedPlan,
        block: HostBlock,
        ordinary: &[PreparedEvent],
        output: &mut [ScheduledEvent],
    ) -> Result<usize, CallbackError> {
        let mut channels = [false; 16];
        for slot in self.active.iter().copied().filter(|slot| {
            Self::slot_requires_termination(*slot, plan, block.discontinuity, ordinary)
        }) {
            if let ActiveSlot::Live { channel, .. } = slot {
                channels[usize::from(channel)] = true;
            }
        }
        let affected_channels = channels.iter().filter(|affected| **affected).count();
        if affected_channels > output.len() {
            return Err(CallbackError::TerminationOutputTooSmall);
        }

        let mut written = 0_usize;
        for (channel, affected) in (0_u8..16).zip(channels) {
            if affected && let Some(destination) = output.get_mut(written) {
                *destination = ScheduledEvent {
                    absolute_frame: block.start_frame,
                    frame_offset: 0,
                    stable_id: (1_u64 << 63) | u64::from(channel),
                    kind: EventKind::AllNotesOff { channel },
                    synthetic: true,
                };
                written += 1;
            }
        }
        for slot in &mut self.active {
            if let ActiveSlot::Live { channel, .. } = *slot
                && channels[usize::from(channel)]
            {
                *slot = ActiveSlot::Empty;
            }
        }
        self.sticky_overflow = true;
        Ok(written)
    }

    fn slot_requires_termination(
        slot: ActiveSlot,
        plan: &PreparedPlan,
        discontinuity: Discontinuity,
        ordinary: &[PreparedEvent],
    ) -> bool {
        Self::slot_invalidated(slot, plan, discontinuity)
            || Self::ordinary_terminates_slot(slot, ordinary)
    }

    fn slot_invalidated(
        slot: ActiveSlot,
        plan: &PreparedPlan,
        discontinuity: Discontinuity,
    ) -> bool {
        let ActiveSlot::Live {
            instance_id,
            channel,
            key,
        } = slot
        else {
            return false;
        };
        match discontinuity {
            Discontinuity::None => false,
            Discontinuity::PlanReplacement => !plan.contains_note_identity(NoteIdentity {
                instance_id,
                channel,
                key,
            }),
            Discontinuity::Seek | Discontinuity::LoopWrap | Discontinuity::SampleRateChange => true,
        }
    }

    fn ordinary_terminates_slot(slot: ActiveSlot, ordinary: &[PreparedEvent]) -> bool {
        let ActiveSlot::Live {
            instance_id,
            channel,
            key,
        } = slot
        else {
            return false;
        };
        ordinary.iter().any(|event| match event.kind {
            EventKind::NoteOff {
                instance_id: off_instance,
                channel: off_channel,
                key: off_key,
                ..
            } => off_instance == instance_id && off_channel == channel && off_key == key,
            EventKind::AllNotesOff {
                channel: off_channel,
            } => off_channel == channel,
            EventKind::NoteOn { .. }
            | EventKind::ControlChange { .. }
            | EventKind::ProgramChange { .. } => false,
        })
    }

    fn apply_emitted_state(
        &mut self,
        output: &mut [ScheduledEvent],
        ordinary_start: usize,
        ordinary_end: usize,
        overflowed: &mut bool,
    ) -> usize {
        let mut write = ordinary_start;
        for read in ordinary_start..ordinary_end {
            let Some(event) = output.get(read).copied() else {
                break;
            };
            let keep = match event.kind {
                EventKind::NoteOn {
                    instance_id,
                    channel,
                    key,
                    ..
                } => {
                    if self.has_active(instance_id) {
                        false
                    } else if self.insert_active(instance_id, channel, key) {
                        true
                    } else {
                        *overflowed = true;
                        false
                    }
                }
                EventKind::NoteOff {
                    instance_id,
                    channel,
                    key,
                    ..
                } => {
                    self.remove_active(instance_id, channel, key);
                    true
                }
                EventKind::AllNotesOff { channel } => {
                    self.clear_channel(channel);
                    true
                }
                EventKind::ControlChange { .. } | EventKind::ProgramChange { .. } => true,
            };
            if keep && let Some(destination) = output.get_mut(write) {
                *destination = event;
                write += 1;
            }
        }
        write
    }

    fn has_active(&self, instance_id: u32) -> bool {
        self.active.iter().any(|slot| {
            matches!(
                slot,
                ActiveSlot::Live {
                    instance_id: active,
                    ..
                } if *active == instance_id
            )
        })
    }

    fn insert_active(&mut self, instance_id: u32, channel: u8, key: u8) -> bool {
        for slot in &mut self.active {
            if *slot == ActiveSlot::Empty {
                *slot = ActiveSlot::Live {
                    instance_id,
                    channel,
                    key,
                };
                return true;
            }
        }
        false
    }

    fn remove_active(&mut self, instance_id: u32, channel: u8, key: u8) {
        for slot in &mut self.active {
            if matches!(
                slot,
                ActiveSlot::Live {
                    instance_id: active,
                    channel: active_channel,
                    key: active_key,
                } if *active == instance_id && *active_channel == channel && *active_key == key
            ) {
                *slot = ActiveSlot::Empty;
                return;
            }
        }
    }

    fn clear_channel(&mut self, channel: u8) {
        for slot in &mut self.active {
            if matches!(slot, ActiveSlot::Live { channel: active, .. } if *active == channel) {
                *slot = ActiveSlot::Empty;
            }
        }
    }
}

const fn retention_key(event: ScheduledEvent) -> (u8, u64, i64) {
    (event.kind.priority(), event.stable_id, event.absolute_frame)
}

fn insertion_sort_events(events: &mut [ScheduledEvent]) {
    for index in 1..events.len() {
        let value = events[index];
        let mut cursor = index;
        while cursor > 0 && schedule_key(value) < schedule_key(events[cursor - 1]) {
            events[cursor] = events[cursor - 1];
            cursor -= 1;
        }
        events[cursor] = value;
    }
}

const fn schedule_key(event: ScheduledEvent) -> (u32, u8, u64) {
    (event.frame_offset, event.kind.priority(), event.stable_id)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlotState {
    Current,
    Retired,
}

struct PlanSlot {
    plan: PreparedPlan,
    generation: u64,
    state: SlotState,
    active_ticket: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublishReceipt {
    pub generation: u64,
    pub digest: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadTicket {
    slot: usize,
    generation: u64,
    digest: u64,
    ticket_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanObservation {
    pub generation: u64,
    pub digest: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReclaimedGeneration {
    pub generation: u64,
    pub digest: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishError {
    NoFreeSlot,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffError {
    StaleTicket,
    ReaderAlreadyEnded,
}

/// Two-slot fake-host handoff used to model publication and delayed reclamation.
///
/// This deliberately is not claimed to be a concurrent lock-free primitive: its
/// methods require exclusive access while reader boundaries are instrumented.
pub struct FixedHandoff {
    slots: [Option<PlanSlot>; 2],
    current: Option<usize>,
    next_generation: u64,
    next_ticket: u64,
}

impl FixedHandoff {
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            current: None,
            next_generation: 1,
            next_ticket: 1,
        }
    }

    /// Control-thread publication into the free fixed slot.
    ///
    /// # Errors
    ///
    /// Returns an error if delayed reclamation leaves no free slot or the
    /// generation counter is exhausted.
    pub fn publish(&mut self, plan: PreparedPlan) -> Result<PublishReceipt, PublishError> {
        let Some(free) = self.slots.iter().position(Option::is_none) else {
            return Err(PublishError::NoFreeSlot);
        };
        let generation = self.next_generation;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(PublishError::GenerationExhausted)?;
        if let Some(current) = self.current
            && let Some(slot) = self.slots.get_mut(current).and_then(Option::as_mut)
        {
            slot.state = SlotState::Retired;
        }
        let digest = plan.digest();
        self.slots[free] = Some(PlanSlot {
            plan,
            generation,
            state: SlotState::Current,
            active_ticket: None,
        });
        self.current = Some(free);
        Ok(PublishReceipt { generation, digest })
    }

    /// Instrument the point where the single fake audio thread captures a slot.
    pub fn begin_callback(&mut self) -> Option<ReadTicket> {
        let slot_index = self.current?;
        let slot = self.slots.get_mut(slot_index)?.as_mut()?;
        if slot.active_ticket.is_some() {
            return None;
        }
        let ticket_id = self.next_ticket;
        self.next_ticket = self.next_ticket.wrapping_add(1);
        slot.active_ticket = Some(ticket_id);
        Some(ReadTicket {
            slot: slot_index,
            generation: slot.generation,
            digest: slot.plan.digest(),
            ticket_id,
        })
    }

    /// Copy the two identity fields; neither can come from another slot.
    ///
    /// # Errors
    ///
    /// Returns [`HandoffError::StaleTicket`] when the ticket no longer pins
    /// the same complete slot.
    pub fn observe(&self, ticket: ReadTicket) -> Result<PlanObservation, HandoffError> {
        let slot = self.ticket_slot(ticket)?;
        Ok(PlanObservation {
            generation: slot.generation,
            digest: slot.plan.digest(),
        })
    }

    /// Use the pinned immutable plan in a fake callback.
    ///
    /// # Errors
    ///
    /// Returns [`HandoffError::StaleTicket`] when the ticket no longer pins
    /// the same complete slot.
    pub fn with_plan<R>(
        &self,
        ticket: ReadTicket,
        operation: impl FnOnce(&PreparedPlan) -> R,
    ) -> Result<R, HandoffError> {
        let slot = self.ticket_slot(ticket)?;
        Ok(operation(&slot.plan))
    }

    /// Release one fake callback ticket.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale ticket or a ticket that was already ended.
    pub fn end_callback(&mut self, ticket: ReadTicket) -> Result<(), HandoffError> {
        let Some(slot) = self.slots.get_mut(ticket.slot).and_then(Option::as_mut) else {
            return Err(HandoffError::StaleTicket);
        };
        if slot.generation != ticket.generation || slot.plan.digest() != ticket.digest {
            return Err(HandoffError::StaleTicket);
        }
        match slot.active_ticket {
            Some(active) if active == ticket.ticket_id => {
                slot.active_ticket = None;
                Ok(())
            }
            _ => Err(HandoffError::ReaderAlreadyEnded),
        }
    }

    /// Remove at most one unpinned retired plan on the control thread.
    pub fn reclaim_retired_control(&mut self) -> Option<ReclaimedGeneration> {
        let index = self.slots.iter().position(|slot| {
            slot.as_ref().is_some_and(|slot| {
                slot.state == SlotState::Retired && slot.active_ticket.is_none()
            })
        })?;
        let slot = self.slots[index].take()?;
        Some(ReclaimedGeneration {
            generation: slot.generation,
            digest: slot.plan.digest(),
        })
    }

    #[must_use]
    pub fn current_generation(&self) -> Option<u64> {
        self.current_slot().map(|slot| slot.generation)
    }

    #[must_use]
    pub fn current_digest(&self) -> Option<u64> {
        self.current_slot().map(|slot| slot.plan.digest())
    }

    fn current_slot(&self) -> Option<&PlanSlot> {
        self.current
            .and_then(|index| self.slots.get(index))
            .and_then(Option::as_ref)
    }

    fn ticket_slot(&self, ticket: ReadTicket) -> Result<&PlanSlot, HandoffError> {
        let slot = self
            .slots
            .get(ticket.slot)
            .and_then(Option::as_ref)
            .ok_or(HandoffError::StaleTicket)?;
        if slot.generation != ticket.generation
            || slot.plan.digest() != ticket.digest
            || slot.active_ticket != Some(ticket.ticket_id)
        {
            return Err(HandoffError::StaleTicket);
        }
        Ok(slot)
    }
}

impl Default for FixedHandoff {
    fn default() -> Self {
        Self::new()
    }
}
