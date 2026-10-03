//! Listening beyond loopback (card mesh-placement-20; ADR-101 section 7).
//! The model server never leaves loopback; only a role's proxy may, and only
//! with a bearer token and a governance permit the gate chained:
//!
//! - the token is read from a file under the runtime dir that is a regular
//!   file (not a link), owned by the daemon's user, not readable by group
//!   or others, and long enough; it is never logged, chained or forwarded;
//! - the permit is a decision of the daemon's [`WorkloadGate`] for a
//!   `workload.start` of kind `inference` with `network: lan` and the
//!   package id `inference-expose:<role>`. With no matching permit in
//!   `workload-permits.json` the gate denies (default deny) and the proxy
//!   stays unbound.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use clawft_kernel::infer_proxy::{ExposureAuth, ExposurePermit};
use clawft_kernel::workload_governance::WorkloadGate;

/// Principal the exposure decision is made as.
pub const PRINCIPAL: &str = "infer-daemon";

/// Every directory between the runtime dir and the token (exclusive of the
/// runtime dir) is a real directory the daemon's user owns that others
/// cannot write to: nobody else can swap the file or a link in its path.
fn check_dirs(dir: &Path, rel: &Path, me: u32) -> Result<(), String> {
    let mut cur = dir.to_path_buf();
    let comps: Vec<_> = rel.components().collect();
    for c in &comps[..comps.len().saturating_sub(1)] {
        cur.push(c);
        let m = std::fs::symlink_metadata(&cur).map_err(|e| format!("token directory {}: {e}", cur.display()))?;
        if !m.file_type().is_dir() {
            return Err(format!("token directory {} is not a real directory", cur.display()));
        }
        if m.uid() != me {
            return Err(format!("token directory {} is not owned by the daemon's user", cur.display()));
        }
        if m.mode() & 0o022 != 0 {
            return Err(format!("token directory {} must not be writable by group or others", cur.display()));
        }
    }
    Ok(())
}

/// Read the bearer token at `rel` under `dir`. The file is opened without
/// following a link and checked through the open descriptor (not by path),
/// so what was checked is what is read.
pub fn read_token(dir: &Path, rel: &str) -> Result<ExposureAuth, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let me = nix::unistd::geteuid().as_raw();
    check_dirs(dir, Path::new(rel), me)?;
    let path = dir.join(rel);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(&path)
        .map_err(|e| format!("token file {rel}: {e} (a link is refused)"))?;
    let meta = file.metadata().map_err(|e| format!("token file {rel}: {e}"))?;
    if !meta.file_type().is_file() {
        return Err(format!("token file {rel} is not a regular file"));
    }
    if meta.uid() != me {
        return Err(format!("token file {rel} is not owned by the daemon's user"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(format!("token file {rel} must not be readable by group or others (chmod 600)"));
    }
    let mut text = String::new();
    file.take(4097).read_to_string(&mut text).map_err(|e| format!("token file {rel}: {e}"))?;
    if text.len() > 4096 {
        return Err(format!("token file {rel} is too large"));
    }
    ExposureAuth::new(&text).map_err(|e| format!("token file {rel}: {e}"))
}

/// Ask the gate whether `role`'s proxy may listen beyond loopback.
pub fn permit_for(gate: &WorkloadGate, role: &str) -> Result<ExposurePermit, String> {
    ExposurePermit::ask(gate, PRINCIPAL, role)
}
