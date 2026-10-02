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
//! after it is moved to `journal.corrupt.<ts>` and the journal comes up
//! read-only for binds until an admin calls [`Journal::accept_truncate`].
//! A bad *first* record is a hard error and nothing is modified (wrong key or
//! wrong directory, not a damaged tail).
//!
//! One writer: an exclusive `flock` on `mesh.lock`, holding the owner pid.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clawft_mesh_local::hexser;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Domain separation prefix for journal signatures.
pub const DOMAIN: &[u8] = b"weftos/mesh-journal/v1\0";
/// Schema version written into every record.
pub const RECORD_VERSION: u32 = 1;
/// Default segment size (plan 1.3): 8 MiB.
pub const DEFAULT_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
/// Active file name.
pub const ACTIVE: &str = "journal.jsonl";
const LOCK: &str = "mesh.lock";
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
    #[error("journal is read-only until `--accept-truncate`")]
    ReadOnly,
    #[error("record body: {0}")]
    Encode(#[from] serde_json::Error),
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
    read_only: bool,
    quarantined: Option<PathBuf>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

fn private_open(path: &Path, append: bool) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.create(true).read(true);
    if append {
        o.append(true);
    } else {
        o.write(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

#[cfg(unix)]
fn take_lock(dir: &Path) -> Result<File, JournalError> {
    use std::os::unix::io::AsRawFd;
    let path = dir.join(LOCK);
    let mut f = private_open(&path, false)?;
    // SAFETY: valid fd owned by `f` for the duration of the call.
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::WouldBlock {
            return Err(err.into());
        }
        let mut holder = None;
        for _ in 0..20 {
            let mut s = String::new();
            if let Ok(mut r) = File::open(&path) {
                let _ = r.read_to_string(&mut s);
            }
            holder = s.trim().parse::<u32>().ok();
            if holder.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        return Err(JournalError::Locked { holder_pid: holder });
    }
    f.set_len(0)?;
    write!(f, "{}", std::process::id())?;
    f.sync_data()?;
    Ok(f)
}

#[cfg(not(unix))]
fn take_lock(_dir: &Path) -> Result<File, JournalError> {
    Err(JournalError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "mesh.lock requires a unix host (Windows is design-only in Phase 3)",
    )))
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
fn verify_line(
    line: &[u8],
    seq: u64,
    prev_hash: &[u8; 32],
    key: &SigningKey,
) -> Result<Record, String> {
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
    /// Index into the file list and byte offset of the first bad record.
    bad: Option<(usize, usize, String)>,
}

fn scan(files: &[PathBuf], key: &SigningKey) -> Result<Scan, JournalError> {
    let mut s = Scan { records: Vec::new(), prev_hash: [0u8; 32], bad: None };
    'files: for (fi, path) in files.iter().enumerate() {
        let data = fs::read(path)?;
        let mut off = 0usize;
        while off < data.len() {
            let Some(nl) = data[off..].iter().position(|b| *b == b'\n') else {
                s.bad = Some((fi, off, "torn final line (no newline)".into()));
                break 'files;
            };
            let line = &data[off..off + nl];
            match verify_line(line, s.records.len() as u64, &s.prev_hash, key) {
                Ok(rec) => {
                    s.prev_hash = Sha256::digest(line).into();
                    s.records.push(rec);
                }
                Err(reason) => {
                    s.bad = Some((fi, off, reason));
                    break 'files;
                }
            }
            off += nl + 1;
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

    pub fn open_with(
        dir: impl AsRef<Path>,
        key: SigningKey,
        opts: JournalOptions,
    ) -> Result<Self, JournalError> {
        let dir = dir.as_ref().to_path_buf();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
            }
        }
        let lock = take_lock(&dir)?;

        let mut files: Vec<PathBuf> = segments(&dir)?.into_iter().map(|(_, p)| p).collect();
        let active = dir.join(ACTIVE);
        if active.exists() {
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
            read_only: false,
            quarantined: None,
        };
        if let Some((fi, off, reason)) = sc.bad {
            if j.records.is_empty() {
                return Err(JournalError::Unverifiable {
                    file: files[fi].display().to_string(),
                    reason,
                });
            }
            j.quarantine(&files, fi, off)?;
        }
        j.active_len = fs::metadata(&active).map_or(0, |m| m.len());
        Ok(j)
    }

    /// Move the bad record and everything after it into `journal.corrupt.<ts>`.
    fn quarantine(&mut self, files: &[PathBuf], fi: usize, off: usize) -> Result<(), JournalError> {
        let ts = now();
        let mut corrupt = self.dir.join(format!("journal.corrupt.{ts}"));
        let mut n = 0;
        while corrupt.exists() {
            n += 1;
            corrupt = self.dir.join(format!("journal.corrupt.{ts}.{n}"));
        }
        let data = fs::read(&files[fi])?;
        let mut out = private_open(&corrupt, false)?;
        out.write_all(&data[off..])?;
        out.sync_all()?;
        for later in &files[fi + 1..] {
            let name = later.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            fs::rename(later, self.dir.join(format!("{}.{name}", corrupt.file_name().unwrap().to_string_lossy())))?;
        }
        let bad = &files[fi];
        if off == 0 {
            fs::remove_file(bad)?;
        } else {
            let f = OpenOptions::new().write(true).open(bad)?;
            f.set_len(off as u64)?;
            f.sync_all()?;
            // The surviving prefix becomes the active file.
            let active = self.dir.join(ACTIVE);
            if *bad != active {
                fs::rename(bad, &active)?;
            }
        }
        sync_dir(&self.dir);
        self.read_only = true;
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

    /// True after a bad tail was quarantined and not yet accepted.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Where the bad tail went, if one was quarantined at open.
    pub fn quarantined(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    /// Admin acknowledgement of a truncation (`weaver mesh journal --accept-truncate`).
    pub fn accept_truncate(&mut self) {
        self.read_only = false;
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Append a record stamped with the current time.
    pub fn append(&mut self, kind: &str, body: serde_json::Value) -> Result<Head, JournalError> {
        self.append_at(now(), kind, body)
    }

    /// Append with an explicit timestamp (deterministic tests, replay tools).
    pub fn append_at(
        &mut self,
        ts: u64,
        kind: &str,
        body: serde_json::Value,
    ) -> Result<Head, JournalError> {
        if self.active_len > 0 && self.active_len >= self.opts.max_segment_bytes {
            self.rotate()?;
        }
        let seq = self.records.len() as u64;
        let prev = if seq == 0 { ZERO_PREV.to_string() } else { hexser::encode(&self.prev_hash) };
        let unsigned = serde_json::to_string(&Unsigned {
            v: RECORD_VERSION,
            seq,
            ts,
            prev: &prev,
            kind,
            body: &body,
        })?;
        let mut msg = Vec::with_capacity(DOMAIN.len() + unsigned.len());
        msg.extend_from_slice(DOMAIN);
        msg.extend_from_slice(unsigned.as_bytes());
        let sig = hexser::encode(&self.key.sign(&msg).to_bytes());
        let mut line = unsigned;
        line.pop(); // closing brace
        line.push_str(&format!(",\"sig\":\"{sig}\"}}"));

        let path = self.dir.join(ACTIVE);
        let created = !path.exists();
        let mut f = private_open(&path, true)?;
        let mut bytes = line.clone().into_bytes();
        bytes.push(b'\n');
        let res = f.write_all(&bytes).and_then(|_| f.sync_data());
        if let Err(e) = res {
            let _ = f.set_len(self.active_len);
            return Err(e.into());
        }
        if created {
            sync_dir(&self.dir);
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
        sync_dir(&self.dir);
        self.active_len = 0;
        Ok(())
    }
}
