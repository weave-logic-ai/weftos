//! Daemon-issued, single-use proof-of-possession nonces (see the parent
//! module docs).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use clawft_types::project::validate_id;

use super::IssueError;
use clawft_kernel::project_identity as ident;

/// How long an issued challenge nonce stays valid.
pub const CHALLENGE_TTL: Duration = Duration::from_secs(60);
const MAX_CHALLENGES: usize = 1024;

/// A challenge nonce this daemon issued, not yet used. Only
/// [`claim_nonce`] makes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonNonce {
    nonce: String,
    project_id: String,
}

impl DaemonNonce {
    /// The nonce, 32 lowercase hex.
    pub fn as_str(&self) -> &str {
        &self.nonce
    }
    /// The project it was issued for.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
}

static CHALLENGES: OnceLock<Mutex<HashMap<String, (String, Instant)>>> = OnceLock::new();

/// Issue a single-use challenge nonce for `project_id` (valid for
/// [`CHALLENGE_TTL`]; at most 1024 outstanding).
pub fn issue_challenge(project_id: &str) -> Result<String, IssueError> {
    validate_id(project_id).map_err(|_| IssueError::Invalid("project id is not a canonical ULID".into()))?;
    let mut raw = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut raw);
    let nonce = ident::hex(&raw);
    let mut map = CHALLENGES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    map.retain(|_, (_, t)| *t > now);
    if map.len() >= MAX_CHALLENGES {
        return Err(IssueError::Unavailable("too many outstanding challenges".into()));
    }
    map.insert(nonce.clone(), (project_id.to_owned(), now + CHALLENGE_TTL));
    Ok(nonce)
}

/// Consume a challenge: it must be one this daemon issued for `project_id`,
/// unexpired and unused. Any failure burns nothing but reports `pop_failed`.
pub fn claim_nonce(nonce: &str, project_id: &str) -> Result<DaemonNonce, IssueError> {
    let mut map = CHALLENGES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    match map.remove(nonce) {
        Some((pid, exp)) if pid == project_id && exp > Instant::now() => Ok(DaemonNonce {
            nonce: nonce.to_owned(),
            project_id: pid,
        }),
        _ => Err(IssueError::Pop("unknown, expired or already used challenge nonce".into())),
    }
}

