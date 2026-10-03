//! Durable log behind a project's ingest store (ADR-100 decision 5).
//!
//! The owner daemon keeps each project's vectors in an in-memory index
//! ([`super::store::VectorBackendStore`]); this log is what lets a restart
//! rebuild it. One file per project under the daemon runtime dir.
//!
//! Layout: an 8-byte magic, then one frame per accepted batch:
//! `u32 payload_len | payload | u32 crc`, where `crc` is the first four bytes
//! of `blake3(payload)` and the payload is `u32 count` followed by records
//! `u16 instance_len | instance | u16 node_len | node | u64 id | DIMS * f32`
//! (all little endian). A batch is written with one `write_all` and synced
//! before the ingest is acknowledged, so a crash leaves whole batches plus at
//! most one torn frame at the tail; [`VectorLog::open`] drops a torn or
//! corrupt tail and truncates the file back to the last good frame.
//!
//! The file is capped: a batch that would take it past the cap is refused up
//! front (the store reports it full) and nothing is written. The log only
//! grows; there is no compaction yet, so repeated upserts of one id consume
//! space until the cap.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::types::DIMS;

/// File magic and format version.
const MAGIC: &[u8; 8] = b"WCIVEC01";
/// Default cap on one project's log file.
pub const DEFAULT_MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;
/// Largest instance or node id a record may carry.
const MAX_NAME: usize = 256;

/// One accepted vector as persisted.
#[derive(Debug, Clone, PartialEq)]
pub struct LogRecord {
    /// Instance that posted it.
    pub instance: String,
    /// Node whose bridge forwarded it.
    pub node: String,
    /// The id the instance gave it.
    pub id: u64,
    /// The vector.
    pub values: [f32; DIMS],
}

impl LogRecord {
    /// Encoded size of this record.
    pub fn encoded_len(&self) -> u64 {
        (2 + self.instance.len() + 2 + self.node.len() + 8 + DIMS * 4) as u64
    }

    /// Whether the ids fit the format (they are bounded by the bridge's
    /// validation; this keeps the file reader's bound honest).
    pub fn is_encodable(&self) -> bool {
        self.instance.len() <= MAX_NAME && self.node.len() <= MAX_NAME
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.instance.len() as u16).to_le_bytes());
        out.extend_from_slice(self.instance.as_bytes());
        out.extend_from_slice(&(self.node.len() as u16).to_le_bytes());
        out.extend_from_slice(self.node.as_bytes());
        out.extend_from_slice(&self.id.to_le_bytes());
        for f in &self.values {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
}

/// Bytes a batch of `records` adds to the file (frame overhead included).
pub fn frame_len(records: &[LogRecord]) -> u64 {
    // u32 len + u32 count + u32 crc
    12 + records.iter().map(LogRecord::encoded_len).sum::<u64>()
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        (self.0.len() >= n).then(|| {
            let (a, b) = self.0.split_at(n);
            self.0 = b;
            a
        })
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn name(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        if n > MAX_NAME {
            return None;
        }
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn record(&mut self) -> Option<LogRecord> {
        let instance = self.name()?;
        let node = self.name()?;
        let id = u64::from_le_bytes(self.take(8)?.try_into().ok()?);
        let mut values = [0f32; DIMS];
        for v in &mut values {
            *v = f32::from_le_bytes(self.take(4)?.try_into().ok()?);
        }
        Some(LogRecord { instance, node, id, values })
    }
}

fn crc(payload: &[u8]) -> [u8; 4] {
    let h = blake3::hash(payload);
    [h.as_bytes()[0], h.as_bytes()[1], h.as_bytes()[2], h.as_bytes()[3]]
}

/// What follows the last good frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tail {
    /// Nothing: the file ends on a frame boundary.
    Clean,
    /// An incomplete frame at the end of the file: what a crash mid-append leaves.
    Torn,
    /// A complete frame that fails its checksum or does not parse. Anything
    /// after it may still hold good data.
    Corrupt,
}

