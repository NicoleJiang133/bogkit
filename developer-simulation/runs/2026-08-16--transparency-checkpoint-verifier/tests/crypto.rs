mod common;

use common::hex32;
use ring::digest::{SHA256, digest};
use transparency_checkpoint_verifier::{
    ErrorCode, root_from_leaf_hashes, verify_consistency, verify_ed25519,
};

#[test]
fn matches_independently_generated_merkle_roots_at_required_sizes() {
    let vectors = [
        (
            0,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            1,
            "3e7077fd2f66d689e0cee6a7cf5b37bf2dca7c979af356d0a31cbc5c85605c7d",
        ),
        (
            2,
            "a7d91894b61fbf46378d88e3e1b1f7aef39532c504b484bd31551d15e0a09dff",
        ),
        (
            3,
            "9b4965f8b220ba42f7039ad0781c966cf90bb1aea15a80586d634b322ab1f4ce",
        ),
        (
            4,
            "b15d2b1b07adada9b13b555c08062b1ae78ad1b0b7e99d97d942c936a6244439",
        ),
        (
            7,
            "45cea7edca9543ee5575a5774d0d8fa9321a8be084b3fb657fa4f6d071a3c94c",
        ),
        (
            8,
            "b15acd8b1ccf7a9b81c04f69b27e5cabd67e90be0e6ff6a4d1ed87004a4f0cc1",
        ),
        (
            15,
            "249600a1a23dbe4d31fa0694264d54846e587dd4a490bc90a4d386979d7fccd6",
        ),
        (
            16,
            "d5057b535a9b33119a6344d8cbf16c5a8f1541c106aa9ee50c6c93260664222d",
        ),
        (
            65_535,
            "e7321cd57abf36c522d89e6d26bd9098a37b6a8b06b318976269e701f6d4802d",
        ),
        (
            65_536,
            "bc9bca909516d5bf148c2bf2dea0a0d9f717ce4987d2fe7504d6694363b41598",
        ),
    ];
    let mut leaves = Vec::with_capacity(65_536);
    for i in 0_u64..65_536 {
        let mut input = [0_u8; 9];
        input[1..].copy_from_slice(&i.to_be_bytes());
        leaves.push(digest(&SHA256, &input).as_ref().try_into().unwrap());
    }
    for (size, expected) in vectors {
        assert_eq!(
            root_from_leaf_hashes(&leaves[..size]),
            hex32(expected),
            "size {size}"
        );
    }
}

#[test]
fn verifies_hand_checked_consistency_vector_and_rejects_every_bit_flip() {
    let old_root = hex32("9b4965f8b220ba42f7039ad0781c966cf90bb1aea15a80586d634b322ab1f4ce");
    let new_root = hex32("b15d2b1b07adada9b13b555c08062b1ae78ad1b0b7e99d97d942c936a6244439");
    let proof = [
        hex32("d81f51781eeb8f46a0e112e86ca335896ccc12cee00e6cfcf58a3501129dfc24"),
        hex32("feebf1863bd1fceedfeff2693829d50ffbcac100d0fbe745482e032f93f6bafb"),
        hex32("a7d91894b61fbf46378d88e3e1b1f7aef39532c504b484bd31551d15e0a09dff"),
    ];
    assert_eq!(verify_consistency(3, 4, old_root, new_root, &proof), Ok(()));

    for node in 0..proof.len() {
        for bit in 0..256 {
            let mut corrupt = proof;
            corrupt[node][bit / 8] ^= 1 << (bit % 8);
            assert_eq!(
                verify_consistency(3, 4, old_root, new_root, &corrupt),
                Err(ErrorCode::ProofInvalid)
            );
        }
    }
    for bit in 0..256 {
        let mut corrupt = new_root;
        corrupt[bit / 8] ^= 1 << (bit % 8);
        assert_eq!(
            verify_consistency(3, 4, old_root, corrupt, &proof),
            Err(ErrorCode::ProofInvalid)
        );
    }
}

#[test]
fn rejects_impossible_sizes_and_excess_proofs() {
    let h = [1_u8; 32];
    assert_eq!(
        verify_consistency(4, 3, h, h, &[]),
        Err(ErrorCode::SizeRollback)
    );
    assert_eq!(
        verify_consistency(0, 1, h, h, &[[0; 32]]),
        Err(ErrorCode::ProofInvalid)
    );
    assert_eq!(verify_consistency(u64::MAX, u64::MAX, h, h, &[]), Ok(()));
    assert_eq!(
        verify_consistency(1, 2, h, h, &[[0; 32]; 129]),
        Err(ErrorCode::ParseProofCount)
    );
}

#[test]
fn verifies_rfc8032_vector_and_rejects_each_signature_bit_flip() {
    let public = hex32("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c");
    let message = [0x72_u8];
    let signature = hex64(
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    );
    assert_eq!(verify_ed25519(&public, &message, &signature), Ok(()));
    for bit in 0..512 {
        let mut corrupt = signature;
        corrupt[bit / 8] ^= 1 << (bit % 8);
        assert_eq!(
            verify_ed25519(&public, &message, &corrupt),
            Err(ErrorCode::SignatureInvalid)
        );
    }
}

fn hex64(s: &str) -> [u8; 64] {
    let mut out = [0_u8; 64];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = (nibble(s.as_bytes()[2 * i]) << 4) | nibble(s.as_bytes()[2 * i + 1]);
    }
    out
}

fn nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        _ => panic!("invalid test fixture"),
    }
}
