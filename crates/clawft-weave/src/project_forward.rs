//! The user-signed forward header (ADR-103 A6, Phase 2 package I).
//!
//! When the user daemon proxies a call to a project's own kernel it stamps
//! `Request.forward = {project_id, issued_at_ms, sig}`, signed by the user key
//! (the chain key of Phase 1 D-1) over
//! `"weftos-project-forward-v1\n<project_id>\n<issued_at_ms>"`. The child
//! verifies it against the `user_pubkey` of its own project certificate:
//!
//! * the signature must be the user key's (domain-tagged distinctly from the
//!   cert, anchor, PoP and chain-event bytes, so one key never signs
//!   confusable bytes);
//! * `project_id` must be the child's bound project;
//! * `issued_at_ms` must lie within [`FORWARD_WINDOW_MS`] of the child's
//!   clock, in either direction (5 s);
//! * single use: a signature is accepted once within the window, so a
//!   captured header cannot be replayed.
//!
//! [`ForwardVerifier`] is process state: the child installs it once at boot
//! with the cert's user key ([`install_trust`]); without it every forward
//! header is refused (`forward_unavailable`), never ignored.
//!
//! Honest limit: the header proves "the holder of the user key forwarded this
//! one request for this project". It does not prove who asked the user
//! daemon; that is the user daemon's own authorisation. A same-uid process
//! that can read the user key can sign one too; this is an isolation guard
//! between one user's projects, not a boundary against a hostile local
//! process (Phase 3 peer credentials, Phase 4 sandboxes).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use clawft_rpc::ForwardHeader;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::verified_project::VerifiedProject;

/// Domain tag of the signed bytes.
pub const FORWARD_DOMAIN: &str = "weftos-project-forward-v1\n";

/// Accepted clock skew / age of a forward header, in milliseconds.
pub const FORWARD_WINDOW_MS: u64 = 5_000;

/// Why a forward header was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardError {
    /// This process holds no user key to verify against (not a child, or not
    /// booted with a certificate).
    Unavailable,
    /// Malformed hex or wrong length.
    Malformed,
    /// The signature is not the user key's over the stated fields.
    BadSignature,
    /// The header names a project other than the one this kernel serves.
    WrongProject,
    /// `issued_at_ms` is more than [`FORWARD_WINDOW_MS`] from now.
    OutsideWindow,
    /// The same signature was already accepted inside the window.
    Replayed,
}

impl ForwardError {
    /// `error_kind` for the refusal response.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Unavailable => "forward_unavailable",
            Self::Malformed | Self::BadSignature => "forward_bad_signature",
            Self::WrongProject => "project_scope_mismatch",
            Self::OutsideWindow => "forward_expired",
            Self::Replayed => "forward_replayed",
        }
    }
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = match self {
            Self::Unavailable => "this kernel has no user key to verify a forward header against",
            Self::Malformed => "forward header is malformed",
            Self::BadSignature => "forward header signature is not the user key's",
            Self::WrongProject => "forward header names a project other than this kernel's",
            Self::OutsideWindow => "forward header is outside its 5 s window",
            Self::Replayed => "forward header was already used",
        };
        f.write_str(m)
    }
}

impl std::error::Error for ForwardError {}

fn signed_bytes(project_id: &str, issued_at_ms: u64) -> Vec<u8> {
    format!("{FORWARD_DOMAIN}{project_id}\n{issued_at_ms}").into_bytes()
}

/// User-daemon side: sign a forward header for one proxied call.
pub fn sign_forward(user_key: &SigningKey, project_id: &str, issued_at_ms: u64) -> ForwardHeader {
    let sig = user_key.sign(&signed_bytes(project_id, issued_at_ms));
    ForwardHeader {
        project_id: project_id.to_owned(),
        issued_at_ms,
        sig: hex::encode(sig.to_bytes()),
    }
}

/// User-daemon side: stamp `req` (a call proxied to `project_id`'s kernel)
/// with a fresh forward header. Also pins `req.project` to the same id, so
/// the child's claim check and the header agree.
pub fn stamp_forward(req: &mut clawft_rpc::Request, user_key: &SigningKey, project_id: &str, now_ms: u64) {
    req.project = Some(project_id.to_owned());
    req.forward = Some(sign_forward(user_key, project_id, now_ms));
}

/// Child side: verifies forward headers for one bound project.
pub struct ForwardVerifier {
    user_key: VerifyingKey,
    /// Signature -> issued_at_ms of headers accepted inside the window.
    seen: Mutex<HashMap<String, u64>>,
}

impl ForwardVerifier {
    /// A verifier trusting `user_pubkey` (from the project certificate).
    pub fn new(user_pubkey: &[u8; 32]) -> Result<Self, ForwardError> {
        Ok(Self {
            user_key: VerifyingKey::from_bytes(user_pubkey).map_err(|_| ForwardError::Malformed)?,
            seen: Mutex::new(HashMap::new()),
        })
    }

    /// Verify `header` for the kernel bound to `bound_project` at `now_ms`.
    ///
    /// Order matters: the signature is checked before anything is recorded,
    /// so a forged header cannot fill the replay table; the replay table is
    /// pruned to the window on every call, so it stays bounded by the rate
    /// of validly signed headers.
    pub fn verify(
        &self,
        header: &ForwardHeader,
        bound_project: &str,
        now_ms: u64,
    ) -> Result<VerifiedProject, ForwardError> {
        let sig_bytes: [u8; 64] = hex::decode(&header.sig)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or(ForwardError::Malformed)?;
        self.user_key
            .verify(
                &signed_bytes(&header.project_id, header.issued_at_ms),
                &Signature::from_bytes(&sig_bytes),
            )
            .map_err(|_| ForwardError::BadSignature)?;
        if header.project_id != bound_project {
            return Err(ForwardError::WrongProject);
        }
        if now_ms.abs_diff(header.issued_at_ms) > FORWARD_WINDOW_MS {
            return Err(ForwardError::OutsideWindow);
        }
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        // Entries older than the window can no longer pass the check above.
        seen.retain(|_, at| now_ms.abs_diff(*at) <= FORWARD_WINDOW_MS);
        if seen.insert(header.sig.clone(), header.issued_at_ms).is_some() {
            return Err(ForwardError::Replayed);
        }
        Ok(VerifiedProject::from_verified_forward(header.project_id.clone()))
    }
}

static TRUST: OnceLock<ForwardVerifier> = OnceLock::new();

/// Install the process's forward verifier (child boot, once). Returns
/// `false` if one is already installed.
pub fn install_trust(user_pubkey: &[u8; 32]) -> Result<bool, ForwardError> {
    Ok(TRUST.set(ForwardVerifier::new(user_pubkey)?).is_ok())
}

/// Verify against the installed trust at the wall clock.
pub fn verify_installed(header: &ForwardHeader, bound_project: &str) -> Result<VerifiedProject, ForwardError> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    TRUST
        .get()
        .ok_or(ForwardError::Unavailable)?
        .verify(header, bound_project, now_ms)
}

#[cfg(test)]
#[path = "project_forward_tests.rs"]
mod tests;
