use midi_scheduler_rt_model::{
    CallbackError, Discontinuity, Event, EventKind, HostBlock, PlanSpec, ScheduledEvent, Scheduler,
    TempoNode, prepare_plan,
};

fn spec(sample_rate: u32, micros_per_quarter: u32, events: Vec<Event>) -> PlanSpec {
    PlanSpec {
        origin_pulse: 0,
        origin_frame: 0,
        sample_rate,
        tempo_nodes: vec![TempoNode {
            pulse: 0,
            micros_per_quarter,
        }],
        events,
    }
}

fn block(token: u64, start_frame: i64, length: u32, sample_rate: u32) -> HostBlock {
    HostBlock {
        token,
        start_frame,
        length,
        sample_rate,
        discontinuity: Discontinuity::None,
    }
}

fn cc(pulse: i64, stable_id: u64) -> Event {
    Event {
        pulse,
        stable_id,
        kind: EventKind::ControlChange {
            channel: 0,
            controller: 1,
            value: (stable_id % 128) as u8,
        },
    }
}

fn note_on(pulse: i64, stable_id: u64, instance_id: u32, channel: u8) -> Event {
    Event {
        pulse,
        stable_id,
        kind: EventKind::NoteOn {
            channel,
            key: (instance_id % 128) as u8,
            velocity: 100,
            instance_id,
        },
    }
}

fn note_off(pulse: i64, stable_id: u64, instance_id: u32, channel: u8) -> Event {
    Event {
        pulse,
        stable_id,
        kind: EventKind::NoteOff {
            channel,
            key: (instance_id % 128) as u8,
            velocity: 0,
            instance_id,
        },
    }
}

#[test]
fn half_open_boundaries_hold_at_every_sample_rate() {
    // Break caught: end-frame events leak into the prior block or start-frame events are skipped.
    for sample_rate in [44_100, 48_000, 96_000] {
        let events: Vec<_> = (0_i64..80)
            .map(|pulse| cc(pulse, pulse.cast_unsigned() + 1))
            .collect();
        let plan = prepare_plan(spec(sample_rate, 15_000, events)).expect("boundary plan");
        let mut chosen = None;
        for pair in plan.events().windows(2) {
            if pair[1].frame == pair[0].frame + 1 && pair[0].frame >= 2 {
                chosen = Some((pair[0].frame - 2, pair[0].frame, pair[1].frame));
                break;
            }
        }
        let (start, last, excluded) = chosen.expect("adjacent represented frames");
        let mut scheduler = Scheduler::new(32);
        let mut out = [ScheduledEvent::EMPTY; 256];
        let result = scheduler.schedule(
            &plan,
            block(
                1,
                start,
                u32::try_from(excluded - start).expect("short block"),
                sample_rate,
            ),
            &mut out,
        );
        assert_eq!(result.error, None);
        assert!(
            out[..result.written]
                .iter()
                .any(|event| event.frame_offset == 0)
        );
        assert!(
            out[..result.written]
                .iter()
                .any(|event| event.frame_offset == u32::try_from(last - start).unwrap())
        );
        assert!(
            !out[..result.written]
                .iter()
                .any(|event| event.absolute_frame == excluded)
        );
    }
}

#[test]
fn duplicate_tokens_emit_nothing_and_variable_blocks_have_no_alignment_assumption() {
    // Break caught: callback replay doubles IDs or changing block size creates a gap/overlap.
    let sizes = [16_u32, 17, 255, 256, 257, 1_024, 2_048];
    let total_frames: i64 = sizes.iter().map(|&size| i64::from(size)).sum();
    let events: Vec<_> = (0..total_frames)
        .map(|frame| cc(frame, frame.cast_unsigned() + 1))
        .collect();
    let plan = prepare_plan(spec(48_000, 20_000, events)).expect("one-frame pulse plan");
    let mut scheduler = Scheduler::new(16);
    let mut out = vec![ScheduledEvent::EMPTY; 2_048].into_boxed_slice();
    let mut start = 0_i64;
    let mut emitted = Vec::new();
    for (index, size) in sizes.into_iter().enumerate() {
        let token = index as u64 + 10;
        let first = scheduler.schedule(&plan, block(token, start, size, 48_000), &mut out);
        emitted.extend(out[..first.written].iter().map(|event| event.stable_id));
        let replay = scheduler.schedule(&plan, block(token, start, size, 48_000), &mut out);
        assert!(replay.duplicate_token);
        assert_eq!(replay.written, 0);
        start += i64::from(size);
    }
    let expected: Vec<_> = (1..=u64::try_from(total_frames).unwrap()).collect();
    assert_eq!(emitted, expected);
}

