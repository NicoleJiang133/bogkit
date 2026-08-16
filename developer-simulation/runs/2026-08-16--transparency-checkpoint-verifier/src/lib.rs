#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use ring::digest::{SHA256, digest};
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};

pub type Hash = [u8; 32];

const MAX_ENVELOPE_BYTES: usize = 4_096;
const MAX_PROOF_HASHES: u64 = 128;
const MAX_PENDING_PER_LOG: usize = 16;
const ED25519_ALGORITHM: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    Advanced,
    Duplicate,
    Equivocation,
    PendingBase,
    PendingLimit,
    ParseEnvelopeSize,
    ParseTruncated,
    ParseMagic,
    ParseIdentity,
    ParseProofCount,
    ParseTrailingBytes,
    UnknownLog,
    UnknownAlgorithm,
    UnknownKey,
    SignatureInvalid,
    ProofInvalid,
    SizeRollback,
    ArchiveCursorRollback,
    BaseMismatch,
    StoreIo,
    StoreCorrupt,
    InjectedReturnedError,
}

#[derive(Debug, Clone)]
pub struct ParsedEnvelope {
    pub algorithm: u8,
    pub tree_size: u64,
    pub root: Hash,
    pub timestamp_ms: u64,
    pub log_id: String,
    pub key_id: String,
    pub base_size: u64,
    pub proof: Vec<Hash>,
    pub signature: Vec<u8>,
    pub signed_bytes: Vec<u8>,
    pub raw_proof: Vec<u8>,
    pub raw_envelope: Vec<u8>,
}

/// Parses the prototype's characterized envelope, rejecting all declared
/// lengths before allocating proof or signature storage.
///
/// # Errors
///
/// Returns a stable parse code when the envelope is oversized, truncated,
/// malformed, or declares a proof beyond the admission limit.
pub fn parse_envelope(raw: &[u8]) -> Result<ParsedEnvelope, ErrorCode> {
    if raw.len() > MAX_ENVELOPE_BYTES {
        return Err(ErrorCode::ParseEnvelopeSize);
    }

    let mut cursor = Cursor::new(raw);
    if cursor.take(4)? != b"CKP1" {
        return Err(ErrorCode::ParseMagic);
    }
    let algorithm = cursor.u8()?;
    let tree_size = cursor.u64()?;
    let root = cursor.hash()?;
    let timestamp_ms = cursor.u64()?;
    let log_len = usize::from(cursor.u8()?);
    let key_len = usize::from(cursor.u8()?);
    if !(1..=64).contains(&log_len) || !(1..=64).contains(&key_len) {
        return Err(ErrorCode::ParseIdentity);
    }
    let log_id = std::str::from_utf8(cursor.take(log_len)?)
        .map_err(|_| ErrorCode::ParseIdentity)?
        .to_owned();
    let key_id = std::str::from_utf8(cursor.take(key_len)?)
        .map_err(|_| ErrorCode::ParseIdentity)?
        .to_owned();
    let signed_end = cursor.position();
    let base_size = cursor.u64()?;
    let proof_count = cursor.u64()?;
    if proof_count > MAX_PROOF_HASHES {
        return Err(ErrorCode::ParseProofCount);
    }
    let proof_len = usize::try_from(proof_count)
        .ok()
        .and_then(|count| count.checked_mul(32))
        .ok_or(ErrorCode::ParseProofCount)?;
    let proof_bytes = cursor.take(proof_len)?;
    let proof = proof_bytes
        .chunks_exact(32)
        .map(|node| {
            let mut hash = [0_u8; 32];
            hash.copy_from_slice(node);
            hash
        })
        .collect();
    let signature_len = usize::from(cursor.u16()?);
    if signature_len > 512 {
        return Err(ErrorCode::ParseEnvelopeSize);
    }
    let signature = cursor.take(signature_len)?.to_vec();
    if cursor.position() != raw.len() {
        return Err(ErrorCode::ParseTrailingBytes);
    }

    Ok(ParsedEnvelope {
        algorithm,
        tree_size,
        root,
        timestamp_ms,
        log_id,
        key_id,
        base_size,
        proof,
        signature,
        signed_bytes: raw[..signed_end].to_vec(),
        raw_proof: proof_bytes.to_vec(),
        raw_envelope: raw.to_vec(),
    })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    const fn position(&self) -> usize {
        self.at
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], ErrorCode> {
        let end = self
            .at
            .checked_add(count)
            .ok_or(ErrorCode::ParseTruncated)?;
        let value = self
            .bytes
            .get(self.at..end)
            .ok_or(ErrorCode::ParseTruncated)?;
        self.at = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, ErrorCode> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ErrorCode> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("two bytes requested"),
        ))
    }

    fn u64(&mut self) -> Result<u64, ErrorCode> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("eight bytes requested"),
        ))
    }

    fn hash(&mut self) -> Result<Hash, ErrorCode> {
        Ok(self
            .take(32)?
            .try_into()
            .expect("thirty-two bytes requested"))
    }
}

