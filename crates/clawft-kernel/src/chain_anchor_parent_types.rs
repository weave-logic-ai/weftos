//! Public types of the `Parent` anchor backend (see `chain_anchor_parent`).

use clawft_types::project::cert::ProjectAnchorStmt;

/// Kind of the user-chain event the user daemon appends per accepted statement.
pub const KIND_ANCHOR: &str = "project.anchor";
/// Kind of the project-chain acknowledgement.
pub const KIND_ANCHORED: &str = "project.anchored";

/// The user daemon's acknowledgement of an accepted statement.
/// Authenticated by the transport only (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorAck {
    /// Sequence of the user-chain `project.anchor` event.
    pub user_seq: u64,
    /// Hash of that event, hex.
    pub user_event_hash: String,
}

/// Why a submission failed.
#[derive(Debug, Clone)]
pub enum AnchorSubmitError {
    /// The user daemon could not be reached (down, socket gone, timeout).
    Unreachable(String),
    /// The user daemon answered and refused. `last` is its last accepted
    /// statement for this project when the refusal was about `seq` or
    /// `prev_anchor`; `key_history` then lists (diagnostics only, never trusted) the public keys of the
    /// project's certificate history (never a compromise-revoked key).
    Rejected {
        /// Daemon `error_kind`.
        kind: String,
        /// Human message.
        message: String,
        /// Its last accepted statement and acknowledgement.
        last: Option<Box<(ProjectAnchorStmt, AnchorAck)>>,
        /// Certificate-history public keys.
        key_history: Vec<[u8; 32]>,
    },
}

/// Sends statements to the user daemon.
pub trait ParentTransport: Send + Sync {
    /// `project.anchor.submit`. MUST return within a bounded time (the
    /// caller abandons it after [`ParentAnchorConfig::submit_timeout_secs`]).
    /// An identical resubmission of the last accepted statement must return
    /// the original acknowledgement.
    fn submit(&self, stmt: &ProjectAnchorStmt) -> Result<AnchorAck, AnchorSubmitError>;
}

/// Retry backoff and the submission deadline.
#[derive(Debug, Clone, Copy)]
pub struct ParentAnchorConfig {
    /// First retry delay, seconds.
    pub backoff_base_secs: i64,
    /// Longest retry delay, seconds.
    pub backoff_max_secs: i64,
    /// A submission still running after this long counts as unreachable.
    pub submit_timeout_secs: u64,
}

impl Default for ParentAnchorConfig {
    fn default() -> Self {
        Self { backoff_base_secs: 5, backoff_max_secs: 300, submit_timeout_secs: 10 }
    }
}
