use midi_scheduler_rt_model::oracle::{OraclePlan, OracleRunner};
use midi_scheduler_rt_model::{
    Discontinuity, Event, EventKind, HostBlock, PlanSpec, ScheduledEvent, Scheduler, TempoNode,
    prepare_plan,
};

#[derive(Clone, Copy)]
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn below(&mut self, upper: u64) -> u64 {
        self.next() % upper
    }
}

fn trace_spec(seed: u64) -> PlanSpec {
    let seed_index = usize::try_from(seed).expect("test seed fits usize");
    let sample_rate = [44_100, 48_000, 96_000][seed_index % 3];
    let mut random = Lcg(seed ^ 0x8a5c_2137_d95e_42ad);
    let origin_pulse = -200;
    let tempo_nodes = vec![
        TempoNode {
            pulse: origin_pulse,
            micros_per_quarter: 15_000
                + u32::try_from(random.below(20_000)).expect("bounded tempo fits"),
        },
        TempoNode {
            pulse: -50,
            micros_per_quarter: 10_000
                + u32::try_from(random.below(30_000)).expect("bounded tempo fits"),
        },
        TempoNode {
            pulse: -49,
            micros_per_quarter: 10_000
                + u32::try_from(random.below(30_000)).expect("bounded tempo fits"),
        },
        TempoNode {
            pulse: 120,
            micros_per_quarter: 10_000
                + u32::try_from(random.below(30_000)).expect("bounded tempo fits"),
        },
    ];
    let mut events = Vec::new();
    let mut stable_id = 1_u64;
    for pair in 0_u32..30 {
        let on_pulse = -190
            + i64::from(pair) * 12
            + i64::try_from(random.below(4)).expect("bounded pulse fits");
        events.push(Event {
            pulse: on_pulse,
            stable_id,
            kind: EventKind::NoteOn {
                channel: (pair % 16) as u8,
                key: u8::try_from(40 + pair % 40).expect("fixture key fits"),
                velocity: 90,
                instance_id: pair + 1,
            },
        });
        stable_id += 1;
        events.push(Event {
            pulse: on_pulse + 5 + i64::try_from(random.below(5)).expect("bounded pulse fits"),
            stable_id,
            kind: EventKind::NoteOff {
                channel: (pair % 16) as u8,
                key: u8::try_from(40 + pair % 40).expect("fixture key fits"),
                velocity: 0,
                instance_id: pair + 1,
            },
        });
        stable_id += 1;
    }
    for index in 0_u64..140 {
        let pulse = origin_pulse + i64::try_from(random.below(500)).expect("bounded pulse fits");
        let kind = match index % 3 {
            0 => EventKind::ControlChange {
                channel: (index % 16) as u8,
                controller: (index % 128) as u8,
                value: u8::try_from(random.below(128)).expect("bounded MIDI byte fits"),
            },
            1 => EventKind::ProgramChange {
                channel: (index % 16) as u8,
                program: u8::try_from(random.below(128)).expect("bounded MIDI byte fits"),
            },
            _ => EventKind::AllNotesOff {
                channel: (index % 16) as u8,
            },
        };
        events.push(Event {
            pulse,
            stable_id,
            kind,
        });
        stable_id += 1;
    }
    // Exact tempo-edge fixtures are present in every seed.
    for pulse in [-50, -49, 120] {
        events.push(Event {
            pulse,
            stable_id,
            kind: EventKind::ControlChange {
                channel: 15,
                controller: 127,
                value: 1,
            },
        });
        stable_id += 1;
    }
    events.sort_unstable_by_key(|event| (event.pulse, event.kind.priority(), event.stable_id));
    PlanSpec {
        origin_pulse,
        origin_frame: -97,
        sample_rate,
        tempo_nodes,
        events,
    }
}

fn run_seed(seed: u64) -> Vec<(u64, u32, EventKind, u64)> {
    let seed_index = usize::try_from(seed).expect("test seed fits usize");
    let spec = trace_spec(seed);
    let oracle_plan = OraclePlan::from_spec(&spec).expect("oracle conversion");
    let plan = prepare_plan(spec).expect("scheduler preparation");
    let mut scheduler = Scheduler::new(128);
    let mut oracle = OracleRunner::new();
    let mut output = [ScheduledEvent::EMPTY; 512];
    let sizes = [16_u32, 17, 255, 256, 257, 1_024, 2_048];
    let first_frame = plan.events().first().expect("events").frame - 3;
    let last_frame = plan.events().last().expect("events").frame + 4;
    let mut start = first_frame;
    let mut token = seed * 10_000 + 1;
    let mut actual_log = Vec::new();
    let mut expected_log = Vec::new();
    let mut block_index = 0_usize;

    while start < last_frame {
        let length = sizes[(block_index + seed_index) % sizes.len()];
        let host = HostBlock {
            token,
            start_frame: start,
            length,
            sample_rate: plan.sample_rate(),
            discontinuity: Discontinuity::None,
        };
        let actual = scheduler.schedule(&plan, host, &mut output);
        assert_eq!(actual.error, None);
        assert!(!actual.overflowed);
        actual_log.extend(
            output[..actual.written]
                .iter()
                .map(|event| (token, event.frame_offset, event.kind, event.stable_id)),
        );
        expected_log.extend(
            oracle
                .schedule(&oracle_plan, host)
                .into_iter()
                .map(|event| (token, event.frame_offset, event.kind, event.stable_id)),
        );

        if block_index.is_multiple_of(3) {
            let replay = scheduler.schedule(&plan, host, &mut output);
            assert!(replay.duplicate_token);
            assert!(oracle.schedule(&oracle_plan, host).is_empty());
        }
        start += i64::from(length);
        token += 1;
        block_index += 1;
    }
    assert_eq!(actual_log, expected_log, "seed {seed}");
    actual_log
}

#[test]
fn one_hundred_seeded_traces_match_the_separate_rational_oracle() {
    // Break caught: any conversion, half-open selection, ordering, or token replay differs from the oracle.
    for seed in 0..100 {
        let _ = run_seed(seed);
    }
}

#[test]
fn five_runs_have_byte_identical_event_digests() {
    // Break caught: hidden process state or unstable ordering changes a representative trace.
    let mut digests = [0_u64; 5];
    for digest in &mut digests {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for (token, offset, kind, stable_id) in run_seed(73) {
            for value in [
                token,
                u64::from(offset),
                u64::from(kind.priority()),
                stable_id,
            ] {
                for byte in value.to_le_bytes() {
                    hash ^= u64::from(byte);
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
        }
        *digest = hash;
    }
    assert!(digests.iter().all(|digest| *digest == digests[0]));
}