#[test]
fn one_thousand_tokens_replayed_one_to_three_times_keep_the_same_id_digest() {
    // Break caught: replay count leaks into callback identity or advances event state.
    let events: Vec<_> = (0_i64..16_000)
        .map(|frame| cc(frame, frame.cast_unsigned() + 1))
        .collect();
    let plan = prepare_plan(spec(48_000, 20_000, events)).expect("replay plan");
    let mut scheduler = Scheduler::new(16);
    let mut out = [ScheduledEvent::EMPTY; 32];
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    let mut emitted = 0_usize;

    for index in 0_u64..1_000 {
        let host = block(
            index + 1,
            i64::try_from(index * 16).expect("bounded frame fits"),
            16,
            48_000,
        );
        let first = scheduler.schedule(&plan, host, &mut out);
        emitted += first.written;
        for event in &out[..first.written] {
            digest ^= event.stable_id;
            digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
        }
        for _ in 0..=(index % 3) {
            let replay = scheduler.schedule(&plan, host, &mut out);
            assert!(replay.duplicate_token);
            assert_eq!(replay.written, 0);
        }
    }
    assert_eq!(emitted, 16_000);
    assert_ne!(digest, 0);
}

#[test]
fn every_non_increasing_token_is_rejected_without_reemission() {
    // Break caught: remembering only the immediately previous token allows 10, 11, 10 to replay block 10.
    let plan =
        prepare_plan(spec(48_000, 20_000, vec![cc(0, 1), cc(1, 2), cc(2, 3)])).expect("token plan");
    let mut scheduler = Scheduler::new(4);
    let mut out = [ScheduledEvent::EMPTY; 4];

    let first = scheduler.schedule(&plan, block(10, 0, 1, 48_000), &mut out);
    assert_eq!(first.written, 1);
    let second = scheduler.schedule(&plan, block(11, 1, 1, 48_000), &mut out);
    assert_eq!(second.written, 1);

    for token in [10_u64, 11, 9, 10] {
        let rejected = scheduler.schedule(&plan, block(token, 0, 3, 48_000), &mut out);
        assert_eq!(rejected.error, Some(CallbackError::NonMonotonicToken));
        assert_eq!(rejected.written, 0);
    }

    let next = scheduler.schedule(&plan, block(12, 2, 1, 48_000), &mut out);
    assert_eq!(next.error, None);
    assert_eq!(next.written, 1);
    assert_eq!(out[0].stable_id, 3);
}

