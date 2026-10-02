//! `project.anchor.submit`: the user daemon accepts a project's signed
//! chain-head statement (ADR-103 A7, Phase 2 package D).
//!
//! Authenticated by the project key through the statement signature, not by
//! a capability token: a project kernel holds no token, and the claim the
//! daemon records is exactly "this certified key signed this statement".
//! User daemon only. Checks, in order: the certificate in force for the id
//! (from [`current_view`], never called under a journal lock) is unrevoked,
//! names the signing key, matches `cert_serial` and verifies; the signature;
//! then under one lock `seq` is last + 1, `prev_anchor` is the hash of the
//! last accepted statement, `at` is at most five minutes ahead, and
//! `head_seq` does not go backwards. An identical resubmission of the last
//! accepted statement is answered with its original acknowledgement, so a
//! project that lost the answer can replay safely. `at` has no lower bound:
//! a statement replayed after a long outage is old by design.
//!
//! Acceptance appends a `project.anchor` event (source `project.anchor`,
//! reserved) to the user chain and writes `<manifests>/<id>.anchor.json`
//! (the chain is saved only on clean shutdown, so the file is the durable
//! record; [`last_accepted`] takes the further of the two).
//!
//! Honest limit: the user daemon attests "this key claimed head X at time
//! T"; it cannot verify X without subscribing to the project chain.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use clawft_kernel::chain_anchor::{ANCHOR_SOURCE, AnchorAck, AnchorSubmitError};
use clawft_kernel::project_identity::{self as ident, IdentityError};
use clawft_rpc::Response;
use clawft_types::project::cert::{CertError, ProjectAnchorStmt};
use clawft_types::project::canon::hex_decode;
use clawft_types::project::validate_id;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::project_cert_rpc::{CertEnv, IssueError, current_view};
use crate::rpc_ext::{ExtCall, ExtFuture};

/// Kind of the user-chain event.
pub use clawft_kernel::chain_anchor::KIND_ANCHOR;
/// How far ahead of the daemon clock `at` may be, seconds.
pub const MAX_AHEAD_SECS: i64 = 300;

/// Serialises verify-then-append so two submissions cannot both be last + 1.
static ACCEPT_LOCK: Mutex<()> = Mutex::new(());

/// The last accepted statement of a project and where the user chain has it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accepted {
    /// The statement.
    pub statement: ProjectAnchorStmt,
    /// User-chain sequence of its `project.anchor` event.
    pub user_seq: u64,
    /// That event's hash, hex.
    pub user_event_hash: String,
}

impl Accepted {
    fn ack(&self) -> AnchorAck {
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
        /// Last accepted, so the project can resynchronise.
        last: Option<Box<Accepted>>,
    },
    /// `prev_anchor` is not the hash of the last accepted statement.
    #[error("prev_anchor does not match the last accepted statement")]
    Prev {
        /// Last accepted.
        last: Option<Box<Accepted>>,
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
    /// Chain or file write failed after the checks passed.
    #[error("{0}")]
    Store(String),
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
            Self::Store(_) => "anchor_store",
        }
    }

    fn last(&self) -> Option<&Accepted> {
        match self {
            Self::Seq { last, .. } | Self::Prev { last } => last.as_deref(),
            _ => None,
        }
    }

    fn response(&self) -> Response {
        let mut r = Response::error_with_kind(self.kind(), self.to_string());
        r.data = self.last().map(|l| json!({ "last": l }));
        r
    }

    /// The error as a project-side transport would see it, for in-process
    /// transports (tests, a same-process parent).
    pub fn to_submit_error(&self) -> AnchorSubmitError {
        AnchorSubmitError::Rejected {
            kind: self.kind().to_owned(),
            message: self.to_string(),
            last: self.last().map(|l| Box::new((l.statement.clone(), l.ack()))),
        }
    }
}

fn anchor_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.anchor.json"))
}

/// The last statement accepted for `project_id`: the further of the durable
/// file and the user chain's `project.anchor` events.
pub fn last_accepted(env: &CertEnv, project_id: &str) -> Option<Accepted> {
    let from_file = std::fs::read(anchor_file(&env.manifests_dir, project_id))
        .ok()
        .and_then(|b| serde_json::from_slice::<Accepted>(&b).ok())
        .filter(|a| a.statement.project_id == project_id);
    let from_chain = env
        .chain
        .tail(0)
        .into_iter()
        .filter(|e| e.source == ANCHOR_SOURCE && e.kind == KIND_ANCHOR)
        .filter_map(|e| {
            let statement: ProjectAnchorStmt =
                serde_json::from_value(e.payload.as_ref()?.get("statement")?.clone()).ok()?;
            (statement.project_id == project_id).then(|| Accepted {
                statement,
                user_seq: e.sequence,
                user_event_hash: ident::hex(&e.hash),
            })
        })
        .max_by_key(|a| a.statement.seq);
    match (from_file, from_chain) {
        (Some(f), Some(c)) => Some(if c.statement.seq > f.statement.seq { c } else { f }),
        (f, c) => f.or(c),
    }
}