#[derive(Debug, Clone, Default)]
pub struct TreeBuilder {
    count: u64,
    frontier: Vec<Option<Hash>>,
}

impl TreeBuilder {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            count: 0,
            frontier: Vec::new(),
        }
    }

    /// Adds one already-domain-separated 32-byte leaf hash.
    ///
    /// # Errors
    ///
    /// Returns `ProofInvalid` if the leaf count overflows or the frontier is
    /// internally inconsistent.
    pub fn append_leaf_hash(&mut self, mut node: Hash) -> Result<(), ErrorCode> {
        let next = self.count.checked_add(1).ok_or(ErrorCode::ProofInvalid)?;
        let mut occupied = self.count;
        let mut level = 0;
        while occupied & 1 == 1 {
            let left = self
                .frontier
                .get_mut(level)
                .and_then(Option::take)
                .ok_or(ErrorCode::ProofInvalid)?;
            node = hash_node(left, node);
            occupied >>= 1;
            level += 1;
        }
        if level == self.frontier.len() {
            self.frontier.push(Some(node));
        } else {
            self.frontier[level] = Some(node);
        }
        self.count = next;
        Ok(())
    }

    #[must_use]
    pub fn root(&self) -> Hash {
        let mut root = None;
        for node in self.frontier.iter().flatten() {
            root = Some(match root {
                None => *node,
                Some(right) => hash_node(*node, right),
            });
        }
        root.unwrap_or_else(empty_root)
    }

    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }
}

#[must_use]
pub fn root_from_leaf_hashes(leaves: &[Hash]) -> Hash {
    let mut builder = TreeBuilder::new();
    for leaf in leaves {
        if builder.append_leaf_hash(*leaf).is_err() {
            return empty_root();
        }
    }
    builder.root()
}

#[must_use]
pub fn empty_root() -> Hash {
    let mut hash = [0_u8; 32];
    hash.copy_from_slice(digest(&SHA256, &[]).as_ref());
    hash
}

fn hash_node(left: Hash, right: Hash) -> Hash {
    let mut input = [0_u8; 65];
    input[0] = 1;
    input[1..33].copy_from_slice(&left);
    input[33..].copy_from_slice(&right);
    let mut hash = [0_u8; 32];
    hash.copy_from_slice(digest(&SHA256, &input).as_ref());
    hash
}

/// Verifies an RFC 6962-style binary Merkle-tree consistency proof.
///
/// # Errors
///
/// Returns a stable size, admission, or proof code if the transition cannot
/// establish that the new root extends the old tree.
pub fn verify_consistency(
    old_size: u64,
    new_size: u64,
    old_root: Hash,
    new_root: Hash,
    proof: &[Hash],
) -> Result<(), ErrorCode> {
    if proof.len() > 128 {
        return Err(ErrorCode::ParseProofCount);
    }
    if old_size > new_size {
        return Err(ErrorCode::SizeRollback);
    }
    if old_size == new_size {
        return if old_root == new_root && proof.is_empty() {
            Ok(())
        } else {
            Err(ErrorCode::ProofInvalid)
        };
    }
    if old_size == 0 {
        return if old_root == empty_root() && proof.is_empty() {
            Ok(())
        } else {
            Err(ErrorCode::ProofInvalid)
        };
    }

    let mut old_path = old_size - 1;
    let mut new_path = new_size - 1;
    while old_path & 1 == 1 {
        old_path >>= 1;
        new_path >>= 1;
    }

    let (mut old_hash, mut new_hash, start) = if old_size.is_power_of_two() {
        (old_root, old_root, 0)
    } else {
        let first = *proof.first().ok_or(ErrorCode::ProofInvalid)?;
        (first, first, 1)
    };

    for node in &proof[start..] {
        if new_path == 0 {
            return Err(ErrorCode::ProofInvalid);
        }
        if old_path & 1 == 1 || old_path == new_path {
            old_hash = hash_node(*node, old_hash);
            new_hash = hash_node(*node, new_hash);
            while old_path != 0 && old_path & 1 == 0 {
                old_path >>= 1;
                new_path >>= 1;
            }
        } else {
            new_hash = hash_node(new_hash, *node);
        }
        old_path >>= 1;
        new_path >>= 1;
    }

    if new_path == 0 && old_hash == old_root && new_hash == new_root {
        Ok(())
    } else {
        Err(ErrorCode::ProofInvalid)
    }
}

