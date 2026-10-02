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
use std::io::{BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clawft_mesh_local::{hexser, Principal};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fsutil::{self, Line, MAX_RECORD_BYTES};
use crate::lost::{harvest, LostInfo};

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
const ZERO_PREV: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// `,"sig":"` + 128 hex + `"}`
const SIG_TAIL_LEN: usize = 8 + 128 + 2;

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

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Kinds only the bindings writer may append.
fn reserved(kind: &str) -> bool {
    kind.starts_with("user.") || kind == KIND_ACCEPT_TRUNCATE
}

/// Numbered segments, ascending: `journal.NNN.jsonl`.
fn segments(dir: &Path) -> std::io::Result<Vec<(u32, PathBuf)>> {
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

/// Verify one line (without its newline) against the expected position.
fn verify_line(line: &[u8], seq: u64, prev_hash: &[u8; 32], key: &SigningKey) -> Result<Record, String> {
    if line.len() <= SIG_TAIL_LEN {
        return Err("line too short".into());
    }
    let split = line.len() - SIG_TAIL_LEN;
    let (head, tail) = line.split_at(split);
    if &tail[..8] != b",\"sig\":\"" || &tail[SIG_TAIL_LEN - 2..] != b"\"}" {
        return Err("malformed signature field".into());
    }
    let sig_hex = std::str::from_utf8(&tail[8..8 + 128]).map_err(|_| "signature not utf-8")?;
    let sig_bytes = hexser::decode::<64>(sig_hex).ok_or("signature not lowercase hex")?;
    let mut msg = Vec::with_capacity(DOMAIN.len() + split + 1);
    msg.extend_from_slice(DOMAIN);
    msg.extend_from_slice(head);
    msg.push(b'}');
    key.verifying_key()
        .verify(&msg, &Signature::from_bytes(&sig_bytes))
        .map_err(|_| "bad signature")?;
    let rec: Record = serde_json::from_slice(line).map_err(|e| format!("unparseable: {e}"))?;
    if rec.v != RECORD_VERSION {
        return Err(format!("unsupported record version {}", rec.v));
    }
    if rec.seq != seq {
        return Err(format!("sequence {} where {} expected", rec.seq, seq));
    }
    if rec.prev != hexser::encode(prev_hash) {
        return Err("hash chain broken".into());
    }
    Ok(rec)
}

struct Scan {
    records: Vec<Record>,
    prev_hash: [u8; 32],
    /// File index, byte offset and reason of the first bad record.
    bad: Option<(usize, u64, String)>,
}

fn scan(files: &[PathBuf], key: &SigningKey) -> Result<Scan, JournalError> {
    let mut s = Scan { records: Vec::new(), prev_hash: [0u8; 32], bad: None };
    let mut buf = Vec::new();
    'files: for (fi, path) in files.iter().enumerate() {
        let mut r = BufReader::new(fsutil::open_file(path, false, false, false)?);
        let mut off = 0u64;
        loop {
            let (kind, n) = fsutil::read_line(&mut r, &mut buf, MAX_RECORD_BYTES)?;
            let reason = match kind {
                Line::Eof => break,
                Line::Torn => "torn final line (no newline)".to_string(),
                Line::TooLong => "record exceeds the size limit".to_string(),
                Line::Complete => match verify_line(&buf, s.records.len() as u64, &s.prev_hash, key) {
                    Ok(rec) => {
                        s.prev_hash = Sha256::digest(&buf).into();
                        s.records.push(rec);
                        off += n as u64;
                        continue;
                    }
                    Err(reason) => reason,
                },
            };
            s.bad = Some((fi, off, reason));
            break 'files;
        }
    }
    Ok(s)
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
        if let Some((fi, off, reason)) = sc.bad {
            if j.records.is_empty() {
                return Err(JournalError::Unverifiable { file: files[fi].display().to_string(), reason });
            }
            j.quarantine(&files, fi, off)?;
        }
        j.active_len = fs::symlink_metadata(&active).map_or(0, |m| m.len());
        Ok(j)
    }

    /// Move the bad record and everything after it into `journal.corrupt.<ts>`,
    /// persist the marker, then truncate.
    fn quarantine(&mut self, files: &[PathBuf], fi: usize, off: u64) -> Result<(), JournalError> {
        let ts = now();
        let mut corrupt = self.dir.join(format!("journal.corrupt.{ts}"));
        let mut n = 0;
        while fs::symlink_metadata(&corrupt).is_ok() {
            n += 1;
            corrupt = self.dir.join(format!("journal.corrupt.{ts}.{n}"));
        }
        let bad = &files[fi];
        let mut src = fsutil::open_file(bad, false, false, false)?;
        src.seek(SeekFrom::Start(off))?;
        let mut out = fsutil::open_file(&corrupt, true, true, false)?;
        std::io::copy(&mut src, &mut out)?;
        out.sync_all()?;
        let mut lost_paths = vec![corrupt.clone()];
        let cname = corrupt.file_name().unwrap_or_default().to_string_lossy().into_owned();
        for later in &files[fi + 1..] {
            let name = later.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let dest = self.dir.join(format!("{cname}.{name}"));
            fs::rename(later, &dest)?;
            lost_paths.push(dest);
        }
        let (count, hw, ids) = harvest(&lost_paths);
        let mut info = self.lost.take().unwrap_or_default();
        info.lost_from_seq = if info.lost_count == 0 { self.records.len() as u64 } else { info.lost_from_seq.min(self.records.len() as u64) };
        info.lost_count += count;
        info.serial_high_water = info.serial_high_water.max(hw);
        info.revoked_user_ids.extend(ids);
        info.revoked_user_ids.sort();
        info.revoked_user_ids.dedup();
        info.quarantine.extend(lost_paths.iter().map(|p| p.display().to_string()));
        info.ts = ts;
        crate::lost::write_marker(&self.dir, &info)?;
        self.lost = Some(info);

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

    /// True while an unacknowledged quarantine marker exists (durable).
    pub fn read_only(&self) -> bool {
        self.lost.is_some()
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

    /// Remove the marker once the acceptance record is journalled.
    pub(crate) fn clear_lost(&mut self, _ack: &AdminAck) -> Result<(), JournalError> {
        let m = self.dir.join(MARKER);
        if fs::symlink_metadata(&m).is_ok() {
            fs::remove_file(&m)?;
            fsutil::sync_dir(&self.dir);
        }
        self.lost = None;
        Ok(())
    }

    /// Test seam: the next append behaves as a partial write whose rollback fails.
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