fn cert_error(e: CertError) -> AnchorError {
    match e {
        CertError::BadSignature => AnchorError::BadSignature,
        other => AnchorError::BadStatement(other.to_string()),
    }
}

/// Verify `stmt` and, when it is the next statement, record it. Pure of the
/// daemon: tests and an in-process transport call this directly.
pub fn submit(
    env: &CertEnv,
    stmt: &ProjectAnchorStmt,
    now: DateTime<Utc>,
) -> Result<Accepted, AnchorError> {
    validate_id(&stmt.project_id)
        .map_err(|_| AnchorError::BadStatement("project_id is not a canonical ULID".into()))?;
    let view = current_view(env)?;
    if view.is_revoked(&stmt.project_id, &stmt.project_key_id) {
        return Err(AnchorError::KeyRevoked {
            project_id: stmt.project_id.clone(),
            key_id: stmt.project_key_id.clone(),
        });
    }
    let cert = view
        .current_cert(&stmt.project_id)
        .ok_or_else(|| AnchorError::NotCertified(stmt.project_id.clone()))?;
    if cert.project_key_id != stmt.project_key_id {
        return Err(AnchorError::KeyRevoked {
            project_id: stmt.project_id.clone(),
            key_id: stmt.project_key_id.clone(),
        });
    }
    let user_pk = env.user_key.verifying_key().to_bytes();
    cert.verify(&user_pk, now).map_err(|e| AnchorError::CertInvalid(e.to_string()))?;
    if stmt.cert_serial != cert.serial {
        return Err(AnchorError::CertInvalid(format!(
            "cert_serial {} is not the certificate in force ({})",
            stmt.cert_serial, cert.serial
        )));
    }
    let project_pk: [u8; 32] = hex_decode(&cert.project_pubkey)
        .ok_or_else(|| AnchorError::CertInvalid("certificate public key is malformed".into()))?;
    stmt.verify(&project_pk).map_err(cert_error)?;

    let _guard = ACCEPT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let last = last_accepted(env, &stmt.project_id);
    if let Some(l) = &last
        && l.statement == *stmt
    {
        return Ok(l.clone());
    }
    let want = last.as_ref().map_or(1, |l| l.statement.seq + 1);
    if stmt.seq != want {
        return Err(AnchorError::Seq { got: stmt.seq, want, last: last.map(Box::new) });
    }
    if stmt.prev_anchor != last.as_ref().map(|l| l.statement.hash()) {
        return Err(AnchorError::Prev { last: last.map(Box::new) });
    }
    let at = DateTime::parse_from_rfc3339(&stmt.at)
        .map_err(|_| AnchorError::BadStatement("`at` is not RFC 3339".into()))?
        .with_timezone(&Utc);
    if at > now + Duration::seconds(MAX_AHEAD_SECS) {
        return Err(AnchorError::Future);
    }
    if let Some(l) = &last
        && stmt.head_seq < l.statement.head_seq
    {
        return Err(AnchorError::HeadRegress { got: stmt.head_seq, last: l.statement.head_seq });
    }
    let ev = env.chain.append(
        ANCHOR_SOURCE,
        KIND_ANCHOR,
        Some(json!({
            "project_id": stmt.project_id,
            "statement": stmt,
            "statement_hash": stmt.hash(),
        })),
    );
    let accepted = Accepted {
        statement: stmt.clone(),
        user_seq: ev.sequence,
        user_event_hash: ident::hex(&ev.hash),
    };
    let bytes = serde_json::to_vec_pretty(&accepted).map_err(|e| AnchorError::Store(e.to_string()))?;
    ident::write_private_atomic(&anchor_file(&env.manifests_dir, &stmt.project_id), &bytes, false)
        .map_err(|e: IdentityError| AnchorError::Store(format!("record accepted anchor: {e}")))?;
    Ok(accepted)
}

/// Handler for `project.anchor.submit`. Params are the statement itself.
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        let env = match crate::project_cert_rpc::env_from(&call.ctx).await {
            Ok(e) => e,
            Err(e) => return AnchorError::Unavailable(e.to_string()).response(),
        };
        let stmt: ProjectAnchorStmt = match serde_json::from_value(call.params) {
            Ok(s) => s,
            Err(e) => return AnchorError::BadStatement(format!("not an anchor statement: {e}")).response(),
        };
        match tokio::task::spawn_blocking(move || submit(&env, &stmt, Utc::now())).await {
            Ok(Ok(a)) => Response::success(json!({
                "user_seq": a.user_seq,
                "user_event_hash": a.user_event_hash,
            })),
            Ok(Err(e)) => e.response(),
            Err(e) => Response::error(format!("anchor task failed: {e}")),
        }
    })
}

/// Parse the success result of `project.anchor.submit` (a transport helper).
pub fn parse_ack(result: &Value) -> Option<AnchorAck> {
    Some(AnchorAck {
        user_seq: result.get("user_seq")?.as_u64()?,
        user_event_hash: result.get("user_event_hash")?.as_str()?.to_owned(),
    })
}

#[cfg(test)]
#[path = "anchor_rpc_tests.rs"]
mod tests;