#[test]
fn discontinuities_terminate_active_instances_before_new_events() {
    // Break caught: a seek, loop, sample-rate swap, or invalidating replacement leaves a prior note active.
    let old = prepare_plan(spec(
        48_000,
        20_000,
        vec![note_on(0, 1, 77, 2), note_off(100, 2, 77, 2)],
    ))
    .expect("old plan");
    let replacement = prepare_plan(spec(48_000, 20_000, vec![cc(0, 3)])).expect("replacement plan");
    let new_rate = prepare_plan(spec(96_000, 10_000, vec![cc(0, 4)])).expect("rate plan");
    let mut scheduler = Scheduler::new(32);
    let mut out = [ScheduledEvent::EMPTY; 32];

    let onset = scheduler.schedule(&old, block(1, 0, 1, 48_000), &mut out);
    assert_eq!(onset.written, 1);
    assert_eq!(scheduler.active_count(), 1);

    for (token, cause, plan, expected_active_after) in [
        (2, Discontinuity::Seek, &old, 1),
        (4, Discontinuity::LoopWrap, &old, 1),
        (6, Discontinuity::PlanReplacement, &replacement, 0),
        (8, Discontinuity::SampleRateChange, &new_rate, 0),
    ] {
        if scheduler.active_count() == 0 {
            let rearm = scheduler.schedule(&old, block(token - 1, 0, 1, 48_000), &mut out);
            assert_eq!(rearm.written, 1);
        }
        let sample_rate = plan.sample_rate();
        let result = scheduler.schedule(
            plan,
            HostBlock {
                discontinuity: cause,
                ..block(token, 0, 1, sample_rate)
            },
            &mut out,
        );
        assert!(result.written >= 1);
        assert!(result.synthetic_terminations >= 1);
        assert!(matches!(
            out[0].kind,
            EventKind::NoteOff {
                instance_id: 77,
                ..
            }
        ));
        assert_eq!(scheduler.active_count(), expected_active_after);
    }

    let invalid = scheduler.schedule(&old, block(50, i64::MAX, 16, 48_000), &mut out);
    assert_eq!(invalid.error, Some(CallbackError::FrameRangeOverflow));
}

#[test]
fn replacement_with_reused_instance_id_but_new_identity_terminates_old_note() {
    // Break caught: replacement validity compares only instance ID and preserves a different channel/key.
    let old = prepare_plan(spec(
        48_000,
        20_000,
        vec![note_on(0, 1, 77, 2), note_off(100, 2, 77, 2)],
    ))
    .expect("old identity plan");
    let replacement = prepare_plan(spec(
        48_000,
        20_000,
        vec![note_on(100, 3, 77, 3), note_off(110, 4, 77, 3)],
    ))
    .expect("replacement identity plan");
    let mut scheduler = Scheduler::new(8);
    let mut out = [ScheduledEvent::EMPTY; 8];
    assert_eq!(
        scheduler
            .schedule(&old, block(1, 0, 1, 48_000), &mut out)
            .written,
        1
    );

    let result = scheduler.schedule(
        &replacement,
        HostBlock {
            discontinuity: Discontinuity::PlanReplacement,
            ..block(2, 50, 1, 48_000)
        },
        &mut out,
    );
    assert_eq!(result.error, None);
    assert_eq!(result.synthetic_terminations, 1);
    assert!(matches!(
        out[0].kind,
        EventKind::NoteOff {
            channel: 2,
            instance_id: 77,
            ..
        }
    ));
    assert_eq!(scheduler.active_count(), 0);
}

#[test]
fn one_hundred_loop_wraps_terminate_then_restart_the_loop_note() {
    // Break caught: repeated wraps accumulate duplicate active instances or order onset before termination.
    let plan = prepare_plan(spec(
        48_000,
        20_000,
        vec![note_on(0, 1, 9, 1), note_off(10, 2, 9, 1)],
    ))
    .expect("loop plan");
    let mut scheduler = Scheduler::new(32);
    let mut out = [ScheduledEvent::EMPTY; 8];
    let first = scheduler.schedule(&plan, block(1, 0, 1, 48_000), &mut out);
    assert_eq!(first.written, 1);

    for token in 2..=101 {
        let result = scheduler.schedule(
            &plan,
            HostBlock {
                discontinuity: Discontinuity::LoopWrap,
                ..block(token, 0, 1, 48_000)
            },
            &mut out,
        );
        assert_eq!(result.synthetic_terminations, 1);
        assert_eq!(result.written, 2);
        assert!(matches!(out[0].kind, EventKind::NoteOff { .. }));
        assert!(matches!(out[1].kind, EventKind::NoteOn { .. }));
        assert_eq!(scheduler.active_count(), 1);
    }
}

