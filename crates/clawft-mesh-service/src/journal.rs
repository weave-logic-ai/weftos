//! The machine journal (ADR-103, plan section 1.3).
//!
//! JSON lines, hash-chained and signed by the box key. A record is
//! `{"v":1,"seq":..,"ts":..,"prev":"<hex>","kind":..,"body":..,"sig":".."}`
//! where `prev` is the SHA-256 of the previous line's bytes (without the
//! newline; 64 zeros at seq 0) and `sig` is
//! `Ed25519(box_key, DOMAIN || line-without-sig)`.
//!
//! The signed bytes are taken from the raw line (the line with its fixed-width
//! trailing `,"sig":"<128 hex>"}` replaced by `}`), so verification never
//! depends on re-serialising JSON.
//!
//! Open verifies every hash link and signature. A bad record and everything
//! after it is moved to `journal.corrupt.<ts>` and a durable marker
//! (`journal.truncated`) is written. While the marker exists the journal is
//! read-only for binds and cert issues, across restarts, until an admin
//! acknowledges via `Bindings::accept_truncate` (which journals what was
//! lost). A bad *first* record is a hard error and nothing is modified (wrong
//! key or wrong directory, not a damaged tail).
//!
//! One writer: an exclusive `flock` on `mesh.lock`, holding the owner pid.

use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clawft_mesh_local::{hexser, Principal};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fsutil::{self, MAX_RECORD_BYTES};
use crate::chain::scan;
use crate::lost::{harvest, recompute, LostInfo};

/// Domain separation prefix for journal signatures.
pub const DOMAIN: &[u8] = b"weftos/mesh-journal/v1\0";
/// Schema version written into every record.
pub const RECORD_VERSION: u32 = 1;
/// Default segment size (plan 1.3): 8 MiB.
pub const DEFAULT_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
/// Active file name.
pub const ACTIVE: &str = "journal.jsonl";
/// Durable "bad tail was quarantined and not yet acknowledged" marker.
pub const MARKER: &str = "journal.truncated";
/// Record kind journalled by an acknowledged truncation.
pub const KIND_ACCEPT_TRUNCATE: &str = "journal.accept_truncate";
/// Signed record of a quarantine: the facts that constrain the state.
pub const KIND_QUARANTINE: &str = "journal.quarantine";
const ZERO_PREV: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("journal io: {0}")]
    Io(#[from] std::io::Error),
    #[error("state directory is locked by pid {}", .holder_pid.map_or("unknown".to_string(), |p| p.to_string()))]
    Locked { holder_pid: Option<u32> },
    #[error("journal head cannot be verified ({file}): {reason}; nothing was modified")]
    Unverifiable { file: String, reason: String },
    #[error("journal is read-only until an admin accepts the truncation")]
    ReadOnly,
    #[error("journal is poisoned by a failed write; restart the service")]
    Poisoned,
    #[error("unsafe state path: {0}")]
    UnsafePath(String),
    #[error("record kind `{0}` is reserved for the bindings writer")]
    ReservedKind(String),
    #[error("record of {0} bytes exceeds the {MAX_RECORD_BYTES} byte limit")]
    RecordTooLarge(usize),
    #[error("record body: {0}")]
    Encode(#[from] serde_json::Error),
}

/// Witness that an operator-authorised admin path requested the acknowledgement.
///
/// Only the admin path (`weaver mesh journal --accept-truncate`, after the
/// peer-credential admin check) may construct this; nothing else should.
#[derive(Debug, Clone)]
pub struct AdminAck {
    pub(crate) by: Principal,
}

impl AdminAck {
    /// Call only after the requester was verified as an admin.
    pub fn admin_verified(by: Principal) -> Self {
        Self { by }
    }
}

/// One journal record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub v: u32,
    pub seq: u64,
    pub ts: u64,
    pub prev: String,
    pub kind: String,
    pub body: serde_json::Value,
    pub sig: String,
}

/// Record minus signature; field order here is the canonical order.
#[derive(Serialize)]
struct Unsigned<'a> {
    v: u32,
    seq: u64,
    ts: u64,
    prev: &'a str,
    kind: &'a str,
    body: &'a serde_json::Value,
}

/// Sequence number and line hash of the newest record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub seq: u64,
    pub hash: String,
}

#[derive(Debug, Clone, Copy)]
pub struct JournalOptions {
    pub max_segment_bytes: u64,
}

impl Default for JournalOptions {
    fn default() -> Self {
        Self { max_segment_bytes: DEFAULT_SEGMENT_BYTES }
    }
}

