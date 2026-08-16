use std::collections::BTreeMap;

use super::{Core, Operation, Payload, stable_digest};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuiteSummary {
    pub histories: usize,
    pub schedules_per_history: usize,
    pub comparisons: usize,
    pub failures: usize,
    pub digest: String,
}

#[must_use]
pub fn run_reduced_oracle_suite(
    history_count: usize,
    schedules_per_history: usize,
    actor_count: usize,
) -> SuiteSummary {
    let actor_count = actor_count.max(1);
    let mut failures = 0;
    let mut digest_material = Vec::new();

    for history in 0..history_count {
        let document_id = format!("doc-{history:03}");
        let operations = generated_scalar_history(&document_id, actor_count);
        let winner = format!("actor-{:02}", actor_count - 1);
        let expected = format!(
            "{{\"document_id\":\"{document_id}\",\"objects\":[{{\"id\":\"shape\",\"kind\":\"rectangle\",\"x\":0,\"y\":0,\"properties\":{{\"fill\":\"value-{winner}\"}}}}],\"lists\":[]}}"
        );
        for schedule in 0..schedules_per_history {
            let mut delivery = operations.clone();
            shuffle(
                &mut delivery,
                (history as u64 + 1) * 1_000_003 + schedule as u64,
            );
            let duplicate_count = delivery.len() * 8 / 100;
            let duplicates: Vec<_> = delivery.iter().take(duplicate_count).cloned().collect();
            delivery.extend(duplicates);
            if schedule == 0 {
                delivery.reverse();
            }

            let mut core = Core::new();
            for operation in delivery {
                core.apply(operation);
            }
            match core.canonical_snapshot(&document_id) {
                Ok(snapshot) => {
                    if snapshot != expected {
                        failures += 1;
                    }
                    digest_material.extend_from_slice(snapshot.as_bytes());
                }
                Err(_) => failures += 1,
            }
        }
    }

    SuiteSummary {
        histories: history_count,
        schedules_per_history,
        comparisons: history_count.saturating_mul(schedules_per_history),
        failures,
        digest: stable_digest(&digest_material),
    }
}

fn generated_scalar_history(document_id: &str, actor_count: usize) -> Vec<Operation> {
    let mut operations = vec![Operation {
        id: "creator-1".to_string(),
        document_id: document_id.to_string(),
        actor_id: "creator".to_string(),
        actor_sequence: 1,
        dependency_clock: BTreeMap::new(),
        accepted_minute: 1,
        payload: Payload::Create {
            object_id: "shape".to_string(),
            object_kind: "rectangle".to_string(),
        },
    }];
    for actor in 0..actor_count {
        let actor_id = format!("actor-{actor:02}");
        operations.push(Operation {
            id: format!("{actor_id}-1"),
            document_id: document_id.to_string(),
            actor_id: actor_id.clone(),
            actor_sequence: 1,
            dependency_clock: BTreeMap::from([("creator".to_string(), 1)]),
            accepted_minute: actor as u64 + 2,
            payload: Payload::SetProperty {
                object_id: "shape".to_string(),
                key: "fill".to_string(),
                value: format!("value-{actor_id}"),
            },
        });
    }
    operations
}

fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut state = seed | 1;
    for index in (1..items.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let modulus = u64::try_from(index + 1).unwrap_or(u64::MAX);
        let selected = usize::try_from(state % modulus).unwrap_or(0);
        items.swap(index, selected);
    }
}
