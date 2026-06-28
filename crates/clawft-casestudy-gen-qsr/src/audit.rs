//! Phase 4 — hash-chained audit log.
//!
//! Mirrors the ExoChain pattern from `crates/clawft-kernel/src/chain.rs`:
//! every meaningful pipeline event (impulse accepted, blocked, dropped,
//! governance decision) records an entry whose `chain_hash` depends on the
//! previous entry's `chain_hash`. Tampering or skipping anywhere in the chain
//! breaks `verify()`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    ImpulseEmitted,
    ImpulseApplied,
    ImpulseBlocked,
    ImpulseDropped,
    ImpulseLateArrival,
    ShardRollover,
    GovernanceSeal,
    ChaosInjection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub seq: u64,
    pub kind: AuditKind,
    pub ts_ms: u64,
    pub payload_hash: String, // BLAKE3 of the payload bytes
    pub prev_hash: String,
    pub chain_hash: String, // BLAKE3(prev_hash || seq || kind || payload_hash)
    pub summary: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("chain break at entry {seq}: prev_hash does not match prior chain_hash")]
    ChainBreak { seq: u64 },
    #[error("hash mismatch at entry {seq}: chain_hash does not match recomputed value")]
    HashMismatch { seq: u64 },
    #[error("sequence gap at entry {seq}: expected {expected}")]
    SequenceGap { seq: u64, expected: u64 },
}

/// In-memory hash-chained auditor. Append-only.
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct HashChainAuditor {
    pub entries: Vec<AuditEntry>,
}

impl HashChainAuditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record<P: Serialize>(
        &mut self,
        kind: AuditKind,
        summary: impl Into<String>,
        payload: &P,
    ) {
        let summary = summary.into();
        let payload_bytes = serde_json::to_vec(payload).unwrap_or_default();
        let payload_hash = blake3::hash(&payload_bytes).to_hex().to_string();
        let prev_hash = self
            .entries
            .last()
            .map(|e| e.chain_hash.clone())
            .unwrap_or_else(|| "0".repeat(64));
        let seq = self.entries.len() as u64;
        let ts_ms = now_ms();
        let chain_hash = compute_chain_hash(&prev_hash, seq, kind, &payload_hash);
        self.entries.push(AuditEntry {
            seq,
            kind,
            ts_ms,
            payload_hash,
            prev_hash,
            chain_hash,
            summary,
        });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn count_by_kind(&self, kind: AuditKind) -> usize {
        self.entries.iter().filter(|e| e.kind == kind).count()
    }

    pub fn verify(&self) -> Result<(), AuditError> {
        let mut expected_prev: String = "0".repeat(64);
        for (i, e) in self.entries.iter().enumerate() {
            let expected_seq = i as u64;
            if e.seq != expected_seq {
                return Err(AuditError::SequenceGap {
                    seq: e.seq,
                    expected: expected_seq,
                });
            }
            if e.prev_hash != expected_prev {
                return Err(AuditError::ChainBreak { seq: e.seq });
            }
            let recomputed = compute_chain_hash(&e.prev_hash, e.seq, e.kind, &e.payload_hash);
            if recomputed != e.chain_hash {
                return Err(AuditError::HashMismatch { seq: e.seq });
            }
            expected_prev = e.chain_hash.clone();
        }
        Ok(())
    }
}

fn compute_chain_hash(prev: &str, seq: u64, kind: AuditKind, payload_hash: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(prev.as_bytes());
    h.update(&seq.to_le_bytes());
    let kind_byte: u8 = match kind {
        AuditKind::ImpulseEmitted => 0,
        AuditKind::ImpulseApplied => 1,
        AuditKind::ImpulseBlocked => 2,
        AuditKind::ImpulseDropped => 3,
        AuditKind::ImpulseLateArrival => 4,
        AuditKind::ShardRollover => 5,
        AuditKind::GovernanceSeal => 6,
        AuditKind::ChaosInjection => 7,
    };
    h.update(&[kind_byte]);
    h.update(payload_hash.as_bytes());
    h.finalize().to_hex().to_string()
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
