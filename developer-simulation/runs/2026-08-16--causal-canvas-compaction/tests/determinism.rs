use std::collections::BTreeMap;

use causal_canvas_compaction::{Core, Operation, Payload, run_reduced_oracle_suite, stable_digest};

#[test]
fn durable_digest_uses_sha256_known_vector() {
    // Catches replacing corruption/identity digests with a small non-cryptographic
    // checksum. This literal is the independently published SHA-256 vector for abc.
    assert_eq!(
        stable_digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn generated_histories_match_hand_derived_oracle_across_schedules() {
    // Catches schedule-sensitive scalar resolution, causal buffer drain, and
    // duplicate delivery changing the canonical bytes or suite digest.
    let first = run_reduced_oracle_suite(5, 3, 8);
    let repeated = run_reduced_oracle_suite(5, 3, 8);
    assert_eq!(first.histories, 5);
    assert_eq!(first.schedules_per_history, 3);
    assert_eq!(first.comparisons, 15);
    assert_eq!(first.failures, 0);
    assert_eq!(first.digest, repeated.digest);
}

#[test]
fn twenty_input_batchings_produce_identical_snapshot_and_manifest_bytes() {
    // Catches transaction/batch-local ordering leaking into durable canonical
    // state. The literal operations are reused, but expected equality is across
    // independently chosen boundaries, not computed by the implementation.
    let payloads = [
        Payload::Create {
            object_id: "shape".to_string(),
            object_kind: "rectangle".to_string(),
        },
        Payload::SetProperty {
            object_id: "shape".to_string(),
            key: "fill".to_string(),
            value: "blue".to_string(),
        },
        Payload::Move {
            object_id: "shape".to_string(),
            x: 12,
            y: 34,
        },
        Payload::ListInsert {
            list_id: "layers".to_string(),
            element_id: "shape-element".to_string(),
            left: None,
            value: "shape".to_string(),
        },
    ];
    let operations: Vec<_> = payloads
        .into_iter()
        .enumerate()
        .map(|(index, payload)| {
            let sequence = index as u64 + 1;
            Operation {
                id: format!("actor-{sequence}"),
                document_id: "canvas".to_string(),
                actor_id: "actor".to_string(),
                actor_sequence: sequence,
                dependency_clock: if sequence == 1 {
                    BTreeMap::new()
                } else {
                    BTreeMap::from([("actor".to_string(), sequence - 1)])
                },
                accepted_minute: sequence,
                payload,
            }
        })
        .collect();

    let mut outputs = Vec::new();
    for schedule in 0..20 {
        let batch_size = schedule % operations.len() + 1;
        let mut core = Core::new();
        for batch in operations.chunks(batch_size) {
            core.apply_batch(batch.iter().cloned());
        }
        let artifact = core.compact("canvas", &[], 10).unwrap();
        outputs.push((artifact.snapshot_json, artifact.manifest_json));
    }
    assert!(outputs.windows(2).all(|pair| pair[0] == pair[1]));
}
