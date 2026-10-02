//! User-key rotation records and the trust history they build (ADR-103 A13).
//!
//! The user key signs project certificates, anchor records, parent policies
//! and the user chain. Rotating it must not orphan what the old key sealed.
//! A [`RotationRecord`] hands the identity from one key to the next and is
//! signed by BOTH (dual-signed handover): the old key proves the owner of the
//! old identity agreed, the new key proves possession. Records are
//! hash-linked (`prev` is the SHA-256 of the previous record including its
//! signatures) and stored in an append-only file ([`RotationLog`]); the user
//! daemon also chains each one (`user.key.rotated`, source
//! [`crate::project_identity::SOURCE`]).
//!
//! [`UserKeyHistory`] folds the records into "which key is current, which
//! keys were retired and when". A verifier asks [`UserKeyHistory::accepts`]:
//! the current key always, a retired key only for material dated at or before
//! its rotation point. The final record must name the key the daemon
//! actually holds, otherwise the history is refused (a record forged with a
//! stolen old key alone names a key nobody holds).
//!
//! Honest limit: the rotation point is a wall-clock time, compared with the
//! time a record claims (a certificate's `issued_at`, an anchor statement's
//! `at`). Someone who still holds the OLD private key can backdate such a
//! claim; rotation after a suspected compromise must therefore also revoke or
//! rekey the affected projects. Material sealed after the rotation point
//! with the old key honestly dated is refused.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use clawft_types::project::canon::{canonical_json, hex_decode, hex_encode};
use clawft_types::project::cert::{key_id, ts};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Domain tag of both signatures of a rotation record.
pub const ROTATION_DOMAIN: &str = "weftos-user-key-rotation-v1\n";
/// Record format version.
pub const ROTATION_VERSION: u32 = 1;
/// Chain event kind appended by the user daemon for every record.
pub const KIND_ROTATED: &str = "user.key.rotated";
/// Name of the append-only log inside the manifest store.
pub const ROTATION_FILE: &str = "user-key-rotations.jsonl";

/// Why a rotation record or history is not trusted.
#[derive(Debug, thiserror::Error)]
pub enum RotationError {
    /// A record is malformed (version, hex, key ids, timestamp).
    #[error("rotation record {seq}: {why}")]
    Malformed {
        /// Record sequence.
        seq: u64,
        /// What is wrong.
        why: String,
    },
    /// A signature does not verify.
    #[error("rotation record {seq}: the {which} key's signature does not verify")]
    BadSignature {
        /// Record sequence.
        seq: u64,
        /// `old` or `new`.
        which: &'static str,
    },
    /// The records do not form one chain (sequence, `prev` or key continuity).
    #[error("rotation record {seq}: {why}")]
    Broken {
        /// Record sequence.
        seq: u64,
        /// What is wrong.
        why: String,
    },
    /// The last record does not hand over to the key this daemon holds.
    #[error("the rotation history ends at key {history} but the user key in use is {current}; refusing to trust the history")]
    NotCurrent {
        /// `key_id` the history ends at.
        history: String,
        /// `key_id` of the key in use.
        current: String,
    },
    /// The log file cannot be read or parsed.
    #[error("rotation log {path}: {why}; refusing to verify until it is repaired")]
    Log {
        /// File path.
        path: String,
        /// What is wrong.
        why: String,
    },
}

/// One dual-signed handover from `old_pubkey` to `new_pubkey`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotationRecord {
    /// [`ROTATION_VERSION`].
    pub v: u32,
    /// 1-based position in the log.
    pub seq: u64,
    /// Key being retired, 64 lowercase hex.
    pub old_pubkey: String,
    /// `key_id(old_pubkey)`.
    pub old_key_id: String,
    /// Key taking over, 64 lowercase hex.
    pub new_pubkey: String,
    /// `key_id(new_pubkey)`.
    pub new_key_id: String,
    /// The rotation point, `YYYY-MM-DDTHH:MM:SSZ`.
    pub rotated_at: String,
    /// SHA-256 ([`Self::hash`]) of the previous record; absent for the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    /// Hex signature by the old key.
    pub sig_old: String,
    /// Hex signature by the new key.
    pub sig_new: String,
}

impl RotationRecord {
    fn body(&self) -> Value {
        json!({
            "v": self.v,
            "seq": self.seq,
            "old_pubkey": self.old_pubkey,
            "old_key_id": self.old_key_id,
            "new_pubkey": self.new_pubkey,
            "new_key_id": self.new_key_id,
            "rotated_at": self.rotated_at,
            "prev": self.prev,
        })
    }

