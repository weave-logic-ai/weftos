//! Trust root, rollback pin and revocation checks of a project kernel
//! (ADR-103 D8, package E). Split from `overlay_runtime`.

use clawft_types::project::ProjectCert;
use clawft_types::project::canon::{hex_decode, hex_encode};
use chrono::Utc;
use clawft_types::runtime_paths::RuntimePaths;

use crate::chain::ChainManager;
use crate::governance_overlay::{OverlayError, read_capped};
use crate::parent_policy::{ParentPolicy, ParentPolicyError, write_atomic_0600};

/// File (under `<root>/.weftos/state/`) holding the newest accepted parent
/// policy version.
pub const VERSION_PIN_FILE: &str = "parent-policy.version";
/// File in the user daemon's run dir (`<run>/<id>/user.pub`, outside the
/// project tree) pinning the trusted user key as 64 lowercase hex. Written by
/// the supervisor with [`write_user_pin`]. When present the child trusts only
/// this key and refuses a certificate issued by another. It assumes whatever
/// runs inside the project (the Phase 4 sandbox) cannot write the run dir;
/// when absent (Phase 2 development) the child falls back to the key in its
/// own certificate and logs a warning.
pub const USER_PIN_FILE: &str = "user.pub";
/// Marker the user daemon drops in the run dir when it revokes the project;
/// its presence stops boot, reload and update. Defined once in
/// `clawft_types::runtime_paths` (`revoked_marker`), shared with the writer
/// and the supervisor.
pub use clawft_types::runtime_paths::REVOKED_FILE;

/// What the project chain says about earlier governance applications.
#[derive(Debug, Default)]
pub(crate) struct History {
    /// Overlay hash of the newest `governance.overlay.applied` event.
    pub last_overlay_hash: Option<String>,
    /// Highest parent policy version any such event records.
    pub max_parent_version: Option<u64>,
    /// Some such event says a `user.pub` pin was in use.
    pub user_pin_used: bool,
    /// At least one such event exists.
    pub any_applied: bool,
}

pub(crate) fn chain_history(chain: &ChainManager) -> History {
    let mut h = History::default();
    for e in chain.tail(0).iter().filter(|e| e.kind == "governance.overlay.applied") {
        h.any_applied = true;
        let p = e.payload.as_ref();
        h.last_overlay_hash = Some(
            p.and_then(|p| p.get("overlay_hash"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
        );
        if let Some(v) = p.and_then(|p| p.get("parent_version")).and_then(|v| v.as_u64()) {
            h.max_parent_version = Some(h.max_parent_version.map_or(v, |m| m.max(v)));
        }
        if p.and_then(|p| p.get("user_pin")).and_then(|v| v.as_bool()) == Some(true) {
            h.user_pin_used = true;
        }
    }
    h
}

/// Write the user-key pin the child reads (supervisor side, package G).
pub fn write_user_pin(run_dir: &std::path::Path, user_pubkey: &[u8; 32]) -> std::io::Result<()> {
    write_atomic_0600(
        &run_dir.join(USER_PIN_FILE),
        format!("{}\n", hex_encode(user_pubkey)).as_bytes(),
    )
}

/// The user key this child trusts, after re-checking the certificate (shape,
/// issuer, expiry, project), the project key beside it, the out-of-tree pin
/// and the revocation marker. Runs at boot and again on every reload and
/// update, so an expired or revoked project stops taking policy.
/// Returns the key and whether it came from the `user.pub` pin.
pub(crate) fn load_user_pubkey(paths: &RuntimePaths) -> Result<([u8; 32], bool), OverlayError> {
    let cert_err = |m: String| OverlayError::Cert(m);
    let revoked = paths.revoked_marker();
    if revoked.exists() {
        return Err(OverlayError::Revoked(revoked.display().to_string()));
    }
    let path = paths
        .project_cert()
        .ok_or_else(|| cert_err("no certificate path for this root".into()))?;
    let text = read_capped(&path)?
        .ok_or_else(|| cert_err(format!("{} does not exist", path.display())))?;
    let cert: ProjectCert =
        serde_json::from_str(&text).map_err(|e| cert_err(format!("does not parse: {e}")))?;
    let cert_pk: [u8; 32] = hex_decode(&cert.user_pubkey)
        .ok_or_else(|| cert_err("`user_pubkey` is not 64 lowercase hex".into()))?;
    let pin_path = paths.root().join(USER_PIN_FILE);
    let mut pinned = false;
    let user_pk = match read_capped(&pin_path)? {
        Some(t) => {
            pinned = true;
            let pin: [u8; 32] = hex_decode(t.trim())
                .ok_or_else(|| cert_err(format!("{} is not 64 lowercase hex", pin_path.display())))?;
            if pin != cert_pk {
                return Err(cert_err(
                    "`user_pubkey` differs from the user.pub pin in the run dir".into(),
                ));
            }
            pin
        }
        None => {
            tracing::warn!(
                path = %pin_path.display(),
                "no user.pub pin: trusting the user key in the project certificate"
            );
            cert_pk
        }
    };
    cert.verify(&user_pk, Utc::now())
        .map_err(|e| cert_err(e.to_string()))?;
    if Some(cert.project_id.as_str()) != paths.child_id() {
        return Err(cert_err("`project_id` is not this kernel's project".into()));
    }
    // The key beside the certificate must be the certified one.
    if let Some(kp) = paths.project_key()
        && let Ok(bytes) = std::fs::read(&kp)
        && let Ok(seed) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        let pk = ed25519_dalek::SigningKey::from_bytes(&seed)
            .verifying_key()
            .to_bytes();
        if hex_encode(&pk) != cert.project_pubkey {
            return Err(cert_err("project.key is not the certified project key".into()));
        }
    }
    Ok((user_pk, pinned))
}

/// The pinned version: `None` when the file is absent; an unparsable file is
/// an error (never read as "no pin").
pub(crate) fn read_pin(paths: &RuntimePaths) -> Result<Option<u64>, OverlayError> {
    let Some(p) = paths.state_dir().map(|d| d.join(VERSION_PIN_FILE)) else {
        return Ok(None);
    };
    let bad = |reason: String| OverlayError::PinCorrupt {
        path: p.display().to_string(),
        reason,
    };
    match std::fs::read_to_string(&p) {
        Ok(t) => t
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| bad("not a version number".into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(bad(e.to_string())),
    }
}

pub(crate) fn write_pin(paths: &RuntimePaths, version: u64) -> Result<(), OverlayError> {
    let p = paths
        .state_dir()
        .ok_or(OverlayError::NotAChild)?
        .join(VERSION_PIN_FILE);
    write_atomic_0600(&p, version.to_string().as_bytes()).map_err(|e| OverlayError::Io {
        path: p.display().to_string(),
        reason: e.to_string(),
    })
}

pub(crate) fn check_version(parent: &ParentPolicy, pinned: Option<u64>) -> Result<(), OverlayError> {
    match pinned {
        Some(pinned) if parent.version < pinned => Err(ParentPolicyError::Rollback {
            have: parent.version,
            pinned,
        }
        .into()),
        _ => Ok(()),
    }
}