#[test]
fn termination_saturation_falls_back_per_channel_and_suppresses_note_ons() {
    // Break caught: overload drops terminations to retain notes, or emits more than capacity.
    let mut events = Vec::new();
    for instance in 0_u32..300 {
        events.push(note_on(
            0,
            u64::from(instance) + 1,
            instance + 1,
            (instance % 16) as u8,
        ));
    }
    for instance in 0_u32..300 {
        events.push(note_off(
            10,
            u64::from(instance) + 1_000,
            instance + 1,
            (instance % 16) as u8,
        ));
    }
    let plan = prepare_plan(spec(48_000, 20_000, events)).expect("saturation plan");
    let mut scheduler = Scheduler::new(512);
    let mut wide = [ScheduledEvent::EMPTY; 512];
    let armed = scheduler.schedule(&plan, block(1, 0, 1, 48_000), &mut wide);
    assert_eq!(armed.written, 300);
    assert_eq!(scheduler.active_count(), 300);

    let mut fixed = [ScheduledEvent::EMPTY; 256];
    let wrapped = scheduler.schedule(
        &plan,
        HostBlock {
            discontinuity: Discontinuity::LoopWrap,
            ..block(2, 0, 1, 48_000)
        },
        &mut fixed,
    );
    assert_eq!(wrapped.written, 16);
    assert_eq!(wrapped.synthetic_terminations, 16);
    assert!(
        fixed[..wrapped.written]
            .iter()
            .all(|event| matches!(event.kind, EventKind::AllNotesOff { .. }))
    );
    assert!(scheduler.sticky_overflow());
    assert_eq!(scheduler.active_count(), 0);
    scheduler.clear_overflow_control();
    assert!(!scheduler.sticky_overflow());
}

#[test]
fn ordinary_note_off_overload_falls_back_without_stuck_notes_or_note_on_leakage() {
    // Break caught: retaining only 256 of 300 ordinary NoteOffs leaves 44 instances active.
    let mut events = Vec::new();
    for instance in 1_u32..=300 {
        events.push(note_on(
            0,
            u64::from(instance),
            instance,
            (instance % 16) as u8,
        ));
    }
    for instance in 1_u32..=300 {
        events.push(note_off(
            10,
            u64::from(instance) + 1_000,
            instance,
            (instance % 16) as u8,
        ));
    }
    for instance in 1_u32..=20 {
        events.push(note_on(
            10,
            u64::from(instance) + 2_000,
            instance + 10_000,
            (instance % 16) as u8,
        ));
    }
    events.sort_unstable_by_key(|event| (event.pulse, event.kind.priority(), event.stable_id));
    let plan = prepare_plan(spec(48_000, 20_000, events)).expect("ordinary termination plan");
    let mut scheduler = Scheduler::new(512);
    let mut arm = [ScheduledEvent::EMPTY; 512];
    assert_eq!(
        scheduler
            .schedule(&plan, block(1, 0, 1, 48_000), &mut arm)
            .written,
        300
    );
    assert_eq!(scheduler.active_count(), 300);

    let mut fixed = [ScheduledEvent::EMPTY; 256];
    let result = scheduler.schedule(&plan, block(2, 10, 1, 48_000), &mut fixed);
    assert_eq!(result.error, None);
    assert_eq!(result.synthetic_terminations, 16);
    assert_eq!(result.written, 16);
    assert!(result.overflowed);
    assert!(
        fixed[..result.written]
            .iter()
            .all(|event| matches!(event.kind, EventKind::AllNotesOff { .. }))
    );
    assert_eq!(scheduler.active_count(), 0);
    assert!(scheduler.sticky_overflow());
}

#[test]
fn combined_discontinuity_and_ordinary_terminations_use_one_channel_fallback() {
    // Break caught: counting overlapping active instances once hides 400 termination outputs behind 200 notes.
    let mut events = Vec::new();
    for instance in 1_u32..=200 {
        events.push(note_on(
            0,
            u64::from(instance),
            instance,
            (instance % 16) as u8,
        ));
    }
    for instance in 1_u32..=200 {
        events.push(note_off(
            10,
            u64::from(instance) + 1_000,
            instance,
            (instance % 16) as u8,
        ));
    }
    let plan = prepare_plan(spec(48_000, 20_000, events)).expect("combined termination plan");
    let mut scheduler = Scheduler::new(256);
    let mut output = [ScheduledEvent::EMPTY; 256];
    assert_eq!(
        scheduler
            .schedule(&plan, block(1, 0, 1, 48_000), &mut output)
            .written,
        200
    );

    let result = scheduler.schedule(
        &plan,
        HostBlock {
            discontinuity: Discontinuity::LoopWrap,
            ..block(2, 10, 1, 48_000)
        },
        &mut output,
    );
    assert_eq!(result.synthetic_terminations, 16);
    assert_eq!(result.written, 16);
    assert!(
        output[..16]
            .iter()
            .all(|event| matches!(event.kind, EventKind::AllNotesOff { .. }))
    );
    assert_eq!(scheduler.active_count(), 0);
}

