//! The durable quarantine marker and what a quarantined tail is known to hold.
//!
//! The marker only *gates* read-only across restarts and records where the lost
//! bytes went. The facts that constrain the state (serial floor, revoked
//! users) live in the signed `journal.quarantine` record; numbers in a marker
//! are recomputed from the quarantine files whenever they would be acted on.

use std::fs;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsutil::{self, Line, MAX_RECORD_BYTES};
use crate::journal::{JournalError, MARKER};

fn yes() -> bool {
    true
}

/// What a quarantined tail is known to have contained. Harvested from the
/// unverified lost lines, so it is a hint that can only make the service more
/// conservative; the quarantine file itself is the evidence an admin must
/// review. `serial_high_water` is clamped (see `Journal::finalize_marker`);
/// the raw value is informational.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LostInfo {
    pub quarantine: Vec<String>,
    pub lost_from_seq: u64,
    pub lost_count: u64,
    pub serial_high_water: u64,
    pub raw_serial_high_water: u64,
    pub revoked_user_ids: Vec<String>,
    pub ts: u64,
    /// Whether the signed `journal.quarantine` record for this quarantine is
    /// in the chain. A marker without the field is a pure read-only gate.
    #[serde(default = "yes")]
    pub recorded: bool,
}

/// Lenient pass over (file, start offset) sources: (line count, raw serial
/// high-water, user ids named by revoke records). Over-long and unparseable
/// lines are skipped, never fatal.
pub(crate) fn harvest(sources: &[(PathBuf, u64)]) -> (u64, u64, Vec<String>) {
    let (mut count, mut hw, mut ids) = (0u64, 0u64, Vec::new());
    let mut buf = Vec::new();
    for (p, start) in sources {
        let Ok(mut f) = fsutil::open_file(p, false, false, false) else { continue };
        if f.seek(SeekFrom::Start(*start)).is_err() {
            continue;
        }
        let mut r = BufReader::new(f);
        while let Ok((kind, _)) = fsutil::read_line(&mut r, &mut buf, MAX_RECORD_BYTES) {
            match kind {
                Line::Eof => break,
                Line::TooLong => {
                    count += 1;
                    if fsutil::skip_line(&mut r).is_err() {
                        break;
                    }
                    continue;
                }
                Line::Complete | Line::Torn => count += 1,
            }
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

/// Re-derive an unrecorded marker's numbers from the files it names (basenames
/// only, inside `dir`), ignoring whatever numbers the marker file claimed.
pub(crate) fn recompute(dir: &Path, info: &mut LostInfo) {
    let sources: Vec<(PathBuf, u64)> = info
        .quarantine
        .iter()
        .filter_map(|q| Path::new(q).file_name().map(|n| (dir.join(n), 0)))
        .collect();
    let (count, hw, mut ids) = harvest(&sources);
    ids.sort();
    ids.dedup();
    info.lost_count = count;
    info.raw_serial_high_water = hw;
    info.revoked_user_ids = ids;
}

/// Keep only plain basenames of quarantine files (`journal.corrupt.*`), dedup,
/// and bound the list so the marker stays far below its read limit.
pub(crate) fn sanitize_names(names: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let base = Path::new(&n).file_name().map(|b| b.to_string_lossy().into_owned());
        let ok = |b: &String| *b == n && b.starts_with("journal.corrupt.") && !out.contains(b);
        if let Some(b) = base.filter(ok) {
            out.push(b);
        }
        if out.len() >= 1024 {
            break;
        }
    }
    out
}

pub(crate) fn load_marker(path: &Path) -> Result<Option<LostInfo>, JournalError> {
    if fs::symlink_metadata(path).is_err() {
        return Ok(None);
    }
    let f = fsutil::open_file(path, false, false, false)?;
    let mut data = Vec::new();
    f.take(1 << 20).read_to_end(&mut data)?;
    // An unreadable marker still means "read-only": keep the journal safe.
    let mut info: LostInfo = serde_json::from_slice(&data)
        .unwrap_or_else(|_| LostInfo { recorded: true, ..LostInfo::default() });
    info.quarantine = sanitize_names(std::mem::take(&mut info.quarantine));
    Ok(Some(info))
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
