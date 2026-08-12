use midi_scheduler_rt_model::{
    Discontinuity, Event, EventKind, FixedHandoff, HostBlock, PlanSpec, ScheduledEvent, Scheduler,
    TempoNode, prepare_plan,
};

#[test]
fn representative_plan_and_forty_five_minute_block_shape_complete_without_overflow() {
    let tempo_nodes = (0_i64..20_000)
        .map(|index| TempoNode {
            pulse: index * 6_480,
            micros_per_quarter: 19_999 + u32::try_from(index % 2).expect("tempo toggle fits"),
        })
        .collect();
    let events = (0_i64..180_000)
        .map(|index| Event {
            pulse: index * 720,
            stable_id: index.cast_unsigned() + 1,
            kind: EventKind::ControlChange {
                channel: u8::try_from(index % 16).expect("channel fits"),
                controller: 1,
                value: u8::try_from(index % 128).expect("value fits"),
            },
        })
        .collect();
    let plan = prepare_plan(PlanSpec {
        origin_pulse: 0,
        origin_frame: 0,
        sample_rate: 48_000,
        tempo_nodes,
        events,
    })
    .expect("representative plan validates");
    assert_eq!(plan.events().len(), 180_000);

    let mut scheduler = Scheduler::new(64);
    let mut output = [ScheduledEvent::EMPTY; 256];
    let sizes = [16_u32, 17, 255, 256, 257, 1_024, 2_048];
    let end_frame = 45_i64 * 60 * 48_000;
    let mut frame = 0_i64;
    let mut token = 1_u64;
    let mut block_index = 0_usize;
    let mut emitted = 0_usize;
    while frame < end_frame {
        let length = sizes[block_index % sizes.len()];
        let result = scheduler.schedule(
            &plan,
            HostBlock {
                token,
                start_frame: frame,
                length,
                sample_rate: 48_000,
                discontinuity: Discontinuity::None,
            },
            &mut output,
        );
        assert_eq!(result.error, None);
        assert!(!result.overflowed);
        emitted += result.written;
        frame += i64::from(length);
        token += 1;
        block_index += 1;
    }
    assert_eq!(emitted, 180_000);
    assert!(block_index > 10_000);

    let mut handoff = FixedHandoff::new();
    handoff.publish(plan.clone()).expect("initial publication");
    for _ in 0..250 {
        handoff
            .publish(plan.clone())
            .expect("replacement publication");
        assert!(handoff.reclaim_retired_control().is_some());
    }
    assert_eq!(handoff.current_generation(), Some(251));
}
