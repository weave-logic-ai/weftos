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

use clawft_kernel::gate::GateBackend;
use clawft_kernel::infer_proxy::{ExposureAuth, ExposurePermit};
use clawft_kernel::workload_governance::WorkloadGate;
use serde_json::json;

/// Principal the exposure decision is made as.
pub const PRINCIPAL: &str = "infer-daemon";

/// Read the bearer token at `rel` under `dir`.
pub fn read_token(dir: &Path, rel: &str) -> Result<ExposureAuth, String> {
    let path = dir.join(rel);
    let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("token file {rel}: {e}"))?;
    if !meta.file_type().is_file() {
        return Err(format!("token file {rel} is not a regular file"));
    }
    let me = nix::unistd::geteuid().as_raw();
    if meta.uid() != me {
        return Err(format!("token file {rel} is not owned by the daemon's user"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(format!("token file {rel} must not be readable by group or others (chmod 600)"));
    }
    if meta.len() > 4096 {
        return Err(format!("token file {rel} is too large"));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("token file {rel}: {e}"))?;
    ExposureAuth::new(&text).map_err(|e| format!("token file {rel}: {e}"))
}

/// Ask the gate whether `role`'s proxy may listen beyond loopback.
pub fn permit_for(gate: &WorkloadGate, role: &str) -> Result<ExposurePermit, String> {
    let ctx = json!({"workload": {
        "kind": "inference",
        "package_trust": "operator_attested",
        "node_tier": "pinned",
        "network": "lan",
        "secrets": false,
        "emulated": false,
        "resource_cost": 0.1,
        "package_id": format!("inference-expose:{role}"),
        "signer_keys": [],
        "artifact_hashes": [],
    }});
    let d = gate.check(PRINCIPAL, "workload.start", &ctx);
    ExposurePermit::from_decision(&d).ok_or_else(|| match d {
        clawft_kernel::gate::GateDecision::Deny { reason, .. } => format!("governance denied listening beyond loopback: {reason}"),
        clawft_kernel::gate::GateDecision::Defer { reason } => format!("listening beyond loopback is deferred to a human: {reason}"),
        _ => "no governance permit".to_string(),
    })
}
