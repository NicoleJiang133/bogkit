use midi_scheduler_rt_model::{
    Event, EventKind, FixedHandoff, HandoffError, PlanError, PlanSpec, TempoNode, prepare_plan,
};

fn plan(stable_id: u64) -> midi_scheduler_rt_model::PreparedPlan {
    prepare_plan(PlanSpec {
        origin_pulse: 0,
        origin_frame: 0,
        sample_rate: 48_000,
        tempo_nodes: vec![TempoNode {
            pulse: 0,
            micros_per_quarter: 20_000,
        }],
        events: vec![Event {
            pulse: 0,
            stable_id,
            kind: EventKind::ControlChange {
                channel: 0,
                controller: 1,
                value: (stable_id % 128) as u8,
            },
        }],
    })
    .expect("valid plan")
}

#[test]
fn rejected_plan_does_not_mutate_published_generation_or_digest() {
    // Break caught: a failed preparation partially publishes or advances generation.
    let mut handoff = FixedHandoff::new();
    let first = handoff.publish(plan(1)).expect("first publication");
    let before = (handoff.current_generation(), handoff.current_digest());
    let invalid = prepare_plan(PlanSpec {
        origin_pulse: 0,
        origin_frame: 0,
        sample_rate: 12_345,
        tempo_nodes: vec![TempoNode {
            pulse: 0,
            micros_per_quarter: 20_000,
        }],
        events: vec![],
    });
    assert_eq!(invalid, Err(PlanError::UnsupportedSampleRate));
    assert_eq!(
        (handoff.current_generation(), handoff.current_digest()),
        before
    );
    assert_eq!(first.generation, before.0.expect("published generation"));
}

#[test]
fn reader_that_started_before_publish_observes_one_complete_old_generation() {
    // Break caught: generation comes from one slot while digest/plan comes from another.
    let mut handoff = FixedHandoff::new();
    let old = handoff.publish(plan(11)).expect("old publication");
    let ticket = handoff.begin_callback().expect("old read ticket");
    let new = handoff.publish(plan(22)).expect("new publication");
    assert_ne!(old.digest, new.digest);

    let observation = handoff
        .observe(ticket)
        .expect("retired slot remains pinned");
    assert_eq!(
        (observation.generation, observation.digest),
        (old.generation, old.digest)
    );
    assert_eq!(handoff.reclaim_retired_control(), None);
    handoff.end_callback(ticket).expect("reader release");
    let reclaimed = handoff
        .reclaim_retired_control()
        .expect("old plan reclaimed after release");
    assert_eq!(reclaimed.generation, old.generation);
    assert_eq!(handoff.reclaim_retired_control(), None);
}

#[test]
fn deterministic_handoff_model_covers_ten_thousand_before_and_after_reads() {
    // Break caught: a retired slot is reused early, lost, or reclaimed more than once.
    let mut handoff = FixedHandoff::new();
    let mut current = handoff.publish(plan(1)).expect("initial publication");
    let mut reclaimed = 0_u64;

    for replacement in 0_u64..10_000 {
        if replacement % 2 == 0 {
            // Publication immediately after callback captured the generation.
            let ticket = handoff.begin_callback().expect("read before publish");
            let next = handoff
                .publish(plan(replacement + 2))
                .expect("publish into free slot");
            let observed = handoff.observe(ticket).expect("pinned old observation");
            assert_eq!(
                (observed.generation, observed.digest),
                (current.generation, current.digest)
            );
            assert_eq!(handoff.reclaim_retired_control(), None);
            handoff.end_callback(ticket).expect("end old read");
            current = next;
        } else {
            // Publication immediately before callback captures the generation.
            let next = handoff
                .publish(plan(replacement + 2))
                .expect("publish before read");
            let ticket = handoff.begin_callback().expect("read new publication");
            let observed = handoff.observe(ticket).expect("complete new observation");
            assert_eq!(
                (observed.generation, observed.digest),
                (next.generation, next.digest)
            );
            handoff.end_callback(ticket).expect("end new read");
            current = next;
        }
        let old = handoff
            .reclaim_retired_control()
            .expect("exactly one retired generation");
        assert!(old.generation < current.generation);
        reclaimed += 1;
        assert_eq!(handoff.reclaim_retired_control(), None);
    }

    assert_eq!(reclaimed, 10_000);
    assert_eq!(handoff.current_generation(), Some(current.generation));
}

#[test]
fn stale_or_double_ended_tickets_return_errors_instead_of_panicking() {
    // Break caught: invalid handoff tokens underflow a reader count or index freed storage.
    let mut handoff = FixedHandoff::new();
    handoff.publish(plan(1)).expect("publication");
    let ticket = handoff.begin_callback().expect("ticket");
    handoff.end_callback(ticket).expect("first end");
    assert_eq!(
        handoff.end_callback(ticket),
        Err(HandoffError::ReaderAlreadyEnded)
    );
}
