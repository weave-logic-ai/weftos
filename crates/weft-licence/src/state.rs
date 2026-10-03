//! Durable service state: checkout slots with their `seq`, the serve ledger,
//! and the operator key list. Every write is atomic and fsynced; a `seq` is
//! on disk before the grant that carries it is released.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use weft_licence_wire::{GrantArtifact, LicenceRef, SignedEnvelope, hex_decode_exact};

use crate::error::SvcError;
use crate::fsio;

const SLOTS_FILE: &str = "slots.json";
const SERVES_FILE: &str = "serves.json";
const NONCES_FILE: &str = "nonces.json";
const OPERATORS_FILE: &str = "operators";
const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;

/// Whether a slot still renews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotStatus {
    /// Renewed on every pull.
    Active,
    /// Released by the operator; its last grant is a withdrawal.
    Released,
    /// The licence stopped covering it; its last grant is a withdrawal.
    Lapsed,
}

/// One (cog, version): the arches checked out so far, its `seq`, and the
/// latest signed grant (re-served by `GET /grants` without signing again).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    /// The mesh this checkout belongs to (hex). A slot is live only for the
    /// currently bound mesh; empty (older state) never matches.
    #[serde(default)]
    pub mesh_id: String,
    /// Cog id.
    pub cog_id: String,
    /// Version.
    pub version: String,
    /// Last `seq` signed. The next grant uses `seq + 1`.
    pub seq: u64,
    /// Checked-out binaries by arch (the union applies across checkouts).
    pub arches: BTreeMap<String, GrantArtifact>,
    /// sha256 of the registry entry used.
    pub manifest_sha256: String,
    /// Registry the binaries came from.
    pub registry: String,
    /// The licence the grants rest on (hashed reference, expiry).
    pub licence: LicenceRef,
    /// Active, released or lapsed.
    pub status: SlotStatus,
    /// Global issue counter at the last grant, for `GET /grants?since=`.
    pub issue_ctr: u64,
    /// The latest signed grant.
    pub grant: Option<SignedEnvelope>,
}

/// The slot table and counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slots {
    /// Global monotonic issue counter.
    pub ctr: u64,
    /// Highest `issued_at` ever signed (the persisted clock floor).
    pub last_issued_at: u64,
    /// `cog@version` to slot.
    pub slots: BTreeMap<String, Slot>,
}

/// One byte transfer, for the 3-per-24-hours rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServeRec {
    /// Steward key id.
    pub key_id: String,
    /// Cog id.
    pub cog_id: String,
    /// Version.
    pub version: String,
    /// Architecture.
    pub arch: String,
    /// When it started, unix seconds.
    pub ts: u64,
}

/// Slot key.
pub fn slot_key(cog: &str, version: &str) -> String {
    format!("{cog}@{version}")
}

/// The state directory and its in-memory copy.
#[derive(Debug)]
pub struct Store {
    /// State directory.
    pub dir: PathBuf,
    /// Slots.
    pub slots: Slots,
    /// Serve ledger.
    pub serves: Vec<ServeRec>,
    /// Nonces seen inside the replay window (`nonce`, `ts`).
    pub nonces: Vec<(String, u64)>,
    /// Test seam: make slot-table writes fail (a crash before the write).
    pub fail_persist: bool,
}

fn load_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T, SvcError> {
    match fsio::read_capped(path, MAX_STATE_BYTES) {
        Ok(None) => Ok(T::default()),
        Ok(Some(b)) => serde_json::from_slice(&b)
            .map_err(|_| SvcError::Corrupt(path.display().to_string())),
        Err(_) => Err(SvcError::Corrupt(path.display().to_string())),
    }
}

impl Store {
    /// Open the state directory. A corrupt file is an error, never silently
    /// reset: a reset `seq` would reissue old sequence numbers.
    pub fn open(dir: &Path) -> Result<Self, SvcError> {
        fsio::ensure_private_dir(dir).map_err(|e| SvcError::Io(e.to_string()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            slots: load_json(&dir.join(SLOTS_FILE))?,
            serves: load_json(&dir.join(SERVES_FILE))?,
            nonces: load_json(&dir.join(NONCES_FILE))?,
            fail_persist: false,
        })
    }

    fn write(&self, name: &str, bytes: Vec<u8>) -> Result<(), SvcError> {
        if self.fail_persist && name == SLOTS_FILE {
            return Err(SvcError::Persist("injected failure".into()));
        }
        fsio::write_atomic(&self.dir.join(name), &bytes).map_err(|e| SvcError::Persist(e.to_string()))
    }

    /// Persist the slot table durably.
    pub fn persist_slots(&self) -> Result<(), SvcError> {
        let b = serde_json::to_vec(&self.slots).map_err(|e| SvcError::Persist(e.to_string()))?;
        self.write(SLOTS_FILE, b)
    }

    /// Persist the serve ledger durably.
    pub fn persist_serves(&self) -> Result<(), SvcError> {
        let b = serde_json::to_vec(&self.serves).map_err(|e| SvcError::Persist(e.to_string()))?;
        self.write(SERVES_FILE, b)
    }

    /// Persist the nonce list (replay protection survives a restart).
    pub fn persist_nonces(&self) -> Result<(), SvcError> {
        let b = serde_json::to_vec(&self.nonces).map_err(|e| SvcError::Persist(e.to_string()))?;
        self.write(NONCES_FILE, b)
    }

    /// Path of the operator override directory.
    pub fn overrides_dir(&self) -> PathBuf {
        self.dir.join("overrides")
    }
}

/// Pinned operator keys: the config list plus `<state_dir>/operators`, one
/// 64-hex key per line (written by `init --operator-key`).
#[derive(Debug, Clone, Default)]
pub struct OperatorKeys(Vec<[u8; 32]>);

impl OperatorKeys {
    /// Merge config keys with the state-dir file.
    pub fn load(state_dir: &Path, from_config: &[String]) -> Result<Self, SvcError> {
        let mut keys = Vec::new();
        let mut add = |s: &str| -> Result<(), SvcError> {
            let k = hex_decode_exact::<32>(s.trim())
                .ok_or_else(|| SvcError::Config(format!("operator key {s:?} is not 64 hex")))?;
            if !keys.contains(&k) {
                keys.push(k);
            }
            Ok(())
        };
        for s in from_config {
            add(s)?;
        }
        if let Some(b) = fsio::read_capped(&state_dir.join(OPERATORS_FILE), 64 * 1024)
            .map_err(|e| SvcError::Io(e.to_string()))?
        {
            for line in String::from_utf8_lossy(&b).lines().filter(|l| !l.trim().is_empty()) {
                add(line)?;
            }
        }
        Ok(Self(keys))
    }

    /// Append a key to the state-dir file (`init --operator-key`).
    pub fn pin(state_dir: &Path, hex: &str) -> Result<(), SvcError> {
        hex_decode_exact::<32>(hex.trim())
            .ok_or_else(|| SvcError::Config("operator key is not 64 hex".into()))?;
        let path = state_dir.join(OPERATORS_FILE);
        let mut cur = fsio::read_capped(&path, 64 * 1024)
            .map_err(|e| SvcError::Io(e.to_string()))?
            .unwrap_or_default();
        cur.extend_from_slice(format!("{}\n", hex.trim()).as_bytes());
        fsio::write_atomic(&path, &cur).map_err(|e| SvcError::Io(e.to_string()))
    }

    /// True when `pk` is a pinned operator key.
    pub fn contains(&self, pk: &[u8; 32]) -> bool {
        self.0.contains(pk)
    }

    /// True when no operator key is pinned.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
