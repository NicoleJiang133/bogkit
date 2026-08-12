use midi_scheduler_rt_model::{Event, EventKind, PlanError, PlanSpec, TempoNode, prepare_plan};

fn note_on(pulse: i64, stable_id: u64, instance_id: u32) -> Event {
    Event {
        pulse,
        stable_id,
        kind: EventKind::NoteOn {
            channel: 0,
            key: 60,
            velocity: 100,
            instance_id,
        },
    }
}

fn note_off(pulse: i64, stable_id: u64, instance_id: u32) -> Event {
    Event {
        pulse,
        stable_id,
        kind: EventKind::NoteOff {
            channel: 0,
            key: 60,
            velocity: 0,
            instance_id,
        },
    }
}

fn valid_spec(events: Vec<Event>) -> PlanSpec {
    PlanSpec {
        origin_pulse: 0,
        origin_frame: 0,
        sample_rate: 48_000,
        tempo_nodes: vec![TempoNode {
            pulse: 0,
            micros_per_quarter: 20_000,
        }],
        events,
    }
}

#[test]
fn preparation_uses_checked_exact_segment_accumulation() {
    // Break caught: conversion rounds a segment independently or uses float.
    let spec = PlanSpec {
        origin_pulse: -10,
        origin_frame: 7,
        sample_rate: 48_000,
        tempo_nodes: vec![
            TempoNode {
                pulse: -10,
                micros_per_quarter: 20_000,
            },
            TempoNode {
                pulse: -8,
                micros_per_quarter: 10_000,
            },
        ],
        events: vec![note_on(-8, 1, 10), note_off(-6, 2, 10)],
    };

    let plan = prepare_plan(spec).expect("valid exact plan");
    let frames: Vec<_> = plan.events().iter().map(|event| event.frame).collect();
    assert_eq!(frames, vec![9, 10]);
}

#[test]
fn preparation_accepts_signed_origin_and_reorders_same_frame_by_priority() {
    // Break caught: signed origins overflow, or pulse order wins after pulses collapse to a frame.
    let events = vec![
        note_on(i64::MIN, 2, 22),
        Event {
            pulse: i64::MIN + 1,
            stable_id: 1,
            kind: EventKind::AllNotesOff { channel: 0 },
        },
        note_off(i64::MIN + 2, 3, 22),
    ];
    let spec = PlanSpec {
        origin_pulse: i64::MIN,
        origin_frame: -5,
        sample_rate: 44_100,
        tempo_nodes: vec![TempoNode {
            pulse: i64::MIN,
            micros_per_quarter: 1,
        }],
        events,
    };

    let plan = prepare_plan(spec).expect("minimum signed pulse is admitted");
    assert_eq!(plan.events()[0].kind, EventKind::AllNotesOff { channel: 0 });
    assert_eq!(plan.events()[1].stable_id, 3);
    assert_eq!(plan.events()[2].stable_id, 2);
    assert!(plan.events().iter().all(|event| event.frame == -5));
}

#[test]
fn conversion_accepts_last_frame_and_rejects_one_pulse_above_it() {
    // Break caught: overflow wraps/saturates instead of rejecting preparation.
    let fitting = PlanSpec {
        origin_pulse: 0,
        origin_frame: i64::MAX - 50,
        sample_rate: 48_000,
        tempo_nodes: vec![TempoNode {
            pulse: 0,
            micros_per_quarter: 1_000_000,
        }],
        events: vec![note_on(0, 1, 1), note_off(1, 2, 1)],
    };
    let plan = prepare_plan(fitting).expect("frame at i64::MAX fits");
    assert_eq!(plan.events()[1].frame, i64::MAX);

    let overflowing = PlanSpec {
        origin_pulse: 0,
        origin_frame: i64::MAX - 50,
        sample_rate: 48_000,
        tempo_nodes: vec![TempoNode {
            pulse: 0,
            micros_per_quarter: 1_000_000,
        }],
        events: vec![note_on(0, 1, 1), note_off(2, 2, 1)],
    };
    assert_eq!(prepare_plan(overflowing), Err(PlanError::FrameOverflow));
}

