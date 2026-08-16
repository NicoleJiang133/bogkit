#![allow(dead_code)]

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};

pub const ROOT_0: [u8; 32] =
    hex32("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
pub const ROOT_1: [u8; 32] =
    hex32("3e7077fd2f66d689e0cee6a7cf5b37bf2dca7c979af356d0a31cbc5c85605c7d");
pub const ROOT_2: [u8; 32] =
    hex32("a7d91894b61fbf46378d88e3e1b1f7aef39532c504b484bd31551d15e0a09dff");
pub const ROOT_3: [u8; 32] =
    hex32("9b4965f8b220ba42f7039ad0781c966cf90bb1aea15a80586d634b322ab1f4ce");
pub const ROOT_4: [u8; 32] =
    hex32("b15d2b1b07adada9b13b555c08062b1ae78ad1b0b7e99d97d942c936a6244439");

pub const fn hex32(s: &str) -> [u8; 32] {
    let bytes = s.as_bytes();
    let mut out = [0_u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nibble(bytes[2 * i]) << 4) | nibble(bytes[2 * i + 1]);
        i += 1;
    }
    out
}

const fn nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        _ => panic!("invalid fixture hex"),
    }
}

pub struct TestSigner {
    pair: Ed25519KeyPair,
}

impl TestSigner {
    pub fn new() -> Self {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("test key");
        Self {
            pair: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("parse test key"),
        }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.pair
            .public_key()
            .as_ref()
            .try_into()
            .expect("Ed25519 key length")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn envelope(
        &self,
        log_id: &str,
        key_id: &str,
        base_size: u64,
        size: u64,
        root: [u8; 32],
        proof: &[[u8; 32]],
        timestamp_ms: u64,
    ) -> Vec<u8> {
        let mut out = signed_prefix(log_id, key_id, size, root, timestamp_ms);
        let signature = self.pair.sign(&out);
        out.extend_from_slice(&base_size.to_be_bytes());
        out.extend_from_slice(&(proof.len() as u64).to_be_bytes());
        for node in proof {
            out.extend_from_slice(node);
        }
        let signature_len = u16::try_from(signature.as_ref().len()).expect("short test signature");
        out.extend_from_slice(&signature_len.to_be_bytes());
        out.extend_from_slice(signature.as_ref());
        out
    }
}

pub fn unsigned_envelope(
    log_id: &str,
    key_id: &str,
    base_size: u64,
    size: u64,
    root: [u8; 32],
    proof: &[[u8; 32]],
) -> Vec<u8> {
    let mut out = signed_prefix(log_id, key_id, size, root, 1_700_000_000_000);
    out.extend_from_slice(&base_size.to_be_bytes());
    out.extend_from_slice(&(proof.len() as u64).to_be_bytes());
    for node in proof {
        out.extend_from_slice(node);
    }
    out.extend_from_slice(&64_u16.to_be_bytes());
    out.extend_from_slice(&[7_u8; 64]);
    out
}

fn signed_prefix(
    log_id: &str,
    key_id: &str,
    size: u64,
    root: [u8; 32],
    timestamp_ms: u64,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"CKP1");
    out.push(1);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(&root);
    out.extend_from_slice(&timestamp_ms.to_be_bytes());
    out.push(u8::try_from(log_id.len()).expect("short test identity"));
    out.push(u8::try_from(key_id.len()).expect("short test key id"));
    out.extend_from_slice(log_id.as_bytes());
    out.extend_from_slice(key_id.as_bytes());
    out
}
