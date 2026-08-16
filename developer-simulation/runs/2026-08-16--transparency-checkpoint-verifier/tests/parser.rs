mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use common::{ROOT_3, hex32, unsigned_envelope};
use transparency_checkpoint_verifier::{ErrorCode, parse_envelope};

#[test]
fn parses_fields_and_preserves_exact_proof_and_envelope_bytes() {
    let proof = [
        hex32("d81f51781eeb8f46a0e112e86ca335896ccc12cee00e6cfcf58a3501129dfc24"),
        hex32("feebf1863bd1fceedfeff2693829d50ffbcac100d0fbe745482e032f93f6bafb"),
        hex32("a7d91894b61fbf46378d88e3e1b1f7aef39532c504b484bd31551d15e0a09dff"),
    ];
    let raw = unsigned_envelope("log-a", "key-1", 3, 4, ROOT_3, &proof);

    let parsed = parse_envelope(&raw).expect("valid characterization envelope");

    assert_eq!(parsed.log_id, "log-a");
    assert_eq!(parsed.key_id, "key-1");
    assert_eq!(parsed.base_size, 3);
    assert_eq!(parsed.tree_size, 4);
    assert_eq!(parsed.proof, proof);
    assert_eq!(parsed.raw_proof, proof.concat());
    assert_eq!(parsed.raw_envelope, raw);
}

#[test]
fn rejects_oversized_or_excessive_proof_declarations_before_allocation() {
    let mut too_many = unsigned_envelope("log-a", "key-1", 0, 1, ROOT_3, &[]);
    let count_offset = too_many.len() - 64 - 2 - 8;
    too_many[count_offset..count_offset + 8].copy_from_slice(&129_u64.to_be_bytes());
    assert_eq!(
        parse_envelope(&too_many).unwrap_err(),
        ErrorCode::ParseProofCount
    );

    too_many[count_offset..count_offset + 8].copy_from_slice(&u64::MAX.to_be_bytes());
    assert_eq!(
        parse_envelope(&too_many).unwrap_err(),
        ErrorCode::ParseProofCount
    );

    let oversized = vec![0_u8; 4_097];
    assert_eq!(
        parse_envelope(&oversized).unwrap_err(),
        ErrorCode::ParseEnvelopeSize
    );
}

#[test]
fn truncation_at_every_byte_is_bounded_and_never_panics() {
    let raw = unsigned_envelope("log-a", "key-1", 3, 4, ROOT_3, &[[9_u8; 32]]);
    for end in 0..raw.len() {
        let result = catch_unwind(AssertUnwindSafe(|| parse_envelope(&raw[..end])));
        assert!(result.is_ok(), "panic at truncation {end}");
        assert!(
            result.expect("no panic").is_err(),
            "accepted truncation {end}"
        );
    }
}

#[test]
fn malformed_4k_inputs_finish_without_panicking() {
    for seed in 0_u8..=31 {
        let mut raw = vec![seed; 4_096];
        raw[0..4].copy_from_slice(b"CKP1");
        let result = catch_unwind(AssertUnwindSafe(|| parse_envelope(&raw)));
        assert!(result.is_ok());
    }
}

#[test]
fn rejects_ten_thousand_deterministic_adversarial_characterization_cases() {
    let valid = unsigned_envelope("log-a", "key-1", 0, 1, ROOT_3, &[]);
    let count_offset = valid.len() - 64 - 2 - 8;
    for case in 0..10_000 {
        let mut raw = valid.clone();
        match case % 5 {
            0 => raw.truncate(case % raw.len()),
            1 => raw[count_offset..count_offset + 8].copy_from_slice(&129_u64.to_be_bytes()),
            2 => raw[case % 4] ^= 1 << (case % 8),
            3 => raw.resize(4_097, u8::try_from(case % 251).unwrap()),
            _ => raw[53] = 0,
        }
        let started = std::time::Instant::now();
        assert!(
            parse_envelope(&raw).is_err(),
            "accepted synthetic case {case}"
        );
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "case {case} exceeded 50ms"
        );
    }
}