#[test]
fn malformed_plans_return_stable_error_codes() {
    // Break caught: malformed plans publish ambiguous partial state or panic.
    let cases = [
        (
            valid_spec(vec![note_off(1, 1, 99)]),
            PlanError::UnmatchedNoteOff,
        ),
        (
            valid_spec(vec![note_on(0, 1, 5), note_on(1, 2, 5)]),
            PlanError::DuplicateNoteInstance,
        ),
        (
            valid_spec(vec![note_on(0, 1, 5), note_off(1, 2, 5), note_off(2, 3, 5)]),
            PlanError::DuplicateTermination,
        ),
        (
            valid_spec(vec![note_on(2, 2, 5), note_off(1, 1, 5)]),
            PlanError::EventsUnsorted,
        ),
        (
            valid_spec(vec![note_on(0, 7, 5), note_off(1, 7, 5)]),
            PlanError::DuplicateStableId,
        ),
        (
            PlanSpec {
                sample_rate: 47_999,
                ..valid_spec(vec![])
            },
            PlanError::UnsupportedSampleRate,
        ),
        (
            PlanSpec {
                tempo_nodes: vec![TempoNode {
                    pulse: 1,
                    micros_per_quarter: 500_000,
                }],
                ..valid_spec(vec![])
            },
            PlanError::TempoOriginMismatch,
        ),
        (
            PlanSpec {
                tempo_nodes: vec![
                    TempoNode {
                        pulse: 0,
                        micros_per_quarter: 500_000,
                    },
                    TempoNode {
                        pulse: 0,
                        micros_per_quarter: 400_000,
                    },
                ],
                ..valid_spec(vec![])
            },
            PlanError::TempoNodesUnsorted,
        ),
        (
            valid_spec(vec![Event {
                pulse: 0,
                stable_id: 1,
                kind: EventKind::ProgramChange {
                    channel: 16,
                    program: 0,
                },
            }]),
            PlanError::InvalidMidiData,
        ),
    ];

    for (spec, expected) in cases {
        assert_eq!(prepare_plan(spec), Err(expected));
    }
}

#[test]
fn note_off_identity_must_match_its_note_on_channel_and_key() {
    // Break caught: instance ID alone closes a different channel/key identity.
    let channel_mismatch = valid_spec(vec![
        note_on(0, 1, 5),
        Event {
            pulse: 1,
            stable_id: 2,
            kind: EventKind::NoteOff {
                channel: 1,
                key: 60,
                velocity: 0,
                instance_id: 5,
            },
        },
    ]);
    let key_mismatch = valid_spec(vec![
        note_on(0, 1, 5),
        Event {
            pulse: 1,
            stable_id: 2,
            kind: EventKind::NoteOff {
                channel: 0,
                key: 61,
                velocity: 0,
                instance_id: 5,
            },
        },
    ]);

    assert_eq!(
        prepare_plan(channel_mismatch),
        Err(PlanError::TerminationIdentityMismatch)
    );
    assert_eq!(
        prepare_plan(key_mismatch),
        Err(PlanError::TerminationIdentityMismatch)
    );

    let valid = prepare_plan(valid_spec(vec![note_on(0, 1, 5), note_off(1, 2, 5)]))
        .expect("matching termination identity remains valid");
    assert_eq!(valid.events().len(), 2);
}

#[test]
fn consecutive_tempo_nodes_and_note_at_node_use_new_tempo_after_edge() {
    // Break caught: a tempo node is applied one pulse early or late.
    let spec = PlanSpec {
        origin_pulse: 0,
        origin_frame: 0,
        sample_rate: 48_000,
        tempo_nodes: vec![
            TempoNode {
                pulse: 0,
                micros_per_quarter: 20_000,
            },
            TempoNode {
                pulse: 2,
                micros_per_quarter: 40_000,
            },
            TempoNode {
                pulse: 3,
                micros_per_quarter: 10_000,
            },
        ],
        events: vec![note_on(2, 1, 1), note_off(4, 2, 1)],
    };
    let plan = prepare_plan(spec).expect("valid consecutive-node plan");
    assert_eq!(plan.events()[0].frame, 2);
    assert_eq!(plan.events()[1].frame, 4);
}