    fn signed_bytes(&self) -> Vec<u8> {
        let mut out = ROTATION_DOMAIN.as_bytes().to_vec();
        out.extend_from_slice(canonical_json(&self.body()).as_bytes());
        out
    }

    /// Sign a handover with both keys. `prev` is the previous record, if any.
    pub fn sign(
        old: &SigningKey,
        new: &SigningKey,
        prev: Option<&RotationRecord>,
        rotated_at: DateTime<Utc>,
    ) -> Self {
        let (op, np) = (old.verifying_key().to_bytes(), new.verifying_key().to_bytes());
        let mut r = Self {
            v: ROTATION_VERSION,
            seq: prev.map_or(1, |p| p.seq + 1),
            old_pubkey: hex_encode(&op),
            old_key_id: key_id(&op),
            new_pubkey: hex_encode(&np),
            new_key_id: key_id(&np),
            rotated_at: ts(rotated_at),
            prev: prev.map(Self::hash),
            sig_old: String::new(),
            sig_new: String::new(),
        };
        let bytes = r.signed_bytes();
        r.sig_old = hex_encode(&old.sign(&bytes).to_bytes());
        r.sig_new = hex_encode(&new.sign(&bytes).to_bytes());
        r
    }

    /// SHA-256 (hex) of the record including both signatures.
    pub fn hash(&self) -> String {
        let mut v = self.body();
        v["sig_old"] = json!(self.sig_old);
        v["sig_new"] = json!(self.sig_new);
        hex_encode(&Sha256::digest(canonical_json(&v).as_bytes()))
    }