/// Decode frames from `data` (after the magic). Returns the records, the
/// byte offset (into `data`) of the end of the last good frame, and what
/// follows it.
fn decode_frames(data: &[u8]) -> (Vec<LogRecord>, usize, Tail) {
    let mut out = Vec::new();
    let mut good = 0usize;
    let mut cur = Cursor(data);
    let tail = loop {
        if cur.0.is_empty() {
            break Tail::Clean;
        }
        let Some(len) = cur.u32() else { break Tail::Torn };
        let Some(payload) = cur.take(len as usize) else { break Tail::Torn };
        let Some(sum) = cur.take(4) else { break Tail::Torn };
        if sum != crc(payload) {
            break Tail::Corrupt;
        }
        let mut p = Cursor(payload);
        let Some(count) = p.u32() else { break Tail::Corrupt };
        let mut batch = Vec::new();
        let mut ok = true;
        for _ in 0..count {
            match p.record() {
                Some(r) => batch.push(r),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok || !p.0.is_empty() {
            break Tail::Corrupt;
        }
        out.extend(batch);
        good = data.len() - cur.0.len();
    };
    (out, good, tail)
}

/// Why a log could not be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    /// The file is not a usable log (wrong magic, over the cap). Safe to move aside.
    #[error("{0}")]
    Corrupt(String),
    /// A filesystem error that may be transient; the file is not implicated.
    #[error("{0}")]
    Io(String),
}

/// Run blocking file work without stalling an async worker: on a
/// multi-threaded tokio runtime the worker is handed off (`block_in_place`),
/// anywhere else the closure just runs.
pub fn run_blocking<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// An open, append-only vector log.
pub struct VectorLog {
    path: PathBuf,
    file: File,
    size: u64,
    cap: u64,
}

impl VectorLog {
    /// Open (creating if absent) the log at `path` and return every record it
    /// holds. A torn tail (a crash mid-append) is dropped and the file
    /// truncated back to the last good frame. A complete frame that fails its
    /// checksum is corruption: a warning with the dropped byte count is logged,
    /// the dropped tail is first copied to `<file>.corrupt.<unix ts>`, and the
    /// file is truncated so appends resume on a frame boundary. A file with
    /// the wrong magic, or larger than `cap`, is [`OpenError::Corrupt`] and is
    /// left untouched.
    pub fn open(path: &Path, cap: u64) -> Result<(Self, Vec<LogRecord>), OpenError> {
        let io = |e: io::Error| OpenError::Io(format!("{}: {e}", path.display()));
        if let Some(parent) = path.parent() {
            create_private_dir(parent).map_err(io)?;
        }
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(path).map_err(io)?;
        let len = file.metadata().map_err(io)?.len();
        if len > cap {
            return Err(OpenError::Corrupt(format!(
                "{} is {len} bytes, over the {cap} byte cap",
                path.display()
            )));
        }
        let mut data = Vec::with_capacity(len as usize);
        file.read_to_end(&mut data).map_err(io)?;
        let (records, size) = if data.is_empty() {
            file.write_all(MAGIC).map_err(io)?;
            file.sync_data().map_err(io)?;
            (Vec::new(), MAGIC.len() as u64)
        } else if data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC {
            return Err(OpenError::Corrupt(format!("{} is not a vector log", path.display())));
        } else {
            let body = &data[MAGIC.len()..];
            let (records, good, tail) = decode_frames(body);
            let size = (MAGIC.len() + good) as u64;
            let dropped = body.len() - good;
            match tail {
                Tail::Clean => {}
                Tail::Torn => {
                    tracing::warn!(path = %path.display(), dropped, "vector log ends in a torn frame; dropping it");
                }
                Tail::Corrupt => {
                    let aside = path.with_extension(format!(
                        "{}.corrupt.{}",
                        path.extension().and_then(|e| e.to_str()).unwrap_or("vec"),
                        now_secs()
                    ));
                    tracing::warn!(
                        path = %path.display(), dropped, kept = %aside.display(),
                        "vector log has a corrupt frame; dropping it and everything after it (copy kept)"
                    );
                    // Keep the bytes before cutting them: they may hold good frames.
                    std::fs::write(&aside, &body[good..]).map_err(io)?;
                }
            }
            if tail != Tail::Clean {
                file.set_len(size).map_err(io)?;
                file.sync_data().map_err(io)?;
            }
            (records, size)
        };
        file.seek(SeekFrom::Start(size)).map_err(io)?;
        Ok((Self { path: path.to_path_buf(), file, size, cap }, records))
    }

    /// Whether a batch of `records` fits under the cap.
    pub fn has_room_for(&self, records: &[LogRecord]) -> bool {
        records.is_empty() || self.size + frame_len(records) <= self.cap
    }

    /// Append one batch as a single frame and sync it. Nothing is written
    /// when the frame would pass the cap.
    pub fn append(&mut self, records: &[LogRecord]) -> io::Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        if !self.has_room_for(records) {
            return Err(io::Error::other(format!(
                "{} would pass its {} byte cap",
                self.path.display(),
                self.cap
            )));
        }
        let mut payload = Vec::with_capacity(frame_len(records) as usize);
        payload.extend_from_slice(&(records.len() as u32).to_le_bytes());
        for r in records {
            r.encode(&mut payload);
        }
        let mut frame = Vec::with_capacity(payload.len() + 8);
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(&crc(&payload));
        self.file.write_all(&frame)?;
        self.file.sync_data()?;
        self.size += frame.len() as u64;
        Ok(())
    }

    /// Bytes in the file.
    pub fn size(&self) -> u64 {
        self.size
    }
}
