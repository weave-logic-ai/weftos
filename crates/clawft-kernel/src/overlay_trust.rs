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
/// its presence stops boot, reload and update.
pub const REVOKED_FILE: &str = "revoked";

/// The overlay hash of the newest `governance.overlay.applied` event, if any.
pub(crate) fn last_applied_overlay_hash(chain: &ChainManager) -> Option<String> {
    chain
        .tail(0)
        .iter()
        .rev()
        .find(|e| e.kind == "governance.overlay.applied")
        .map(|e| {
            e.payload
                .as_ref()
                .and_then(|p| p.get("overlay_hash"))
                .and_then(|h| h.as_str())
                .unwrap_or_default()
                .to_owned()
        })
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
pub(crate) fn load_user_pubkey(paths: &RuntimePaths) -> Result<[u8; 32], OverlayError> {
    let cert_err = |m: String| OverlayError::Cert(m);
    if paths.root().join(REVOKED_FILE).exists() {
        return Err(OverlayError::Revoked(
            paths.root().join(REVOKED_FILE).display().to_string(),
        ));
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
    let user_pk = match read_capped(&pin_path)? {
        Some(t) => {
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
    Ok(user_pk)
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