    /// The rotation point as a time.
    pub fn rotated(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.rotated_at)
            .ok()
            .map(|t| t.with_timezone(&Utc))
            .filter(|t| ts(*t) == self.rotated_at)
    }

    /// Shape and both signatures.
    pub fn verify(&self) -> Result<(), RotationError> {
        let bad = |why: &str| RotationError::Malformed { seq: self.seq, why: why.to_owned() };
        if self.v != ROTATION_VERSION {
            return Err(bad("unsupported version"));
        }
        let op: [u8; 32] = hex_decode(&self.old_pubkey).ok_or_else(|| bad("old_pubkey is not 64 lowercase hex"))?;
        let np: [u8; 32] = hex_decode(&self.new_pubkey).ok_or_else(|| bad("new_pubkey is not 64 lowercase hex"))?;
        if self.old_key_id != key_id(&op) || self.new_key_id != key_id(&np) {
            return Err(bad("a key id does not match its public key"));
        }
        if op == np {
            return Err(bad("old and new keys are the same"));
        }
        if self.rotated().is_none() {
            return Err(bad("rotated_at is not canonical RFC 3339 (YYYY-MM-DDTHH:MM:SSZ)"));
        }
        let bytes = self.signed_bytes();
        for (which, pk, sig) in [("old", op, &self.sig_old), ("new", np, &self.sig_new)] {
            let sig: [u8; 64] = hex_decode(sig).ok_or_else(|| bad("a signature is not 128 lowercase hex"))?;
            VerifyingKey::from_bytes(&pk)
                .map_err(|_| bad("a public key is not a valid point"))?
                .verify_strict(&bytes, &Signature::from_bytes(&sig))
                .map_err(|_| RotationError::BadSignature { seq: self.seq, which })?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Retired {
    pubkey: [u8; 32],
    rotated_at: DateTime<Utc>,
}

/// The current user key plus every key it replaced and when.
#[derive(Debug, Clone)]
pub struct UserKeyHistory {
    current: [u8; 32],
    retired: Vec<Retired>,
}

impl UserKeyHistory {
    /// No rotation ever happened.
    pub fn single(current: &[u8; 32]) -> Self {
        Self { current: *current, retired: Vec::new() }
    }

    /// Fold `records` (oldest first) into a history ending at `current`.
    /// Every record must verify, link to its predecessor and continue its key.
    pub fn from_records(current: &[u8; 32], records: &[RotationRecord]) -> Result<Self, RotationError> {
        let mut retired: Vec<Retired> = Vec::new();
        let mut prev: Option<&RotationRecord> = None;
        for r in records {
            r.verify()?;
            let broken = |why: &str| RotationError::Broken { seq: r.seq, why: why.to_owned() };
            match prev {
                None if r.seq != 1 || r.prev.is_some() => return Err(broken("the first record must be seq 1 with no prev")),
                Some(p) => {
                    if r.seq != p.seq + 1 {
                        return Err(broken("sequence is not previous + 1"));
                    }
                    if r.prev.as_deref() != Some(p.hash().as_str()) {
                        return Err(broken("prev does not match the previous record"));
                    }
                    if r.old_pubkey != p.new_pubkey {
                        return Err(broken("old key is not the previous record's new key"));
                    }
                    if r.rotated() < p.rotated() {
                        return Err(broken("rotated_at goes backwards"));
                    }
                }
                None => {}
            }
            let op = hex_decode::<32>(&r.old_pubkey).ok_or_else(|| broken("old_pubkey"))?;
            let when = r.rotated().ok_or_else(|| broken("rotated_at"))?;
            retired.push(Retired { pubkey: op, rotated_at: when });
            prev = Some(r);
        }
        let end = match prev {
            Some(l) => hex_decode::<32>(&l.new_pubkey).unwrap_or([0; 32]),
            None => *current,
        };
        if &end != current {
            return Err(RotationError::NotCurrent { history: key_id(&end), current: key_id(current) });
        }
        Ok(Self { current: *current, retired })
    }

    /// The key currently in use.
    pub fn current(&self) -> &[u8; 32] {
        &self.current
    }

    /// Number of retired keys.
    pub fn rotations(&self) -> usize {
        self.retired.len()
    }

    /// The public key with this `key_id`, current or retired.
    pub fn key_for(&self, user_key_id: &str) -> Option<[u8; 32]> {
        if key_id(&self.current) == user_key_id {
            return Some(self.current);
        }
        self.retired.iter().find(|r| key_id(&r.pubkey) == user_key_id).map(|r| r.pubkey)
    }

    /// May `pubkey` have sealed something dated `at`? The current key always;
    /// a retired key only up to (and including) its rotation point.
    pub fn accepts(&self, pubkey: &[u8; 32], at: DateTime<Utc>) -> bool {
        if pubkey == &self.current {
            return true;
        }
        self.retired.iter().any(|r| &r.pubkey == pubkey && at <= r.rotated_at)
    }

    /// Public keys of every retired key, oldest first.
    pub fn retired_keys(&self) -> impl Iterator<Item = [u8; 32]> + '_ {
        self.retired.iter().map(|r| r.pubkey)
    }

    /// The rotation point of a retired key.
    pub fn rotated_at(&self, pubkey: &[u8; 32]) -> Option<DateTime<Utc>> {
        self.retired.iter().find(|r| &r.pubkey == pubkey).map(|r| r.rotated_at)
    }
}

/// The append-only rotation log in a manifest store.
#[derive(Debug, Clone)]
pub struct RotationLog {
    path: PathBuf,
}

impl RotationLog {
    /// The log for the manifest store `dir`.
    pub fn new(dir: &Path) -> Self {
        Self { path: dir.join(ROTATION_FILE) }
    }

    /// File path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every record. A missing file is an empty log; an unreadable or
    /// unparseable one is an error (fail closed).
    pub fn read(&self) -> Result<Vec<RotationRecord>, RotationError> {
        let log = |why: String| RotationError::Log { path: self.path.display().to_string(), why };
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(log(e.to_string())),
        };
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .enumerate()
            .map(|(i, l)| serde_json::from_str(l).map_err(|e| log(format!("line {}: {e}", i + 1))))
            .collect()
    }

    /// The history for `current`: single when the log is empty.
    pub fn history(&self, current: &[u8; 32]) -> Result<UserKeyHistory, RotationError> {
        UserKeyHistory::from_records(current, &self.read()?)
    }

    /// Append `record` (the whole file is rewritten atomically, 0600). The
    /// record must extend the existing log.
    pub fn append(&self, record: &RotationRecord) -> Result<(), RotationError> {
        let mut all = self.read()?;
        all.push(record.clone());
        let end = hex_decode::<32>(&record.new_pubkey).unwrap_or([0; 32]);
        UserKeyHistory::from_records(&end, &all)?;
        let mut body = String::new();
        for r in &all {
            body.push_str(&serde_json::to_string(r).unwrap_or_default());
            body.push('\n');
        }
        crate::project_identity::write_private_atomic(&self.path, body.as_bytes(), false)
            .map_err(|e| RotationError::Log { path: self.path.display().to_string(), why: e.to_string() })
    }
}

#[cfg(test)]
#[path = "user_key_rotation_tests.rs"]
mod tests;
