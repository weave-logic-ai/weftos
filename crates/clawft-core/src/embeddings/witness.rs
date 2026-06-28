//! WITNESS -- SHA-256 hash-chained tamper-evident audit trail (H2.6).
//!
//! Each memory write (store, update, delete) creates a [`WitnessSegment`]
//! containing a SHA-256 hash of the operation data and a pointer to the
//! previous segment's hash, forming a hash chain. Verification walks the
//! chain from the root and recomputes each hash to detect tampering.
//!
//! The chain starts with `previous_hash = [0u8; 32]` (the "genesis" hash).
//!
//! # Usage
//!
//! ```rust,no_run
//! use clawft_core::embeddings::witness::{WitnessChain, WitnessOperation};
//!
//! let mut chain = WitnessChain::new();
//! chain.append(WitnessOperation::Store, b"hello world");
//! chain.append(WitnessOperation::Update, b"hello world v2");
//! assert!(chain.verify());
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The genesis hash: 32 zero bytes, used as `previous_hash` for the
/// first segment in a chain.
pub const GENESIS_HASH: [u8; 32] = [0u8; 32];

/// The type of operation recorded in a WITNESS segment.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WitnessOperation {
    /// A new entry was stored.
    Store,
    /// An existing entry was updated.
    Update,
    /// An entry was deleted.
    Delete,
}

impl std::fmt::Display for WitnessOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WitnessOperation::Store => write!(f, "store"),
            WitnessOperation::Update => write!(f, "update"),
            WitnessOperation::Delete => write!(f, "delete"),
        }
    }
}

/// A single segment in the WITNESS hash chain.
///
/// Each segment records one memory operation and is cryptographically
/// linked to the previous segment via SHA-256.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WitnessSegment {
    /// Unique identifier for this segment.
    pub segment_id: Uuid,
    /// When this segment was created.
    pub timestamp: DateTime<Utc>,
    /// The type of operation.
    pub operation: WitnessOperation,
    /// SHA-256 hash of the operation data.
    pub data_hash: [u8; 32],
    /// SHA-256 hash of the previous segment (or GENESIS_HASH for root).
    pub previous_hash: [u8; 32],
    /// This segment's own hash (computed from all fields above).
    pub segment_hash: [u8; 32],
}

impl WitnessSegment {
    /// Compute the segment hash from its constituent fields.
    ///
    /// The hash covers: segment_id, timestamp, operation, data_hash,
    /// and previous_hash -- ensuring that modifying any field will
    /// invalidate the hash.
    pub fn compute_hash(
        segment_id: &Uuid,
        timestamp: &DateTime<Utc>,
        operation: WitnessOperation,
        data_hash: &[u8; 32],
        previous_hash: &[u8; 32],
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(segment_id.as_bytes());
        hasher.update(timestamp.to_rfc3339().as_bytes());
        hasher.update(operation.to_string().as_bytes());
        hasher.update(data_hash);
        hasher.update(previous_hash);
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }

    /// Verify that this segment's hash matches its fields.
    pub fn verify_self(&self) -> bool {
        let expected = Self::compute_hash(
            &self.segment_id,
            &self.timestamp,
            self.operation,
            &self.data_hash,
            &self.previous_hash,
        );
        self.segment_hash == expected
    }
}

/// Errors from WITNESS chain operations.
#[non_exhaustive]
#[derive(Debug)]
pub enum WitnessError {
    /// The chain is corrupted: a segment's hash does not match.
    ChainCorrupted {
        /// Index of the corrupted segment.
        index: usize,
        /// The segment ID of the corrupted segment.
        segment_id: Uuid,
    },
    /// A segment's `previous_hash` does not match the previous
    /// segment's `segment_hash`.
    LinkBroken {
        /// Index of the segment with the broken link.
        index: usize,
        /// The segment ID.
        segment_id: Uuid,
    },
    /// Serialization error.
    Serde(serde_json::Error),
    /// I/O error.
    Io(std::io::Error),
}

impl std::fmt::Display for WitnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WitnessError::ChainCorrupted { index, segment_id } => {
                write!(
                    f,
                    "WITNESS chain corrupted at index {index} \
                     (segment {segment_id})"
                )
            }
            WitnessError::LinkBroken { index, segment_id } => {
                write!(
                    f,
                    "WITNESS chain link broken at index {index} \
                     (segment {segment_id})"
                )
            }
            WitnessError::Serde(e) => write!(f, "WITNESS serde error: {e}"),
            WitnessError::Io(e) => write!(f, "WITNESS I/O error: {e}"),
        }
    }
}

impl std::error::Error for WitnessError {}

impl From<serde_json::Error> for WitnessError {
    fn from(e: serde_json::Error) -> Self {
        WitnessError::Serde(e)
    }
}

impl From<std::io::Error> for WitnessError {
    fn from(e: std::io::Error) -> Self {
        WitnessError::Io(e)
    }
}

// ── WitnessChain ────────────────────────────────────────────────────

