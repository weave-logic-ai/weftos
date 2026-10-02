//! Result and error types of `project.anchor.submit` (see `anchor_rpc`).

use clawft_kernel::chain_anchor::{AnchorAck, AnchorSubmitError};
use clawft_rpc::Response;
use clawft_types::project::canon::hex_decode;
use clawft_types::project::cert::ProjectAnchorStmt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::MAX_AHEAD_SECS;
use crate::project_cert_rpc::IssueError;

/// The last accepted statement of a project and where the user chain has it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accepted {
    /// The statement.
    pub statement: ProjectAnchorStmt,
    /// User-chain sequence of its `project.anchor` event.
    pub user_seq: u64,
    /// That event's hash, hex.
    pub user_event_hash: String,
    /// User-key signature over `{statement_hash, user_seq, user_event_hash}`
    /// (see `anchor_record`): a record without a valid one is ignored.
    #[serde(default)]
    pub rec_sig: String,
}

impl Accepted {
    pub(super) fn ack(&self) -> AnchorAck {
        AnchorAck { user_seq: self.user_seq, user_event_hash: self.user_event_hash.clone() }
    }
}

/// Why a statement was refused; `kind()` is the RPC `error_kind`.
#[derive(Debug, thiserror::Error)]
pub enum AnchorError {
    /// Not the user daemon, or it has no chain.
    #[error("{0}")]
    Unavailable(String),
    /// Identity view failed (corrupt journal, store error).
    #[error(transparent)]
    Identity(#[from] IssueError),
    /// No certificate in force for the project.
    #[error("project {0} has no certified key")]
    NotCertified(String),
    /// The statement's key was revoked or replaced.
    #[error("key {key_id} of project {project_id} is revoked or replaced")]
    KeyRevoked {
        /// Project id.
        project_id: String,
        /// Signing key id.
        key_id: String,
    },
    /// The certificate does not verify, or `cert_serial` does not name it.
    #[error("{0}")]
    CertInvalid(String),
    /// Malformed statement.
    #[error("{0}")]
    BadStatement(String),
    /// The signature does not verify under the certified key.
    #[error("statement signature does not verify")]
    BadSignature,
    /// `seq` is not last + 1.
    #[error("seq {got} is not the next accepted seq {want}")]
    Seq {
        /// Submitted.
        got: u64,
        /// Expected.
        want: u64,
        /// Last accepted and the certificate-history keys, so the project can resynchronise.
        last: Option<Box<Resync>>,
    },
    /// `prev_anchor` is not the hash of the last accepted statement.
    #[error("prev_anchor does not match the last accepted statement")]
    Prev {
        /// As in [`AnchorError::Seq`].
        last: Option<Box<Resync>>,
    },
    /// `at` is more than [`MAX_AHEAD_SECS`] ahead.
    #[error("`at` is more than {MAX_AHEAD_SECS} s in the future")]
    Future,
    /// `head_seq` went backwards.
    #[error("head_seq {got} is behind the last accepted head_seq {last}")]
    HeadRegress {
        /// Submitted.
        got: u64,
        /// Last accepted.
        last: u64,
    },
    /// Too many authenticated refusals in a row.
    #[error("too many refused statements; retry in {0} s")]
    Backoff(i64),
    /// Chain or file write failed after the checks passed.
    #[error("{0}")]
    Store(String),
}

/// What a project needs to resynchronise after a `seq` / `prev` refusal.
#[derive(Debug, Clone, Serialize)]
pub struct Resync {
    /// The daemon's last accepted statement.
    pub last: Accepted,
    /// Public keys (hex) of the project's certificate history, compromised keys excluded.
    pub key_history: Vec<String>,
}

impl AnchorError {
    /// Stable snake_case discriminator for clients.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "anchor_unavailable",
            Self::Identity(e) => e.kind(),
            Self::NotCertified(_) => "anchor_not_certified",
            Self::KeyRevoked { .. } => "anchor_key_revoked",
            Self::CertInvalid(_) => "anchor_cert_invalid",
            Self::BadStatement(_) => "anchor_bad_statement",
            Self::BadSignature => "anchor_bad_signature",
            Self::Seq { .. } => "anchor_seq",
            Self::Prev { .. } => "anchor_prev",
            Self::Future => "anchor_future",
            Self::HeadRegress { .. } => "anchor_head_regress",
            Self::Backoff(_) => "anchor_backoff",
            Self::Store(_) => "anchor_store",
        }
    }

    pub(super) fn resync(&self) -> Option<&Resync> {
        match self {
            Self::Seq { last, .. } | Self::Prev { last } => last.as_deref(),
            _ => None,
        }
    }

    pub(super) fn response(&self) -> Response {
        let mut r = Response::error_with_kind(self.kind(), self.to_string());
        r.data = self.resync().map(|x| json!({ "last": x.last, "key_history": x.key_history }));
        r
    }

    /// The error as a project-side transport would see it, for in-process
    /// transports (tests, a same-process parent).
    pub fn to_submit_error(&self) -> AnchorSubmitError {
        AnchorSubmitError::Rejected {
            kind: self.kind().to_owned(),
            message: self.to_string(),
            last: self.resync().map(|x| Box::new((x.last.statement.clone(), x.last.ack()))),
            key_history: self
                .resync()
                .map(|x| x.key_history.iter().filter_map(|h| hex_decode::<32>(h)).collect())
                .unwrap_or_default(),
        }
    }
}