#[test]
fn failed_too_small_termination_output_can_retry_the_same_token() {
    // Break caught: a failed callback consumes its token, so the safe retry is suppressed.
    let plan = prepare_plan(spec(
        48_000,
        20_000,
        vec![
            note_on(0, 1, 1, 0),
            note_on(0, 2, 2, 1),
            note_off(10, 3, 1, 0),
            note_off(10, 4, 2, 1),
        ],
    ))
    .expect("retry plan");
    let mut scheduler = Scheduler::new(8);
    let mut wide = [ScheduledEvent::EMPTY; 8];
    assert_eq!(
        scheduler
            .schedule(&plan, block(1, 0, 1, 48_000), &mut wide)
            .written,
        2
    );
    assert_eq!(scheduler.active_count(), 2);

    let retry_block = HostBlock {
        discontinuity: Discontinuity::Seek,
        ..block(2, 20, 1, 48_000)
    };
    let mut too_small = [ScheduledEvent::EMPTY; 1];
    let failed = scheduler.schedule(&plan, retry_block, &mut too_small);
    assert_eq!(failed.error, Some(CallbackError::TerminationOutputTooSmall));
    assert_eq!(failed.written, 0);
    assert_eq!(scheduler.active_count(), 2);

    let retried = scheduler.schedule(&plan, retry_block, &mut wide);
    assert_eq!(retried.error, None);
    assert_eq!(retried.synthetic_terminations, 2);
    assert_eq!(retried.written, 2);
    assert!(
        wide[..2]
            .iter()
            .all(|event| matches!(event.kind, EventKind::NoteOff { .. }))
    );
    assert_eq!(scheduler.active_count(), 0);
}

#[test]
fn mixed_four_thousand_ninety_six_event_frame_keeps_one_fallback_per_channel() {
    // Break caught: ordinary AllNotesOff duplicates the per-channel fallback or a NoteOn survives it.
    let mut events = Vec::new();
    let mut stable_id = 1_u64;
    for instance in 1_u32..=300 {
        events.push(note_on(-1, stable_id, instance, (instance % 16) as u8));
        stable_id += 1;
    }
    for instance in 1_u32..=300 {
        events.push(note_off(0, stable_id, instance, (instance % 16) as u8));
        stable_id += 1;
    }
    for index in 0_u32..3_796 {
        let kind = match index % 4 {
            0 => EventKind::AllNotesOff {
                channel: (index % 16) as u8,
            },
            1 => EventKind::ControlChange {
                channel: (index % 16) as u8,
                controller: (index % 128) as u8,
                value: 2,
            },
            2 => EventKind::ProgramChange {
                channel: (index % 16) as u8,
                program: (index % 128) as u8,
            },
            _ => EventKind::NoteOn {
                channel: (index % 16) as u8,
                key: (index % 128) as u8,
                velocity: 2,
                instance_id: index + 10_000,
            },
        };
        events.push(Event {
            pulse: 0,
            stable_id,
            kind,
        });
        stable_id += 1;
    }
    events.sort_unstable_by_key(|event| (event.pulse, event.kind.priority(), event.stable_id));
    let plan = prepare_plan(PlanSpec {
        origin_pulse: -1,
        origin_frame: -1,
        sample_rate: 48_000,
        tempo_nodes: vec![TempoNode {
            pulse: -1,
            micros_per_quarter: 20_000,
        }],
        events,
    })
    .expect("mixed saturation plan");
    let mut scheduler = Scheduler::new(2_048);
    let mut wide = [ScheduledEvent::EMPTY; 512];
    assert_eq!(
        scheduler
            .schedule(&plan, block(1, -1, 1, 48_000), &mut wide)
            .written,
        300
    );
    let mut fixed = [ScheduledEvent::EMPTY; 256];
    let result = scheduler.schedule(
        &plan,
        HostBlock {
            discontinuity: Discontinuity::LoopWrap,
            ..block(2, 0, 1, 48_000)
        },
        &mut fixed,
    );
    assert_eq!(result.written, 256);
    assert_eq!(
        fixed[..result.written]
            .iter()
            .filter(|event| matches!(event.kind, EventKind::AllNotesOff { .. }))
            .count(),
        16
    );
    assert!(
        !fixed[..result.written]
            .iter()
            .any(|event| matches!(event.kind, EventKind::NoteOn { .. }))
    );
    assert!(scheduler.sticky_overflow());
}

