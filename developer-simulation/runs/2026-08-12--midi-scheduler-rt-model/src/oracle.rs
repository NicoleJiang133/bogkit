//! Slow allocating rational oracle.
//!
//! This module intentionally duplicates conversion and block selection. It
//! does not call the scheduler's preparation or timestamp helpers.

use crate::{EventKind, HostBlock, PlanSpec, ScheduledEvent};

const ORACLE_DENOMINATOR: i128 = 960_i128 * 1_000_000_i128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OracleError {
    EmptyTempoMap,
    PulseBeforeOrigin,
    FrameOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OracleEvent {
    frame: i64,
    stable_id: u64,
    kind: EventKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OraclePlan {
    sample_rate: u32,
    events: Vec<OracleEvent>,
}

impl OraclePlan {
    /// Convert every event independently with the slow rational baseline.
    ///
    /// # Errors
    ///
    /// Returns an error when the tempo map is empty, an event precedes the
    /// declared origin, or the exact frame does not fit in a signed 64-bit value.
    pub fn from_spec(spec: &PlanSpec) -> Result<Self, OracleError> {
        let mut events = Vec::with_capacity(spec.events.len());
        for event in &spec.events {
            let frame = oracle_frame(spec, event.pulse)?;
            events.push(OracleEvent {
                frame,
                stable_id: event.stable_id,
                kind: event.kind,
            });
        }
        events.sort_unstable_by_key(|event| (event.frame, event.kind.priority(), event.stable_id));
        Ok(Self {
            sample_rate: spec.sample_rate,
            events,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OracleRunner {
    last_token: Option<u64>,
}

impl OracleRunner {
    #[must_use]
    pub const fn new() -> Self {
        Self { last_token: None }
    }

    pub fn schedule(&mut self, plan: &OraclePlan, block: HostBlock) -> Vec<ScheduledEvent> {
        if self.last_token == Some(block.token)
            || block.sample_rate != plan.sample_rate
            || block.length == 0
        {
            return Vec::new();
        }
        let Some(end) = block.start_frame.checked_add(i64::from(block.length)) else {
            return Vec::new();
        };
        self.last_token = Some(block.token);
        let mut output = Vec::new();
        for event in &plan.events {
            if event.frame >= block.start_frame && event.frame < end {
                let Ok(frame_offset) = u32::try_from(event.frame - block.start_frame) else {
                    continue;
                };
                output.push(ScheduledEvent {
                    absolute_frame: event.frame,
                    frame_offset,
                    stable_id: event.stable_id,
                    kind: event.kind,
                    synthetic: false,
                });
            }
        }
        output
    }
}

fn oracle_frame(spec: &PlanSpec, target_pulse: i64) -> Result<i64, OracleError> {
    if target_pulse < spec.origin_pulse {
        return Err(OracleError::PulseBeforeOrigin);
    }
    let Some(first) = spec.tempo_nodes.first() else {
        return Err(OracleError::EmptyTempoMap);
    };
    let mut numerator = i128::from(spec.origin_frame)
        .checked_mul(ORACLE_DENOMINATOR)
        .ok_or(OracleError::FrameOverflow)?;
    let mut segment_start = spec.origin_pulse;
    let mut micros = first.micros_per_quarter;

    for next in spec.tempo_nodes.iter().skip(1) {
        let segment_end = target_pulse.min(next.pulse);
        numerator = oracle_add(
            numerator,
            segment_end,
            segment_start,
            micros,
            spec.sample_rate,
        )?;
        if target_pulse <= next.pulse {
            return oracle_integer_frame(numerator);
        }
        segment_start = next.pulse;
        micros = next.micros_per_quarter;
    }
    numerator = oracle_add(
        numerator,
        target_pulse,
        segment_start,
        micros,
        spec.sample_rate,
    )?;
    oracle_integer_frame(numerator)
}

fn oracle_add(
    accumulator: i128,
    end_pulse: i64,
    start_pulse: i64,
    micros: u32,
    sample_rate: u32,
) -> Result<i128, OracleError> {
    let distance = i128::from(end_pulse) - i128::from(start_pulse);
    let segment = distance
        .checked_mul(i128::from(micros))
        .and_then(|value| value.checked_mul(i128::from(sample_rate)))
        .ok_or(OracleError::FrameOverflow)?;
    accumulator
        .checked_add(segment)
        .ok_or(OracleError::FrameOverflow)
}

fn oracle_integer_frame(numerator: i128) -> Result<i64, OracleError> {
    i64::try_from(numerator.div_euclid(ORACLE_DENOMINATOR)).map_err(|_| OracleError::FrameOverflow)
}