/// Verifies an Ed25519 signature over the exact supplied message bytes.
///
/// # Errors
///
/// Returns `SignatureInvalid` for any malformed key/signature or failed
/// verification.
pub fn verify_ed25519(
    public_key: &[u8; 32],
    message: &[u8],
    signature: &[u8],
) -> Result<(), ErrorCode> {
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(message, signature)
        .map_err(|_| ErrorCode::SignatureInvalid)
}

#[derive(Debug, Clone)]
pub struct KnownLog {
    pub log_id: String,
    pub key_id: String,
    pub algorithm: u8,
    pub public_key: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct Registry {
    logs: BTreeMap<String, KnownLog>,
}

impl Registry {
    /// Constructs a fixed log/key registry.
    ///
    /// # Errors
    ///
    /// Returns `ParseIdentity` for empty or duplicate log identities.
    pub fn new(logs: Vec<KnownLog>) -> Result<Self, ErrorCode> {
        let mut by_id = BTreeMap::new();
        for log in logs {
            if log.log_id.is_empty()
                || log.key_id.is_empty()
                || by_id.insert(log.log_id.clone(), log).is_some()
            {
                return Err(ErrorCode::ParseIdentity);
            }
        }
        Ok(Self { logs: by_id })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentCheckpoint {
    pub tree_size: u64,
    pub root: Hash,
    pub archive_cursor: u64,
    pub raw_proof: Vec<u8>,
    pub raw_envelope: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Equivocation {
    pub log_id: String,
    pub tree_size: u64,
    pub first_root: Hash,
    pub second_root: Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub log_id: String,
    pub code: ErrorCode,
    pub base_size: u64,
    pub tree_size: u64,
    pub root: Hash,
    pub archive_cursor: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingCheckpoint {
    pub base_size: u64,
    pub tree_size: u64,
    pub root: Hash,
    pub archive_cursor: u64,
    pub envelope_id: Hash,
    pub raw_envelope: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EngineSnapshot {
    pub logs: BTreeMap<String, CurrentCheckpoint>,
    pub pending: BTreeMap<String, Vec<PendingCheckpoint>>,
    pub decisions: Vec<DecisionRecord>,
    pub equivocations: Vec<Equivocation>,
    pub seen_envelopes: BTreeSet<Hash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    pub code: ErrorCode,
    pub advanced_sizes: Vec<u64>,
}

pub struct Engine {
    registry: Registry,
    snapshot: EngineSnapshot,
}

impl Engine {
    #[must_use]
    pub fn new(registry: Registry) -> Self {
        let logs = registry
            .logs
            .keys()
            .map(|log_id| {
                (
                    log_id.clone(),
                    CurrentCheckpoint {
                        tree_size: 0,
                        root: empty_root(),
                        archive_cursor: 0,
                        raw_proof: Vec::new(),
                        raw_envelope: Vec::new(),
                    },
                )
            })
            .collect();
        Self {
            registry,
            snapshot: EngineSnapshot {
                logs,
                ..EngineSnapshot::default()
            },
        }
    }

    /// Restores a verifier from a persisted snapshot.
    ///
    /// # Errors
    ///
    /// Returns `StoreCorrupt` when registry/log correspondence, signatures,
    /// exact envelope identities, or deterministic snapshot invariants fail.
    pub fn from_snapshot(registry: Registry, snapshot: EngineSnapshot) -> Result<Self, ErrorCode> {
        validate_snapshot(&registry, &snapshot)?;
        Ok(Self { registry, snapshot })
    }

    pub fn submit(&mut self, raw: &[u8], archive_cursor: u64) -> SubmitOutcome {
        let parsed = match parse_envelope(raw) {
            Ok(parsed) => parsed,
            Err(code) => return outcome(code),
        };
        let Some(known) = self.registry.logs.get(&parsed.log_id) else {
            return outcome(ErrorCode::UnknownLog);
        };
        if parsed.algorithm != ED25519_ALGORITHM || parsed.algorithm != known.algorithm {
            return outcome(ErrorCode::UnknownAlgorithm);
        }
        if parsed.key_id != known.key_id {
            return outcome(ErrorCode::UnknownKey);
        }
        if let Err(code) =
            verify_ed25519(&known.public_key, &parsed.signed_bytes, &parsed.signature)
        {
            return outcome(code);
        }

        let envelope_id = sha256(raw);
        if self.snapshot.seen_envelopes.contains(&envelope_id)
            || self
                .snapshot
                .pending
                .get(&parsed.log_id)
                .is_some_and(|pending| pending.iter().any(|item| item.raw_envelope == raw))
        {
            return outcome(ErrorCode::Duplicate);
        }

        self.process_verified(&parsed, archive_cursor, envelope_id, true)
    }

    fn process_verified(
        &mut self,
        parsed: &ParsedEnvelope,
        archive_cursor: u64,
        envelope_id: Hash,
        drain: bool,
    ) -> SubmitOutcome {
        let current = self
            .snapshot
            .logs
            .get(&parsed.log_id)
            .expect("registry and initialized logs agree")
            .clone();

        if archive_cursor < current.archive_cursor {
            return outcome(ErrorCode::ArchiveCursorRollback);
        }

        if parsed.tree_size < current.tree_size {
            return outcome(ErrorCode::SizeRollback);
        }
        if parsed.tree_size == current.tree_size {
            if parsed.root == current.root {
                self.snapshot.seen_envelopes.insert(envelope_id);
                return outcome(ErrorCode::Duplicate);
            }
            let alert = Equivocation {
                log_id: parsed.log_id.clone(),
                tree_size: parsed.tree_size,
                first_root: current.root,
                second_root: parsed.root,
            };
            if !self.snapshot.equivocations.contains(&alert) {
                self.snapshot.equivocations.push(alert);
                self.snapshot.equivocations.sort_by(equivocation_order);
            }
            self.record(parsed, archive_cursor, ErrorCode::Equivocation);
            self.snapshot.seen_envelopes.insert(envelope_id);
            return outcome(ErrorCode::Equivocation);
        }
        if parsed.base_size < current.tree_size {
            return outcome(ErrorCode::BaseMismatch);
        }
        if parsed.base_size > current.tree_size {
            let pending = self
                .snapshot
                .pending
                .entry(parsed.log_id.clone())
                .or_default();
            if pending.len() >= MAX_PENDING_PER_LOG {
                return outcome(ErrorCode::PendingLimit);
            }
            pending.push(PendingCheckpoint {
                base_size: parsed.base_size,
                tree_size: parsed.tree_size,
                root: parsed.root,
                archive_cursor,
                envelope_id,
                raw_envelope: parsed.raw_envelope.clone(),
            });
            pending.sort_by(pending_order);
            self.record(parsed, archive_cursor, ErrorCode::PendingBase);
            return outcome(ErrorCode::PendingBase);
        }

        if let Err(code) = verify_consistency(
            current.tree_size,
            parsed.tree_size,
            current.root,
            parsed.root,
            &parsed.proof,
        ) {
            self.record(parsed, archive_cursor, code);
            return outcome(code);
        }

        let log_id = parsed.log_id.clone();
        let size = parsed.tree_size;
        self.snapshot.logs.insert(
            log_id.clone(),
            CurrentCheckpoint {
                tree_size: parsed.tree_size,
                root: parsed.root,
                archive_cursor,
                raw_proof: parsed.raw_proof.clone(),
                raw_envelope: parsed.raw_envelope.clone(),
            },
        );
        self.record(parsed, archive_cursor, ErrorCode::Advanced);
        self.snapshot.seen_envelopes.insert(envelope_id);

        let mut advanced_sizes = vec![size];
        if drain {
            self.drain_pending(&log_id, &mut advanced_sizes);
        }
        SubmitOutcome {
            code: ErrorCode::Advanced,
            advanced_sizes,
        }
    }

    fn drain_pending(&mut self, log_id: &str, advanced: &mut Vec<u64>) {
        loop {
            let current_size = self.snapshot.logs.get(log_id).expect("known log").tree_size;
            let next = self.snapshot.pending.get_mut(log_id).and_then(|items| {
                let index = items
                    .iter()
                    .position(|item| item.base_size <= current_size)?;
                Some(items.remove(index))
            });
            let Some(next) = next else {
                break;
            };
            let Ok(parsed) = parse_envelope(&next.raw_envelope) else {
                continue;
            };
            let result =
                self.process_verified(&parsed, next.archive_cursor, next.envelope_id, false);
            if result.code == ErrorCode::Advanced {
                advanced.extend(result.advanced_sizes);
            } else {
                self.finish_removed_pending(
                    &parsed,
                    next.archive_cursor,
                    next.envelope_id,
                    result.code,
                );
            }
        }
    }

    fn finish_removed_pending(
        &mut self,
        parsed: &ParsedEnvelope,
        archive_cursor: u64,
        envelope_id: Hash,
        code: ErrorCode,
    ) {
        match code {
            ErrorCode::ProofInvalid => {
                self.snapshot.seen_envelopes.insert(envelope_id);
            }
            ErrorCode::ArchiveCursorRollback
            | ErrorCode::BaseMismatch
            | ErrorCode::SizeRollback => {
                self.record(parsed, archive_cursor, code);
                self.snapshot.seen_envelopes.insert(envelope_id);
            }
            _ => {}
        }
    }

    fn record(&mut self, parsed: &ParsedEnvelope, archive_cursor: u64, code: ErrorCode) {
        self.snapshot.decisions.push(DecisionRecord {
            log_id: parsed.log_id.clone(),
            code,
            base_size: parsed.base_size,
            tree_size: parsed.tree_size,
            root: parsed.root,
            archive_cursor,
        });
        self.snapshot.decisions.sort_by(decision_order);
    }

    #[must_use]
    pub fn current(&self, log_id: &str) -> Option<&CurrentCheckpoint> {
        self.snapshot.logs.get(log_id)
    }

    #[must_use]
    pub fn snapshot(&self) -> EngineSnapshot {
        self.snapshot.clone()
    }

    #[must_use]
    pub fn stable_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.snapshot).unwrap_or_default()
    }
}

fn validate_snapshot(registry: &Registry, snapshot: &EngineSnapshot) -> Result<(), ErrorCode> {
    if !registry.logs.keys().eq(snapshot.logs.keys()) {
        return Err(ErrorCode::StoreCorrupt);
    }

    for (log_id, current) in &snapshot.logs {
        let known = registry.logs.get(log_id).ok_or(ErrorCode::StoreCorrupt)?;
        if current.tree_size == 0 {
            if current.root != empty_root()
                || current.archive_cursor != 0
                || !current.raw_proof.is_empty()
                || !current.raw_envelope.is_empty()
            {
                return Err(ErrorCode::StoreCorrupt);
            }
            continue;
        }
        let parsed = parse_envelope(&current.raw_envelope).map_err(|_| ErrorCode::StoreCorrupt)?;
        validate_known_envelope(known, &parsed)?;
        if parsed.log_id != *log_id
            || parsed.tree_size != current.tree_size
            || parsed.root != current.root
            || parsed.base_size >= parsed.tree_size
            || parsed.raw_proof != current.raw_proof
            || !snapshot
                .seen_envelopes
                .contains(&sha256(&current.raw_envelope))
        {
            return Err(ErrorCode::StoreCorrupt);
        }
    }

    let mut pending_ids = BTreeSet::new();
    for (log_id, items) in &snapshot.pending {
        let known = registry.logs.get(log_id).ok_or(ErrorCode::StoreCorrupt)?;
        let current = snapshot.logs.get(log_id).ok_or(ErrorCode::StoreCorrupt)?;
        if items.len() > MAX_PENDING_PER_LOG
            || !items.is_sorted_by(|left, right| pending_order(left, right).is_le())
        {
            return Err(ErrorCode::StoreCorrupt);
        }
        for item in items {
            let parsed = parse_envelope(&item.raw_envelope).map_err(|_| ErrorCode::StoreCorrupt)?;
            validate_known_envelope(known, &parsed)?;
            if parsed.log_id != *log_id
                || parsed.base_size != item.base_size
                || parsed.tree_size != item.tree_size
                || parsed.root != item.root
                || parsed.tree_size <= parsed.base_size
                || item.base_size <= current.tree_size
                || item.archive_cursor < current.archive_cursor
                || item.envelope_id != sha256(&item.raw_envelope)
                || snapshot.seen_envelopes.contains(&item.envelope_id)
                || !pending_ids.insert(item.envelope_id)
            {
                return Err(ErrorCode::StoreCorrupt);
            }
        }
    }

    if !snapshot
        .decisions
        .iter()
        .all(|decision| registry.logs.contains_key(&decision.log_id))
        || !snapshot
            .decisions
            .is_sorted_by(|left, right| decision_order(left, right).is_le())
        || !snapshot.equivocations.iter().all(|alert| {
            registry.logs.contains_key(&alert.log_id) && alert.first_root != alert.second_root
        })
        || !snapshot
            .equivocations
            .is_sorted_by(|left, right| equivocation_order(left, right).is_le())
        || snapshot
            .equivocations
            .windows(2)
            .any(|pair| pair[0] == pair[1])
    {
        return Err(ErrorCode::StoreCorrupt);
    }

    Ok(())
}

fn validate_known_envelope(known: &KnownLog, parsed: &ParsedEnvelope) -> Result<(), ErrorCode> {
    if parsed.log_id != known.log_id
        || parsed.key_id != known.key_id
        || parsed.algorithm != ED25519_ALGORITHM
        || parsed.algorithm != known.algorithm
        || verify_ed25519(&known.public_key, &parsed.signed_bytes, &parsed.signature).is_err()
    {
        Err(ErrorCode::StoreCorrupt)
    } else {
        Ok(())
    }
}

fn outcome(code: ErrorCode) -> SubmitOutcome {
    SubmitOutcome {
        code,
        advanced_sizes: Vec::new(),
    }
}

fn decision_order(left: &DecisionRecord, right: &DecisionRecord) -> std::cmp::Ordering {
    (
        &left.log_id,
        left.tree_size,
        left.base_size,
        left.root,
        left.code,
        left.archive_cursor,
    )
        .cmp(&(
            &right.log_id,
            right.tree_size,
            right.base_size,
            right.root,
            right.code,
            right.archive_cursor,
        ))
}

fn pending_order(left: &PendingCheckpoint, right: &PendingCheckpoint) -> std::cmp::Ordering {
    (
        left.base_size,
        left.tree_size,
        left.root,
        left.archive_cursor,
        left.envelope_id,
        &left.raw_envelope,
    )
        .cmp(&(
            right.base_size,
            right.tree_size,
            right.root,
            right.archive_cursor,
            right.envelope_id,
            &right.raw_envelope,
        ))
}

fn equivocation_order(left: &Equivocation, right: &Equivocation) -> std::cmp::Ordering {
    (
        &left.log_id,
        left.tree_size,
        left.first_root,
        left.second_root,
    )
        .cmp(&(
            &right.log_id,
            right.tree_size,
            right.first_root,
            right.second_root,
        ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationStage {
    StateWrite,
    ProofWrite,
    DecisionWrite,
    Flush,
    GenerationRename,
    PointerWrite,
    PointerFlush,
    PointerRename,
    DirectorySync,
}

impl PublicationStage {
    pub const ALL: [Self; 9] = [
        Self::StateWrite,
        Self::ProofWrite,
        Self::DecisionWrite,
        Self::Flush,
        Self::GenerationRename,
        Self::PointerWrite,
        Self::PointerFlush,
        Self::PointerRename,
        Self::DirectorySync,
    ];
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    state_sha256: String,
    proofs_sha256: String,
    decisions_digest: String,
}

#[derive(Debug, Serialize)]
struct ProofArchive<'a> {
    current: BTreeMap<&'a str, (&'a [u8], &'a [u8])>,
    pending: BTreeMap<&'a str, Vec<&'a [u8]>>,
}

#[derive(Debug)]
pub struct DirectoryStore {
    root: PathBuf,
}

impl DirectoryStore {
    #[must_use]
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            root: path.as_ref().to_path_buf(),
        }
    }

    /// Publishes one deterministic generation and atomically switches the
    /// visible generation pointer.
    ///
    /// # Errors
    ///
    /// Returns a stable store code for serialization or I/O, or one generic
    /// in-process returned error after the requested stage has completed.
    pub fn publish(
        &self,
        snapshot: &EngineSnapshot,
        returned_error_after: Option<PublicationStage>,
    ) -> Result<(), ErrorCode> {
        fs::create_dir_all(&self.root).map_err(|_| ErrorCode::StoreIo)?;
        let state = serde_json::to_vec(snapshot).map_err(|_| ErrorCode::StoreCorrupt)?;
        let proofs = proof_bytes(snapshot)?;
        let decisions = decision_bytes(snapshot)?;
        let manifest = serde_json::to_vec(&Manifest {
            state_sha256: hex(&sha256(&state)),
            proofs_sha256: hex(&sha256(&proofs)),
            decisions_digest: hex(&sha256(&decisions)),
        })
        .map_err(|_| ErrorCode::StoreCorrupt)?;
        let mut generation_material =
            Vec::with_capacity(state.len() + proofs.len() + decisions.len());
        generation_material.extend_from_slice(&state);
        generation_material.extend_from_slice(&proofs);
        generation_material.extend_from_slice(&decisions);
        let generation = format!("gen-{}", hex(&sha256(&generation_material)));
        let temporary = self.root.join(format!(".tmp-{generation}"));
        let complete = self.root.join(&generation);

        if temporary.exists() {
            fs::remove_dir_all(&temporary).map_err(|_| ErrorCode::StoreIo)?;
        }
        if complete.exists() {
            validate_generation_exact(&complete, &state, &proofs, &decisions, &manifest)?;
            inject(returned_error_after, PublicationStage::GenerationRename)?;
        } else {
            fs::create_dir(&temporary).map_err(|_| ErrorCode::StoreIo)?;
            write_file(&temporary.join("state.json"), &state)?;
            inject(returned_error_after, PublicationStage::StateWrite)?;
            write_file(&temporary.join("proofs.json"), &proofs)?;
            inject(returned_error_after, PublicationStage::ProofWrite)?;
            write_file(&temporary.join("decisions.jsonl"), &decisions)?;
            inject(returned_error_after, PublicationStage::DecisionWrite)?;
            write_file(&temporary.join("manifest.json"), &manifest)?;
            for name in [
                "state.json",
                "proofs.json",
                "decisions.jsonl",
                "manifest.json",
            ] {
                File::open(temporary.join(name))
                    .and_then(|file| file.sync_all())
                    .map_err(|_| ErrorCode::StoreIo)?;
            }
            inject(returned_error_after, PublicationStage::Flush)?;
            fs::rename(&temporary, &complete).map_err(|_| ErrorCode::StoreIo)?;
            inject(returned_error_after, PublicationStage::GenerationRename)?;
        }

        let pointer_tmp = self.root.join("CURRENT.next");
        write_file(&pointer_tmp, generation.as_bytes())?;
        inject(returned_error_after, PublicationStage::PointerWrite)?;
        File::open(&pointer_tmp)
            .and_then(|file| file.sync_all())
            .map_err(|_| ErrorCode::StoreIo)?;
        inject(returned_error_after, PublicationStage::PointerFlush)?;
        fs::rename(&pointer_tmp, self.root.join("CURRENT")).map_err(|_| ErrorCode::StoreIo)?;
        inject(returned_error_after, PublicationStage::PointerRename)?;
        sync_directory(&self.root)?;
        inject(returned_error_after, PublicationStage::DirectorySync)?;
        Ok(())
    }

    /// Reopens and digest-checks the complete visible generation.
    ///
    /// # Errors
    ///
    /// Returns `StoreIo` when no generation is readable and `StoreCorrupt`
    /// when any payload or manifest fails validation.
    pub fn reopen(&self) -> Result<EngineSnapshot, ErrorCode> {
        let VisibleParts {
            state,
            proofs,
            decisions,
            manifest,
            ..
        } = self.visible_parts()?;
        validate_manifest(&state, &proofs, &decisions, &manifest)?;
        let snapshot: EngineSnapshot =
            serde_json::from_slice(&state).map_err(|_| ErrorCode::StoreCorrupt)?;
        if proof_bytes(&snapshot)? != proofs || decision_bytes(&snapshot)? != decisions {
            return Err(ErrorCode::StoreCorrupt);
        }
        Ok(snapshot)
    }

    /// Returns a canonical concatenation of the visible pointer and files.
    ///
    /// # Errors
    ///
    /// Returns a stable store code if the visible generation is incomplete.
    pub fn visible_bytes(&self) -> Result<Vec<u8>, ErrorCode> {
        let VisibleParts {
            generation,
            state,
            proofs,
            decisions,
            manifest,
        } = self.visible_parts()?;
        let mut all = generation.into_bytes();
        all.push(b'\n');
        for part in [state, proofs, decisions, manifest] {
            all.extend_from_slice(&part);
            all.push(b'\n');
        }
        Ok(all)
    }

    fn visible_parts(&self) -> Result<VisibleParts, ErrorCode> {
        let generation =
            fs::read_to_string(self.root.join("CURRENT")).map_err(|_| ErrorCode::StoreIo)?;
        let generation = generation.trim().to_owned();
        if generation.is_empty() || generation.contains('/') || generation.contains("..") {
            return Err(ErrorCode::StoreCorrupt);
        }
        let dir = self.root.join(&generation);
        Ok(VisibleParts {
            generation,
            state: fs::read(dir.join("state.json")).map_err(|_| ErrorCode::StoreCorrupt)?,
            proofs: fs::read(dir.join("proofs.json")).map_err(|_| ErrorCode::StoreCorrupt)?,
            decisions: fs::read(dir.join("decisions.jsonl"))
                .map_err(|_| ErrorCode::StoreCorrupt)?,
            manifest: fs::read(dir.join("manifest.json")).map_err(|_| ErrorCode::StoreCorrupt)?,
        })
    }
}

struct VisibleParts {
    generation: String,
    state: Vec<u8>,
    proofs: Vec<u8>,
    decisions: Vec<u8>,
    manifest: Vec<u8>,
}

fn validate_generation_exact(
    directory: &Path,
    expected_state: &[u8],
    expected_proofs: &[u8],
    expected_decisions: &[u8],
    expected_manifest: &[u8],
) -> Result<(), ErrorCode> {
    let names: BTreeSet<String> = fs::read_dir(directory)
        .map_err(|_| ErrorCode::StoreCorrupt)?
        .map(|entry| {
            entry
                .map_err(|_| ErrorCode::StoreCorrupt)?
                .file_name()
                .into_string()
                .map_err(|_| ErrorCode::StoreCorrupt)
        })
        .collect::<Result<_, _>>()?;
    let expected_names = [
        "decisions.jsonl",
        "manifest.json",
        "proofs.json",
        "state.json",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    if names != expected_names {
        return Err(ErrorCode::StoreCorrupt);
    }

    let state = fs::read(directory.join("state.json")).map_err(|_| ErrorCode::StoreCorrupt)?;
    let proofs = fs::read(directory.join("proofs.json")).map_err(|_| ErrorCode::StoreCorrupt)?;
    let decisions =
        fs::read(directory.join("decisions.jsonl")).map_err(|_| ErrorCode::StoreCorrupt)?;
    let manifest =
        fs::read(directory.join("manifest.json")).map_err(|_| ErrorCode::StoreCorrupt)?;
    validate_manifest(&state, &proofs, &decisions, &manifest)?;
    if state != expected_state
        || proofs != expected_proofs
        || decisions != expected_decisions
        || manifest != expected_manifest
    {
        return Err(ErrorCode::StoreCorrupt);
    }
    Ok(())
}

fn validate_manifest(
    state: &[u8],
    proofs: &[u8],
    decisions: &[u8],
    manifest: &[u8],
) -> Result<(), ErrorCode> {
    let manifest: Manifest =
        serde_json::from_slice(manifest).map_err(|_| ErrorCode::StoreCorrupt)?;
    if manifest.state_sha256 != hex(&sha256(state))
        || manifest.proofs_sha256 != hex(&sha256(proofs))
        || manifest.decisions_digest != hex(&sha256(decisions))
    {
        Err(ErrorCode::StoreCorrupt)
    } else {
        Ok(())
    }
}

fn proof_bytes(snapshot: &EngineSnapshot) -> Result<Vec<u8>, ErrorCode> {
    let current = snapshot
        .logs
        .iter()
        .map(|(log, checkpoint)| {
            (
                log.as_str(),
                (
                    checkpoint.raw_proof.as_slice(),
                    checkpoint.raw_envelope.as_slice(),
                ),
            )
        })
        .collect();
    let pending = snapshot
        .pending
        .iter()
        .map(|(log, items)| {
            (
                log.as_str(),
                items
                    .iter()
                    .map(|item| item.raw_envelope.as_slice())
                    .collect(),
            )
        })
        .collect();
    serde_json::to_vec(&ProofArchive { current, pending }).map_err(|_| ErrorCode::StoreCorrupt)
}

fn decision_bytes(snapshot: &EngineSnapshot) -> Result<Vec<u8>, ErrorCode> {
    let mut bytes = Vec::new();
    for decision in &snapshot.decisions {
        serde_json::to_writer(&mut bytes, decision).map_err(|_| ErrorCode::StoreCorrupt)?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), ErrorCode> {
    let mut file = File::create(path).map_err(|_| ErrorCode::StoreIo)?;
    file.write_all(bytes).map_err(|_| ErrorCode::StoreIo)
}

fn sync_directory(path: &Path) -> Result<(), ErrorCode> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ErrorCode::StoreIo)
}

fn inject(
    returned_error_after: Option<PublicationStage>,
    stage: PublicationStage,
) -> Result<(), ErrorCode> {
    if returned_error_after == Some(stage) {
        Err(ErrorCode::InjectedReturnedError)
    } else {
        Ok(())
    }
}

fn sha256(bytes: &[u8]) -> Hash {
    digest(&SHA256, bytes)
        .as_ref()
        .try_into()
        .expect("SHA-256 length")
}

#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(TABLE[usize::from(byte >> 4)]));
        out.push(char::from(TABLE[usize::from(byte & 0x0f)]));
    }
    out
}