/// A chain of WITNESS segments forming a tamper-evident audit trail.
///
/// Segments are appended sequentially. Each new segment's `previous_hash`
/// is set to the preceding segment's `segment_hash` (or [`GENESIS_HASH`]
/// for the first segment).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WitnessChain {
    segments: Vec<WitnessSegment>,
}

impl WitnessChain {
    /// Create a new, empty chain.
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    /// Append a new segment to the chain.
    ///
    /// Computes the SHA-256 hash of `data`, links to the previous
    /// segment, and computes the segment hash.
    pub fn append(&mut self, operation: WitnessOperation, data: &[u8]) {
        let previous_hash = self
            .segments
            .last()
            .map(|s| s.segment_hash)
            .unwrap_or(GENESIS_HASH);

        let segment_id = Uuid::new_v4();
        let timestamp = Utc::now();

        let mut data_hasher = Sha256::new();
        data_hasher.update(data);
        let data_result = data_hasher.finalize();
        let mut data_hash = [0u8; 32];
        data_hash.copy_from_slice(&data_result);

        let segment_hash = WitnessSegment::compute_hash(
            &segment_id,
            &timestamp,
            operation,
            &data_hash,
            &previous_hash,
        );

        self.segments.push(WitnessSegment {
            segment_id,
            timestamp,
            operation,
            data_hash,
            previous_hash,
            segment_hash,
        });
    }

    /// Verify the integrity of the entire chain.
    ///
    /// Returns `true` if the chain is valid (all hashes match and all
    /// links are intact). Returns `false` if any tampering is detected.
    pub fn verify(&self) -> bool {
        self.verify_detailed().is_ok()
    }

    /// Verify the chain, returning detailed error information on failure.
    pub fn verify_detailed(&self) -> Result<(), WitnessError> {
        let mut expected_previous = GENESIS_HASH;

        for (i, segment) in self.segments.iter().enumerate() {
            // Check link to previous segment.
            if segment.previous_hash != expected_previous {
                return Err(WitnessError::LinkBroken {
                    index: i,
                    segment_id: segment.segment_id,
                });
            }

            // Check segment self-hash.
            if !segment.verify_self() {
                return Err(WitnessError::ChainCorrupted {
                    index: i,
                    segment_id: segment.segment_id,
                });
            }

            expected_previous = segment.segment_hash;
        }

        Ok(())
    }

    /// Return the number of segments in the chain.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Return `true` if the chain has no segments.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Return a reference to all segments.
    pub fn segments(&self) -> &[WitnessSegment] {
        &self.segments
    }

    /// Return a mutable reference to all segments (for testing/import).
    pub fn segments_mut(&mut self) -> &mut Vec<WitnessSegment> {
        &mut self.segments
    }

    /// Return the hash of the last segment (chain tip), or GENESIS_HASH
    /// if the chain is empty.
    pub fn tip_hash(&self) -> [u8; 32] {
        self.segments
            .last()
            .map(|s| s.segment_hash)
            .unwrap_or(GENESIS_HASH)
    }

    /// Serialize the chain to JSON.
    pub fn to_json(&self) -> Result<String, WitnessError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Deserialize a chain from JSON.
    pub fn from_json(json: &str) -> Result<Self, WitnessError> {
        Ok(serde_json::from_str(json)?)
    }

    /// Save the chain to a file.
    pub fn save(&self, path: &std::path::Path) -> Result<(), WitnessError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = self.to_json()?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a chain from a file.
    ///
    /// Returns an empty chain if the file does not exist.
    pub fn load(path: &std::path::Path) -> Result<Self, WitnessError> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let json = std::fs::read_to_string(path)?;
        Self::from_json(&json)
    }

    /// Load and verify a chain from a file.
    ///
    /// Returns an error if the chain is corrupted.
    pub fn load_verified(path: &std::path::Path) -> Result<Self, WitnessError> {
        let chain = Self::load(path)?;
        chain.verify_detailed()?;
        Ok(chain)
    }
}

impl Default for WitnessChain {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the SHA-256 hash of arbitrary data.
///
/// Convenience function for use by other modules that need to hash data
/// before passing it to the WITNESS chain.
pub fn sha256_hash(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(label: &str) -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_witness_test_{label}_{pid}_{n}.json"))
    }

    #[test]
    fn empty_chain_is_valid() {
        let chain = WitnessChain::new();
        assert!(chain.verify());
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
        assert_eq!(chain.tip_hash(), GENESIS_HASH);
    }

    #[test]
    fn single_segment_chain_valid() {
        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"hello world");
        assert!(chain.verify());
        assert_eq!(chain.len(), 1);

