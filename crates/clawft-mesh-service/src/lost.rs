//! The durable quarantine marker and what a quarantined tail is known to hold.

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsutil::{self, Line, MAX_RECORD_BYTES};
use crate::journal::{JournalError, MARKER};

/// What a quarantined tail is known to have contained. Harvested from the
/// unverified lost lines, so it is a hint that can only make the service more
/// conservative (higher serial floor, more keys treated as revoked); the
/// quarantine file itself is the evidence an admin must review.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LostInfo {
    pub quarantine: Vec<String>,
    pub lost_from_seq: u64,
    pub lost_count: u64,
    pub serial_high_water: u64,
    pub revoked_user_ids: Vec<String>,
    pub ts: u64,
}

/// Lenient pass over quarantined files: (line count, serial high-water,
/// user ids named by revoke records).
pub(crate) fn harvest(paths: &[PathBuf]) -> (u64, u64, Vec<String>) {
    let (mut count, mut hw, mut ids) = (0u64, 0u64, Vec::new());
    let mut buf = Vec::new();
    for p in paths {
        let Ok(f) = fsutil::open_file(p, false, false, false) else { continue };
        let mut r = BufReader::new(f);
        while let Ok((kind, _)) = fsutil::read_line(&mut r, &mut buf, MAX_RECORD_BYTES) {
            match kind {
                Line::Eof | Line::TooLong => break,
                Line::Complete | Line::Torn => {}
            }
            count += 1;
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf) else { continue };
            match v["kind"].as_str() {
                Some("user.cert.issue") => hw = hw.max(v["body"]["serial"].as_u64().unwrap_or(0)),
                Some("user.revoke") => {
                    if let Some(id) = v["body"]["user_id"].as_str() {
                        ids.push(id.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    (count, hw, ids)
}

pub(crate) fn load_marker(path: &Path) -> Result<Option<LostInfo>, JournalError> {
    if fs::symlink_metadata(path).is_err() {
        return Ok(None);
    }
    let f = fsutil::open_file(path, false, false, false)?;
    let mut data = Vec::new();
    f.take(1 << 20).read_to_end(&mut data)?;
    // An unreadable marker still means "read-only": keep the journal safe.
    Ok(Some(serde_json::from_slice(&data).unwrap_or_else(|_| LostInfo {
        quarantine: vec!["<unreadable marker>".into()],
        ..LostInfo::default()
    })))
}

pub(crate) fn write_marker(dir: &Path, info: &LostInfo) -> Result<(), JournalError> {
    let tmp = dir.join(format!("{MARKER}.tmp"));
    let _ = fs::remove_file(&tmp);
    let mut f = fsutil::open_file(&tmp, true, true, false)?;
    f.write_all(&serde_json::to_vec_pretty(info)?)?;
    f.sync_all()?;
    fs::rename(&tmp, dir.join(MARKER))?;
    fsutil::sync_dir(dir);
    Ok(())
}