pub struct Journal {
    dir: PathBuf,
    key: SigningKey,
    opts: JournalOptions,
    _lock: File,
    records: Vec<Record>,
    prev_hash: [u8; 32],
    active_len: u64,
    lost: Option<LostInfo>,
    quarantined: Option<PathBuf>,
    poisoned: bool,
    fail_next_write: bool,
}

/// Highest serial any journalled record already accounts for.
fn serial_floor(records: &[Record]) -> u64 {
    records
        .iter()
        .filter_map(|r| match r.kind.as_str() {
            "user.cert.issue" => r.body["serial"].as_u64(),
            KIND_QUARANTINE => r.body["serial_high_water"].as_u64(),
            KIND_ACCEPT_TRUNCATE => r.body["serial_floor"].as_u64(),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Kinds only the bindings writer may append.
fn reserved(kind: &str) -> bool {
    kind.starts_with("user.") || kind == KIND_ACCEPT_TRUNCATE || kind == KIND_QUARANTINE
}

/// Numbered segments, ascending: `journal.NNN.jsonl`.
pub(crate) fn segments(dir: &Path) -> std::io::Result<Vec<(u32, PathBuf)>> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(n) = name
            .strip_prefix("journal.")
            .and_then(|r| r.strip_suffix(".jsonl"))
            .and_then(|n| (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then_some(n))
            .and_then(|n| n.parse::<u32>().ok())
        {
            out.push((n, e.path()));
        }
    }
    out.sort();
    Ok(out)
}

impl Journal {
    /// Open (creating the state dir if needed), lock, verify and, if needed,
    /// quarantine a bad tail.
    pub fn open(dir: impl AsRef<Path>, key: SigningKey) -> Result<Self, JournalError> {
        Self::open_with(dir, key, JournalOptions::default())
    }

    pub fn open_with(dir: impl AsRef<Path>, key: SigningKey, opts: JournalOptions) -> Result<Self, JournalError> {
        let dir = dir.as_ref().to_path_buf();
        fsutil::ensure_state_dir(&dir)?;
        let lock = fsutil::take_lock(&dir)?;

        let mut files: Vec<PathBuf> = segments(&dir)?.into_iter().map(|(_, p)| p).collect();
        let active = dir.join(ACTIVE);
        if fs::symlink_metadata(&active).is_ok() {
            files.push(active.clone());
        }
        let sc = scan(&files, &key)?;
        let mut j = Journal {
            dir,
            key,
            opts,
            _lock: lock,
            records: sc.records,
            prev_hash: sc.prev_hash,
            active_len: 0,
            lost: crate::lost::load_marker(&active.with_file_name(MARKER))?,
            quarantined: None,
            poisoned: false,
            fail_next_write: false,
        };
        if let Some(info) = j.lost.as_mut().filter(|i| !i.recorded) {
            // Crash window or hand-written marker: never trust its numbers.
            recompute(&j.dir, info);
        }
        if let Some((fi, off, reason)) = sc.bad {
            if j.records.is_empty() {
                return Err(JournalError::Unverifiable { file: files[fi].display().to_string(), reason });
            }
            j.quarantine(&files, fi, off)?;
        }
        j.active_len = fs::symlink_metadata(&active).map_or(0, |m| m.len());
        j.finalize_marker()?;
        j.drop_stale_marker()?;
        Ok(j)
    }

    /// Quarantine the bad record and everything after it. Order matters for
    /// crash safety: harvest the facts and persist the marker first, then move
    /// bytes, then truncate; the signed record follows in `finalize_marker`.
    fn quarantine(&mut self, files: &[PathBuf], fi: usize, off: u64) -> Result<(), JournalError> {
        let ts = now();
        let mut corrupt = self.dir.join(format!("journal.corrupt.{ts}"));
        let mut n = 0;
        while fs::symlink_metadata(&corrupt).is_ok() {
            n += 1;
            corrupt = self.dir.join(format!("journal.corrupt.{ts}.{n}"));
        }
        let cname = corrupt.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let bad = &files[fi];
        let mut sources = vec![(bad.clone(), off)];
        let mut planned = vec![corrupt.clone()];
        let mut moves = Vec::new();
        for later in &files[fi + 1..] {
            let name = later.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let dest = self.dir.join(format!("{cname}.{name}"));
            sources.push((later.clone(), 0));
            planned.push(dest.clone());
            moves.push((later.clone(), dest));
        }
        let (count, hw, ids) = harvest(&sources);
        let mut info = match self.lost.take() {
            Some(i) if !i.recorded => i,
            Some(i) => LostInfo { quarantine: i.quarantine, ..LostInfo::default() },
            None => LostInfo::default(),
        };
        let seq = self.records.len() as u64;
        info.lost_from_seq = if info.lost_count == 0 { seq } else { info.lost_from_seq.min(seq) };
        info.lost_count += count;
        info.raw_serial_high_water = info.raw_serial_high_water.max(hw);
        info.revoked_user_ids.extend(ids);
        info.revoked_user_ids.sort();
        info.revoked_user_ids.dedup();
        info.quarantine.extend(planned.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()));
        info.quarantine = crate::lost::sanitize_names(std::mem::take(&mut info.quarantine));
        info.ts = ts;
        info.recorded = false;
        crate::lost::write_marker(&self.dir, &info)?;
        self.lost = Some(info);

        let mut src = fsutil::open_file(bad, false, false, false)?;
        src.seek(SeekFrom::Start(off))?;
        let mut out = fsutil::open_file(&corrupt, true, true, false)?;
        std::io::copy(&mut src, &mut out)?;
        out.sync_all()?;
        for (from, to) in &moves {
            fs::rename(from, to)?;
        }
        if off == 0 {
            fs::remove_file(bad)?;
        } else {
            let f = fsutil::open_file(bad, false, true, false)?;
            f.set_len(off)?;
            f.sync_all()?;
            let active = self.dir.join(ACTIVE);
            if *bad != active {
                fs::rename(bad, &active)?;
            }
        }
        fsutil::sync_dir(&self.dir);
        self.quarantined = Some(corrupt);
        Ok(())
    }

    /// A recorded, parseable marker with nothing pending is stale (the accept
    /// record landed but the removal did not): remove it. Never touches an
    /// unrecorded marker (the only gate before the record exists) or an
    /// unreadable one (an admin must clear it explicitly).
    fn drop_stale_marker(&mut self) -> Result<(), JournalError> {
        let stale = self.lost.as_ref().is_some_and(|i| i.recorded && !i.unreadable);
        if stale && self.pending_quarantines().is_empty() {
            let m = self.dir.join(MARKER);
            if fs::symlink_metadata(&m).is_ok() {
                fs::remove_file(&m)?;
                fsutil::sync_dir(&self.dir);
            }
            self.lost = None;
        }
        Ok(())
    }

    /// Journal the signed `journal.quarantine` record for an unrecorded
    /// marker (allowed while read-only), clamping the harvested high-water mark
    /// to what the lost lines could plausibly have issued: a forged serial in
    /// an unverified tail must not be able to exhaust the serial space.
    fn finalize_marker(&mut self) -> Result<(), JournalError> {
        let Some(mut info) = self.lost.clone().filter(|i| !i.recorded) else { return Ok(()) };
        let floor = serial_floor(&self.records);
        info.serial_high_water = info.raw_serial_high_water.min(floor.saturating_add(info.lost_count));
        let body = serde_json::to_value(&info)?;
        self.append_raw(now(), KIND_QUARANTINE, body)?;
        info.recorded = true;
        crate::lost::write_marker(&self.dir, &info)?;
        self.lost = Some(info);
        Ok(())
    }

    /// Newest record's position, `None` for an empty journal.
    pub fn head(&self) -> Option<Head> {
        self.records.last().map(|r| Head { seq: r.seq, hash: hexser::encode(&self.prev_hash) })
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Record> {
        self.records.iter()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// True while a quarantine is unacknowledged. Derived from the signed
    /// chain (a `journal.quarantine` record with no later
    /// `journal.accept_truncate`), or from the marker file when that covers the
    /// window before the record is appended. Deleting the marker alone cannot
    /// lift it.
    pub fn read_only(&self) -> bool {
        self.lost.is_some() || !self.pending_quarantines().is_empty()
    }

    /// Seqs of `journal.quarantine` records newer than the newest accepted
    /// quarantine (oldest first). One acceptance of the latest clears them all.
    pub fn pending_quarantines(&self) -> Vec<u64> {
        let accepted = self
            .records
            .iter()
            .filter(|r| r.kind == KIND_ACCEPT_TRUNCATE)
            .filter_map(|r| r.body["quarantine_seq"].as_u64())
            .max();
        self.records
            .iter()
            .filter(|r| r.kind == KIND_QUARANTINE && accepted.is_none_or(|a| r.seq > a))
            .map(|r| r.seq)
            .collect()
    }

    /// The quarantine an acceptance must name: the newest pending one.
    pub fn latest_pending_quarantine(&self) -> Option<u64> {
        self.pending_quarantines().last().copied()
    }

    /// What the unacknowledged quarantine lost, if anything.
    pub fn lost(&self) -> Option<&LostInfo> {
        self.lost.as_ref()
    }

    /// Where the bad tail went when it was quarantined by *this* open.
    pub fn quarantined(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    /// True after a failed write whose rollback also failed.
    pub fn poisoned(&self) -> bool {
        self.poisoned
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Remove the marker once the acceptance record is journalled and no other
    /// quarantine is still pending.
    pub(crate) fn clear_lost(&mut self, _ack: &AdminAck) -> Result<(), JournalError> {
        if !self.pending_quarantines().is_empty() {
            return Ok(());
        }
        let m = self.dir.join(MARKER);
        if fs::symlink_metadata(&m).is_ok() {
            fs::remove_file(&m)?;
            fsutil::sync_dir(&self.dir);
        }
        self.lost = None;
        Ok(())
    }

    /// Test seam: the next append behaves as a partial write whose rollback fails.
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    pub fn inject_write_failure(&mut self) {
        self.fail_next_write = true;
    }

    /// Append a non-binding record (peer.*, policy.*, facts.*, service.*...)
    /// stamped with the current time. `user.*` kinds are refused: only the
    /// bindings writer may produce them, so a semantically invalid record can
    /// never be forged through this door.
    pub fn append(&mut self, kind: &str, body: serde_json::Value) -> Result<Head, JournalError> {
        self.append_at(now(), kind, body)
    }

    /// As [`Journal::append`] with an explicit timestamp.
    pub fn append_at(&mut self, ts: u64, kind: &str, body: serde_json::Value) -> Result<Head, JournalError> {
        if reserved(kind) {
            return Err(JournalError::ReservedKind(kind.to_string()));
        }
        self.append_raw(ts, kind, body)
    }

    pub(crate) fn append_raw(&mut self, ts: u64, kind: &str, body: serde_json::Value) -> Result<Head, JournalError> {
        if self.poisoned {
            return Err(JournalError::Poisoned);
        }
        if self.active_len > 0 && self.active_len >= self.opts.max_segment_bytes {
            self.rotate()?;
        }
        let seq = self.records.len() as u64;
        let prev = if seq == 0 { ZERO_PREV.to_string() } else { hexser::encode(&self.prev_hash) };
        let unsigned = serde_json::to_string(&Unsigned { v: RECORD_VERSION, seq, ts, prev: &prev, kind, body: &body })?;
        let mut msg = Vec::with_capacity(DOMAIN.len() + unsigned.len());
        msg.extend_from_slice(DOMAIN);
        msg.extend_from_slice(unsigned.as_bytes());
        let sig = hexser::encode(&self.key.sign(&msg).to_bytes());
        let mut line = unsigned;
        line.pop(); // closing brace
        line.push_str(&format!(",\"sig\":\"{sig}\"}}"));
        if line.len() > MAX_RECORD_BYTES {
            return Err(JournalError::RecordTooLarge(line.len()));
        }

        let path = self.dir.join(ACTIVE);
        let created = fs::symlink_metadata(&path).is_err();
        let mut f = fsutil::open_file(&path, true, true, true)?;
        let mut bytes = line.clone().into_bytes();
        bytes.push(b'\n');
        let injected = std::mem::take(&mut self.fail_next_write);
        let res = if injected {
            Err(std::io::Error::other("injected write failure"))
        } else {
            f.write_all(&bytes).and_then(|_| f.sync_data())
        };
        if let Err(e) = res {
            if injected || f.set_len(self.active_len).and_then(|_| f.sync_data()).is_err() {
                self.poisoned = true;
            }
            return Err(e.into());
        }
        if created {
            fsutil::sync_dir(&self.dir);
        }
        self.active_len += bytes.len() as u64;
        self.prev_hash = Sha256::digest(line.as_bytes()).into();
        let head = Head { seq, hash: hexser::encode(&self.prev_hash) };
        self.records.push(Record { v: RECORD_VERSION, seq, ts, prev, kind: kind.into(), body, sig });
        Ok(head)
    }

    /// Close the active file as `journal.NNN.jsonl`; the chain carries over
    /// because the next record's `prev` is the last line's hash.
    fn rotate(&mut self) -> Result<(), JournalError> {
        let next = segments(&self.dir)?.last().map_or(0, |(n, _)| n + 1);
        fs::rename(self.dir.join(ACTIVE), self.dir.join(format!("journal.{next:03}.jsonl")))?;
        fsutil::sync_dir(&self.dir);
        self.active_len = 0;
        Ok(())
    }
}
