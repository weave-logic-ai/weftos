//! Chain verification: per-line hash/signature checks and the bounded scan.

use std::io::BufReader;
use std::path::PathBuf;

use clawft_mesh_local::hexser;
use ed25519_dalek::{Signature, SigningKey, Verifier};
use sha2::{Digest, Sha256};

use crate::fsutil::{self, Line, MAX_RECORD_BYTES};
use crate::journal::{JournalError, Record, DOMAIN, RECORD_VERSION};

/// `,"sig":"` + 128 hex + `"}`
const SIG_TAIL_LEN: usize = 8 + 128 + 2;

/// Verify one line (without its newline) against the expected position.
pub(crate) fn verify_line(line: &[u8], seq: u64, prev_hash: &[u8; 32], key: &SigningKey) -> Result<Record, String> {
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

pub(crate) struct Scan {
    pub records: Vec<Record>,
    pub prev_hash: [u8; 32],
    /// File index, byte offset and reason of the first bad record.
    pub bad: Option<(usize, u64, String)>,
}

pub(crate) fn scan(files: &[PathBuf], key: &SigningKey) -> Result<Scan, JournalError> {
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

/// Result of a read-only verification pass over a state directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub records: u64,
    pub head_seq: Option<u64>,
    pub head_hash: Option<String>,
    /// First bad record (`file`, `reason`); `None` when the whole chain verifies.
    pub bad: Option<(String, String)>,
}

/// Verify every hash link and signature under `dir` without modifying or
/// locking anything (unlike [`crate::Journal::open`], which quarantines a bad
/// tail). Callers that race a writer must serialise with it.
pub fn verify_dir(dir: &std::path::Path, key: &SigningKey) -> Result<VerifyReport, JournalError> {
    let mut files: Vec<PathBuf> = crate::journal::segments(dir)?.into_iter().map(|(_, p)| p).collect();
    let active = dir.join(crate::journal::ACTIVE);
    if std::fs::symlink_metadata(&active).is_ok() {
        files.push(active);
    }
    let sc = scan(&files, key)?;
    Ok(VerifyReport {
        records: sc.records.len() as u64,
        head_seq: sc.records.last().map(|r| r.seq),
        head_hash: (!sc.records.is_empty()).then(|| hexser::encode(&sc.prev_hash)),
        bad: sc.bad.map(|(fi, _, reason)| (files[fi].display().to_string(), reason)),
    })
}