#[test]
fn skipped_block_then_forward_seek_and_backward_seek_both_terminate_prior_notes() {
    // Break caught: cursor reset assumes contiguous blocks and misses transport-generation cleanup.
    let plan = prepare_plan(spec(
        48_000,
        20_000,
        vec![note_on(0, 1, 1, 0), note_off(100, 2, 1, 0)],
    ))
    .expect("seek plan");
    let mut scheduler = Scheduler::new(8);
    let mut out = [ScheduledEvent::EMPTY; 8];
    assert_eq!(
        scheduler
            .schedule(&plan, block(1, 0, 16, 48_000), &mut out)
            .written,
        1
    );
    let forward = scheduler.schedule(
        &plan,
        HostBlock {
            discontinuity: Discontinuity::Seek,
            ..block(2, 64, 16, 48_000)
        },
        &mut out,
    );
    assert_eq!(forward.synthetic_terminations, 1);
    assert_eq!(scheduler.active_count(), 0);

    assert_eq!(
        scheduler
            .schedule(&plan, block(3, 0, 16, 48_000), &mut out)
            .written,
        1
    );
    let backward = scheduler.schedule(
        &plan,
        HostBlock {
            discontinuity: Discontinuity::Seek,
            ..block(4, 0, 16, 48_000)
        },
        &mut out,
    );
    assert_eq!(backward.synthetic_terminations, 1);
    assert!(matches!(out[0].kind, EventKind::NoteOff { .. }));
}

#[test]
fn ordinary_overload_retains_higher_priority_events() {
    // Break caught: callback simply truncates chronological input and keeps NoteOn over termination/control.
    let mut events = Vec::new();
    for id in 0_u64..4_096 {
        let kind = match id % 5 {
            0 => EventKind::AllNotesOff {
                channel: (id % 16) as u8,
            },
            1 => EventKind::ControlChange {
                channel: (id % 16) as u8,
                controller: (id % 128) as u8,
                value: 1,
            },
            2 => EventKind::ProgramChange {
                channel: (id % 16) as u8,
                program: (id % 128) as u8,
            },
            3 | 4 => EventKind::NoteOn {
                channel: (id % 16) as u8,
                key: (id % 128) as u8,
                velocity: 1,
                instance_id: u32::try_from(id + 1).unwrap(),
            },
            _ => unreachable!(),
        };
        events.push(Event {
            pulse: 0,
            stable_id: id + 1,
            kind,
        });
    }
    events.sort_unstable_by_key(|event| (event.pulse, event.kind.priority(), event.stable_id));
    let plan = prepare_plan(spec(48_000, 20_000, events)).expect("ordinary saturation plan");
    let mut scheduler = Scheduler::new(4_096);
    let mut out = [ScheduledEvent::EMPTY; 256];
    let result = scheduler.schedule(&plan, block(1, 0, 1, 48_000), &mut out);
    assert_eq!(result.written, 256);
    assert!(scheduler.sticky_overflow());
    assert!(
        out.iter()
            .all(|event| matches!(event.kind, EventKind::AllNotesOff { .. }))
    );
}
