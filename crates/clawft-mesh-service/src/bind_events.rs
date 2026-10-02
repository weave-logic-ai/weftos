//! Typed bodies of the journal kinds the bindings fold interprets.

use std::time::{SystemTime, UNIX_EPOCH};

use clawft_mesh_local::{node_id_from_pubkey, Principal};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::journal::{JournalError, Record, KIND_ACCEPT_TRUNCATE, KIND_QUARANTINE};

pub const KIND_BIND: &str = "user.bind";
pub const KIND_BIND_PENDING: &str = "user.bind_pending";
pub const KIND_CERT_ISSUE: &str = "user.cert.issue";
pub const KIND_REVOKE: &str = "user.revoke";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindHow {
    Tofu,
    Approved,
    Rebind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReason {
    /// The principal is already bound to a different key.
    PrincipalHasOtherKey,
    /// The key is bound to a different principal.
    KeyBoundToOtherPrincipal,
    /// The key was revoked (or replaced by a rebind) and is never reusable.
    KeyRevoked,
    /// The fold stopped at an invalid record; everything is denied.
    Degraded,
}

/// Result of [`Bindings::check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Existing,
    New,
    Conflict(ConflictReason),
    Pending,
}

#[derive(Debug, thiserror::Error)]
pub enum BindError {
    #[error("bind conflict: {0:?}")]
    Conflict(ConflictReason),
    #[error("principal is already bound")]
    AlreadyBound,
    #[error("principal is not bound")]
    NotBound,
    #[error("unknown user id")]
    UnknownUser,
    #[error("user_id does not match user_pubkey")]
    UserIdMismatch,
    #[error("certificate serial {got} is not the next serial {expected}")]
    SerialOutOfSequence { got: u64, expected: u64 },
    #[error("certificate serial space exhausted")]
    SerialExhausted,
    #[error("principal was revoked; a new key needs an approved bind")]
    ApprovalRequired,
    #[error("an approved bind must record who approved it (`by`)")]
    ApprovalWithoutApprover,
    #[error("serial floor {floor} exceeds the allowed maximum {max}")]
    FloorTooHigh { floor: u64, max: u64 },
    #[error("seq {0} is not a pending journal.quarantine record")]
    NoSuchQuarantine(u64),
    #[error("bindings are degraded ({0}); read-only")]
    Degraded(String),
    #[error("certificate not_after must be after issued_at")]
    BadValidity,
    #[error("serials_revoked_through does not match the issued serials")]
    RevokeMismatch,
    #[error("`how` must be tofu or approved here (use rebind for replacement)")]
    InvalidHow,
    #[error("malformed {kind} record at seq {seq}: {reason}")]
    Malformed { kind: String, seq: u64, reason: String },
    #[error("record seq {seq} violates a binding invariant: {source}")]
    Replay { seq: u64, #[source] source: Box<BindError> },
    #[error(transparent)]
    Journal(#[from] JournalError),
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct BindBody {
    pub principal: Principal,
    #[serde(with = "clawft_mesh_local::hexser::hex32")]
    pub user_pubkey: [u8; 32],
    pub user_id: String,
    pub how: BindHow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Principal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct PendingBody {
    pub principal: Principal,
    #[serde(with = "clawft_mesh_local::hexser::hex32")]
    pub user_pubkey: [u8; 32],
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct CertBody {
    pub user_id: String,
    pub serial: u64,
    pub issued_at: u64,
    pub not_after: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct RevokeBody {
    pub principal: Principal,
    pub user_id: String,
    pub reason: String,
    pub by: Principal,
    pub serials_revoked_through: Option<u64>,
}

/// An admin's acknowledgement of a quarantined tail. `serial_floor` can only
/// raise the serial floor (never lower the clamped quarantine value).
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct AcceptBody {
    /// Seq of the `journal.quarantine` record this accepts.
    pub quarantine_seq: u64,
    pub serial_floor: u64,
    pub quarantine: Vec<String>,
    pub by: Principal,
}

/// Signed facts about a quarantined tail; this is what constrains the state.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct QuarantineBody {
    pub lost_from_seq: u64,
    pub lost_count: u64,
    pub serial_high_water: u64,
    pub raw_serial_high_water: u64,
    pub revoked_user_ids: Vec<String>,
    pub quarantine: Vec<String>,
}

#[derive(Clone)]
pub(crate) enum Event {
    Bind(BindBody),
    Pending(PendingBody),
    Cert(CertBody),
    Revoke(RevokeBody),
    Accept(AcceptBody),
    Quarantine(QuarantineBody),
    /// Kinds this fold does not interpret (peer.*, policy.*, ...).
    Other,
}

impl Event {
    pub fn parse(rec: &Record) -> Result<Event, BindError> {
        let bad = |e: serde_json::Error| BindError::Malformed {
            kind: rec.kind.clone(),
            seq: rec.seq,
            reason: e.to_string(),
        };
        let b = rec.body.clone();
        Ok(match rec.kind.as_str() {
            KIND_BIND => Event::Bind(serde_json::from_value(b).map_err(bad)?),
            KIND_BIND_PENDING => Event::Pending(serde_json::from_value(b).map_err(bad)?),
            KIND_CERT_ISSUE => Event::Cert(serde_json::from_value(b).map_err(bad)?),
            KIND_REVOKE => Event::Revoke(serde_json::from_value(b).map_err(bad)?),
            KIND_ACCEPT_TRUNCATE => Event::Accept(serde_json::from_value(b).map_err(bad)?),
            KIND_QUARANTINE => Event::Quarantine(serde_json::from_value(b).map_err(bad)?),
            _ => Event::Other,
        })
    }

    pub fn kind_body(&self) -> Result<(&'static str, Value), serde_json::Error> {
        Ok(match self {
            Event::Bind(b) => (KIND_BIND, serde_json::to_value(b)?),
            Event::Pending(b) => (KIND_BIND_PENDING, serde_json::to_value(b)?),
            Event::Cert(b) => (KIND_CERT_ISSUE, serde_json::to_value(b)?),
            Event::Revoke(b) => (KIND_REVOKE, serde_json::to_value(b)?),
            Event::Accept(b) => (KIND_ACCEPT_TRUNCATE, serde_json::to_value(b)?),
            Event::Quarantine(b) => (KIND_QUARANTINE, serde_json::to_value(b)?),
            Event::Other => ("", Value::Null),
        })
    }
}

pub(crate) fn id_matches(key: &[u8; 32], user_id: &str) -> Result<(), BindError> {
    if node_id_from_pubkey(key) == user_id {
        Ok(())
    } else {
        Err(BindError::UserIdMismatch)
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
