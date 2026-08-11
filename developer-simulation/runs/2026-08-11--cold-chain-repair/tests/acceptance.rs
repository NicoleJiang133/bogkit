use cold_chain_repair::{
    Config, Observation, Record, candidate_from_records, canonical_json, generated_fixture,
    read_ndjson, reference_from_records, shuffled, write_ndjson,
};

fn config(id: &str, freezer: &str, at: i64, min: i32, max: i32) -> Record {
    Record::Configuration(Config {
        configuration_id: id.into(),
        freezer_id: freezer.into(),
        effective_at: at,
        min_temp_tenths: min,
        max_temp_tenths: max,
    })
}

fn obs(id: &str, freezer: &str, at: i64, temp: i32) -> Record {
    Record::Observation(Observation {
        observation_id: id.into(),
        freezer_id: freezer.into(),
        observed_at: at,
        received_at: at + 21_600,
        temp_tenths: temp,
        gateway_sequence: u64::try_from(at).expect("test timestamp is non-negative"),
        replaces_observation_id: None,
    })
}

#[test]
fn five_minute_boundary_opens_with_exact_provenance() {
    let mut records = vec![config("cfg-a", "f-1", 0, -220, -180)];
    for step in 0..=10 {
        records.push(obs(&format!("o-{step}"), "f-1", step * 30, -170));
    }

    let export = reference_from_records(&records).expect("valid fixture");
    assert_eq!(export.freezers[0].incidents.len(), 1);
    let opened = &export.freezers[0].incidents[0].opened;
    assert_eq!(opened.at, 300);
    assert_eq!(opened.trigger_observation_id, "o-10");
    assert_eq!(opened.contributing_observation_ids.len(), 11);
    assert_eq!(opened.configuration_ids, vec!["cfg-a"]);
}

#[test]
fn ten_minute_boundary_closes_and_exports_canonical_bytes() {
    let mut records = vec![config("cfg-a", "f-1", 0, -220, -180)];
    for step in 0..=10 {
        records.push(obs(&format!("bad-{step}"), "f-1", step * 30, -170));
    }
    for step in 1..=21 {
        records.push(obs(&format!("good-{step}"), "f-1", 300 + step * 30, -200));
    }

    let export = reference_from_records(&records).expect("valid fixture");
    let closed = export.freezers[0].incidents[0]
        .closed
        .as_ref()
        .expect("closed incident");
    assert_eq!(closed.at, 930);
    assert_eq!(closed.trigger_observation_id, "good-21");
    assert_eq!(closed.contributing_observation_ids.len(), 21);
    assert_eq!(canonical_json(&export).unwrap().last(), Some(&b'\n'));
}

#[test]
fn correction_and_backdated_configuration_repair_only_affected_freezer() {
    let mut before = vec![
        config("cfg-a", "f-1", 0, -220, -180),
        config("cfg-b", "f-2", 0, -220, -180),
    ];
    for step in 0..=10 {
        before.push(obs(&format!("a-{step}"), "f-1", step * 30, -170));
        before.push(obs(&format!("b-{step}"), "f-2", step * 30, -170));
    }
    let original = reference_from_records(&before).unwrap();

    let mut after = before.clone();
    let mut correction = obs("a-fix", "f-1", 150, -200);
    let Record::Observation(ref mut corrected) = correction else {
        unreachable!()
    };
    corrected.replaces_observation_id = Some("a-5".into());
    corrected.received_at = 150 + 7 * 86_400;
    after.push(correction);
    after.push(config("cfg-a-maint", "f-1", 240, -220, -160));

    let repaired = reference_from_records(&after).unwrap();
    assert_ne!(original.freezers[0], repaired.freezers[0]);
    assert_eq!(original.freezers[1], repaired.freezers[1]);
}

#[test]
fn duplicates_are_idempotent_and_candidate_matches_reference() {
    let records = vec![
        config("cfg-a", "f-1", 0, -220, -180),
        obs("o-1", "f-1", 0, -200),
        obs("o-2", "f-1", 30, -170),
    ];
    let mut repeated = records.clone();
    repeated.extend(records.clone());

    let reference = reference_from_records(&records).unwrap();
    assert_eq!(reference, reference_from_records(&repeated).unwrap());

    let dir = std::env::temp_dir().join(format!(
        "cold-chain-acceptance-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let candidate = candidate_from_records(&repeated, &dir).unwrap();
    assert_eq!(candidate, reference);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_records_are_rejected_as_a_batch() {
    let unknown_correction = Record::Observation(Observation {
        observation_id: "fix".into(),
        freezer_id: "f-1".into(),
        observed_at: 10,
        received_at: 20,
        temp_tenths: -200,
        gateway_sequence: 2,
        replaces_observation_id: Some("missing".into()),
    });
    let invalid = vec![config("cfg-a", "f-1", 0, -220, -180), unknown_correction];
    let error = reference_from_records(&invalid).unwrap_err().to_string();
    assert!(error.contains("unknown correction target"));

    let no_band = vec![
        config("bad-band", "f-1", 0, -180, -220),
        obs("o", "f-1", 1, -200),
    ];
    assert!(
        reference_from_records(&no_band)
            .unwrap_err()
            .to_string()
            .contains("invalid temperature band")
    );
}

#[test]
fn one_hundred_seeds_match_across_order_and_repeated_batches() {
    let root = std::env::temp_dir().join(format!(
        "cold-chain-differential-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&root);

    for seed in 0..100_u64 {
        let chronological = generated_fixture(seed, 3, 96);
        let expected = reference_from_records(&chronological).unwrap();

        let shuffled_records = shuffled(&chronological, seed ^ 0x5eed_5eed);
        let shuffled_path = root.join(format!("seed-{seed}-shuffled"));
        assert_eq!(
            candidate_from_records(&shuffled_records, &shuffled_path).unwrap(),
            expected,
            "shuffled seed {seed}"
        );

        let mut repeated = shuffled_records.clone();
        repeated.extend(shuffled_records.iter().take(37).cloned());
        let repeated_path = root.join(format!("seed-{seed}-repeated"));
        assert_eq!(
            candidate_from_records(&repeated, &repeated_path).unwrap(),
            expected,
            "repeated seed {seed}"
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ndjson_round_trip_and_malformed_timestamp_error_name_the_line() {
    let records = generated_fixture(5, 2, 32);
    let mut bytes = Vec::new();
    write_ndjson(&records, &mut bytes).unwrap();
    assert_eq!(read_ndjson(bytes.as_slice()).unwrap(), records);

    let malformed = br#"{"observation":{"observation_id":"o","freezer_id":"f","observed_at":"yesterday","received_at":1,"temp_tenths":-200,"gateway_sequence":1,"replaces_observation_id":null}}
"#;
    let error = read_ndjson(malformed.as_slice()).unwrap_err().to_string();
    assert!(error.contains("line 1"));
    assert!(error.contains("observed_at"));
}