        let seg = &chain.segments()[0];
        assert_eq!(seg.operation, WitnessOperation::Store);
        assert_eq!(seg.previous_hash, GENESIS_HASH);
    }

    #[test]
    fn multi_segment_chain_valid() {
        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"entry 1");
        chain.append(WitnessOperation::Update, b"entry 1 v2");
        chain.append(WitnessOperation::Store, b"entry 2");
        chain.append(WitnessOperation::Delete, b"entry 1");
        assert!(chain.verify());
        assert_eq!(chain.len(), 4);
    }

    #[test]
    fn link_continuity() {
        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"a");
        chain.append(WitnessOperation::Store, b"b");

        let s0 = &chain.segments()[0];
        let s1 = &chain.segments()[1];
        assert_eq!(s1.previous_hash, s0.segment_hash);
    }

    #[test]
    fn tampering_detected_modified_data_hash() {
        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"original");
        chain.append(WitnessOperation::Store, b"second");

        // Tamper with the first segment's data_hash.
        chain.segments[0].data_hash[0] ^= 0xFF;

        assert!(!chain.verify());
        let err = chain.verify_detailed().unwrap_err();
        match err {
            WitnessError::ChainCorrupted { index, .. } => {
                assert_eq!(index, 0)
            }
            _ => panic!("expected ChainCorrupted error"),
        }
    }

    #[test]
    fn tampering_detected_modified_segment_hash() {
        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"a");
        chain.append(WitnessOperation::Store, b"b");

        // Tamper with the first segment's segment_hash.
        chain.segments[0].segment_hash[0] ^= 0xFF;

        assert!(!chain.verify());
    }

    #[test]
    fn tampering_detected_broken_link() {
        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"a");
        chain.append(WitnessOperation::Store, b"b");

        // Tamper with second segment's previous_hash.
        chain.segments[1].previous_hash[0] ^= 0xFF;

        assert!(!chain.verify());
        let err = chain.verify_detailed().unwrap_err();
        match err {
            WitnessError::LinkBroken { index, .. } => {
                assert_eq!(index, 1)
            }
            _ => panic!("expected LinkBroken error"),
        }
    }

    #[test]
    fn json_roundtrip() {
        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"test data");
        chain.append(WitnessOperation::Update, b"test data v2");

        let json = chain.to_json().unwrap();
        let loaded = WitnessChain::from_json(&json).unwrap();

        assert!(loaded.verify());
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.segments()[0].operation, WitnessOperation::Store);
        assert_eq!(loaded.segments()[1].operation, WitnessOperation::Update);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = temp_path("save_load");
        let _ = std::fs::remove_file(&path);

        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"persistent data");
        chain.save(&path).unwrap();

        let loaded = WitnessChain::load(&path).unwrap();
        assert!(loaded.verify());
        assert_eq!(loaded.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let path = temp_path("nonexist");
        let _ = std::fs::remove_file(&path);

        let chain = WitnessChain::load(&path).unwrap();
        assert!(chain.is_empty());
    }

    #[test]
    fn load_verified_rejects_tampered() {
        let path = temp_path("verified");
        let _ = std::fs::remove_file(&path);

        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"data");
        chain.save(&path).unwrap();

        // Tamper with the file.
        let mut loaded = WitnessChain::load(&path).unwrap();
        loaded.segments[0].data_hash[0] ^= 0xFF;
        let tampered_json = loaded.to_json().unwrap();
        std::fs::write(&path, tampered_json).unwrap();

        let result = WitnessChain::load_verified(&path);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tip_hash_updates_on_append() {
        let mut chain = WitnessChain::new();
        assert_eq!(chain.tip_hash(), GENESIS_HASH);

        chain.append(WitnessOperation::Store, b"a");
        let tip1 = chain.tip_hash();
        assert_ne!(tip1, GENESIS_HASH);

        chain.append(WitnessOperation::Store, b"b");
        let tip2 = chain.tip_hash();
        assert_ne!(tip2, tip1);
    }

    #[test]
    fn sha256_hash_deterministic() {
        let h1 = sha256_hash(b"test data");
        let h2 = sha256_hash(b"test data");
        assert_eq!(h1, h2);

        let h3 = sha256_hash(b"different data");
        assert_ne!(h1, h3);
    }

    #[test]
    fn operation_display() {
        assert_eq!(format!("{}", WitnessOperation::Store), "store");
        assert_eq!(format!("{}", WitnessOperation::Update), "update");
        assert_eq!(format!("{}", WitnessOperation::Delete), "delete");
    }

    #[test]
    fn witness_error_display() {
        let err = WitnessError::ChainCorrupted {
            index: 3,
            segment_id: Uuid::nil(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("corrupted"));
        assert!(msg.contains("3"));

        let err = WitnessError::LinkBroken {
            index: 5,
            segment_id: Uuid::nil(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("broken"));
        assert!(msg.contains("5"));
    }

    #[test]
    fn segment_verify_self_valid() {
        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"test");
        assert!(chain.segments()[0].verify_self());
    }

    #[test]
    fn segment_verify_self_invalid() {
        let mut chain = WitnessChain::new();
        chain.append(WitnessOperation::Store, b"test");
        let mut seg = chain.segments()[0].clone();
        seg.data_hash[0] ^= 0xFF;
        assert!(!seg.verify_self());
    }

    #[test]
    fn default_creates_empty() {
        let chain = WitnessChain::default();
        assert!(chain.is_empty());
    }
}
