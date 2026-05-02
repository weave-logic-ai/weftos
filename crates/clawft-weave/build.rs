//! Build-time stamp emission for the `weaver` binary.
//!
//! Captures the current git short hash + a UTC build timestamp and
//! exposes them to the crate via `env!("BUILD_GIT_HASH")` /
//! `env!("BUILD_TIMESTAMP")`. Used by the daemon's startup banner so
//! every boot makes which build is running visible in the first line
//! of stdout — saves the "is this the new binary?" guessing game when
//! debugging against a freshly-rebuilt daemon.
//!
//! Failure modes are non-fatal: if `git` isn't available (tarball
//! build, sandboxed CI) the hash falls back to `unknown`.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // Re-run when HEAD moves so the captured hash stays accurate.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");

    let hash = git_short_hash().unwrap_or_else(|| "unknown".to_string());
    let dirty = git_is_dirty().unwrap_or(false);
    let hash_full = if dirty {
        format!("{hash}-dirty")
    } else {
        hash
    };
    println!("cargo:rustc-env=BUILD_GIT_HASH={hash_full}");

    // ISO-8601-ish UTC stamp. Avoiding chrono in the build script to
    // keep build deps minimal — the formatting here is good enough for
    // a banner.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ts = format_unix_secs(secs);
    println!("cargo:rustc-env=BUILD_TIMESTAMP={ts}");
}

fn git_short_hash() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

fn git_is_dirty() -> Option<bool> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(!out.stdout.is_empty())
}

/// Convert unix-seconds to `YYYY-MM-DDTHH:MM:SSZ` UTC. Pure-stdlib so
/// the build script doesn't grow chrono / time deps.
fn format_unix_secs(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    let (y, mo, d) = days_to_ymd(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn days_to_ymd(days_since_epoch: i64) -> (i32, u32, u32) {
    // Days since 1970-01-01 → (year, month, day). Howard Hinnant's
    // civil_from_days algorithm — fast, exact, well-known.
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}
