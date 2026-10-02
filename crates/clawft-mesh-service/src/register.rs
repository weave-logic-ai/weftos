//! `register`: prove key possession, bind the uid to the key, issue a
//! certificate and take the address (plan 1.2, 1.3).

use clawft_mesh_local::proto::{
    verify_register_sig, Accepted, BindState, ErrorBody, ErrorKind, Message, RegisterAck,
    RegisterReq,
};
use clawft_mesh_local::{node_id_from_pubkey, UserCert};
use serde_json::{json, Value};

use crate::config::BindPolicy;
use crate::local_server::{Conn, ConnCtx, Step};
use crate::registry::{RegisterError, Registration};
use crate::state::{unix_now, Core};
use crate::{BindError, BindHow, BindMeta, Check, ConflictReason, JournalError};
use clawft_mesh_local::framing::FrameError;

/// A refusal to send (and close on, when the kind is fatal).
pub(crate) struct Reject {
    pub kind: ErrorKind,
    pub message: String,
    pub remedy: String,
    pub data: Option<Value>,
}

impl Reject {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self { kind, message: message.into(), remedy: remedy.into(), data: None }
    }

    fn pending(uid: u32) -> Self {
        Self::new(
            ErrorKind::BindPending,
            "this user key is waiting for an administrator to approve it",
            format!("an admin runs `weaver mesh bind approve {uid}`"),
        )
    }

    fn conflict(r: ConflictReason, uid: u32) -> Self {
        match r {
            ConflictReason::PrincipalHasOtherKey => Self::new(
                ErrorKind::BindConflict,
                format!("uid {uid} is bound to a different user key"),
                format!("if the key was replaced on purpose, an admin runs `weaver mesh bind rebind {uid}`"),
            ),
            ConflictReason::KeyBoundToOtherPrincipal => Self::new(
                ErrorKind::BindConflict,
                "this user key is already bound to a different account",
                "generate a separate user key for this account",
            ),
            ConflictReason::KeyRevoked => Self::new(
                ErrorKind::BindConflict,
                "this user key was revoked",
                "generate a new user key",
            ),
            ConflictReason::Degraded => Self::new(
                ErrorKind::Forbidden,
                "the service journal is degraded and refuses new bindings",
                "an admin runs `weaver mesh journal verify`",
            ),
        }
    }

    pub(crate) fn journal_error(e: &JournalError) -> Self {
        match e {
            JournalError::ReadOnly => Self::new(
                ErrorKind::Forbidden,
                "the service journal is read-only after a quarantined tail",
                "an admin runs `weaver mesh journal verify --accept-truncate`",
            ),
            other => Self::new(ErrorKind::Forbidden, format!("the journal refused the change: {other}"), ""),
        }
    }

    pub(crate) fn bind_error(e: &BindError) -> Self {
        match e {
            BindError::Journal(JournalError::ReadOnly) => Self::new(
                ErrorKind::Forbidden,
                "the service journal is read-only after a quarantined tail",
                "an admin runs `weaver mesh journal verify --accept-truncate`",
            ),
            BindError::Degraded(_) => Self::new(
                ErrorKind::Forbidden,
                "the service journal is degraded and refuses new bindings",
                "an admin runs `weaver mesh journal verify`",
            ),
            other => Self::new(ErrorKind::Forbidden, other.to_string(), ""),
        }
    }
}

impl Conn {
    pub(crate) async fn reject(&mut self, id: Option<u64>, r: Reject) -> Result<Step, FrameError> {
        let fatal = r.kind.is_fatal() || r.kind == ErrorKind::BindPending;
        let mut body = ErrorBody::new(r.kind, r.message, r.remedy);
        body.data = r.data;
        self.send(id, Message::Error(body)).await?;
        Ok(if fatal { Step::Close } else { Step::Continue })
    }

    pub(crate) async fn register(&mut self, id: Option<u64>, req: RegisterReq) -> Result<Step, FrameError> {
        let st = self.st.clone();
        if !st.limiter.allow_register(&self.principal) {
            return self.reject(id, Reject::new(
                ErrorKind::RateLimited, "too many registrations", "wait a minute before retrying")).await;
        }
        // The challenge is single-use: a failed attempt spends it too.
        if std::mem::replace(&mut self.challenge_used, true) {
            return self.reject(id, Reject::new(
                ErrorKind::Forbidden, "register may be sent once per connection", "reconnect")).await;
        }
        if !verify_register_sig(&req, &self.challenge, &self.principal, &st.node_id) {
            return self.reject(id, Reject::new(
                ErrorKind::BadSig, "register signature does not verify", "re-register")).await;
        }
        let user_id = node_id_from_pubkey(&req.user_pubkey);
        if req.addresses.user_id != user_id {
            return self.reject(id, Reject::new(
                ErrorKind::Forbidden, "addresses.user_id does not match user_pubkey", "")).await;
        }
        // Early check so a refused second daemon does not burn a serial.
        if let Some(holder) = st.registry.get(&user_id) {
            return self.reject(id, in_use(holder.pid)).await;
        }
        // Journal writes fsync: keep them off the async workers.
        let (ctx, rq) = (self.ctx(), req.clone());
        let issued = tokio::task::spawn_blocking(move || bind_and_issue(&ctx, &rq)).await;
        let (bind, cert) = match issued {
            Ok(Ok(x)) => x,
            Ok(Err(r)) => return self.reject(id, r).await,
            Err(_) => {
                return self.reject(id, Reject::new(ErrorKind::Forbidden, "internal error while binding", "")).await;
            }
        };
        let now = unix_now();
        let (reg, out_rx, killed) = Registration::new(
            self.conn_id,
            self.principal.clone(),
            user_id.clone(),
            req.user_pubkey,
            self.pid,
            self.exe.clone(),
            req.capabilities.clone(),
            now,
        );
        let projects: Vec<String> = req.addresses.projects.iter().map(|p| p.project_id.clone()).collect();
        let outcome = match st.registry.register(&reg, &projects, &req.topic_prefixes) {
            Ok(o) => o,
            Err(RegisterError::InUse { holder_pid }) => return self.reject(id, in_use(holder_pid)).await,
        };
        // accept_from governs local cross-tenant sends only; deliveries from
        // admitted remote peers are gated by admission, not by this list.
        let (accept, accept_rejected) = sanitize_accept_from(&req.accept_from);
        reg.set_accept_from(accept);
        reg.set_cert(cert.clone());
        self.reg = Some(reg);
        self.cert = Some(cert.clone());
        let ack = RegisterAck {
            user_id,
            cert,
            accepted: Accepted { addresses: outcome.addresses, topic_prefixes: outcome.topic_prefixes },
            rejected: outcome.rejected.into_iter().chain(accept_rejected).collect(),
            bind,
        };
        self.send(id, Message::RegisterAck(ack)).await?;
        Ok(Step::Registered(Box::new((out_rx, killed))))
    }
}

/// Bind (per policy) and issue (or reuse) a certificate, under the core lock.
/// Synchronous: runs on a blocking thread.
fn bind_and_issue(ctx: &ConnCtx, req: &RegisterReq) -> Result<(BindState, UserCert), Reject> {
    let st = &ctx.st;
    let key = req.user_pubkey;
    let uid = ctx.uid;
    let user_id = node_id_from_pubkey(&key);
    let meta = BindMeta { by: None, peer_pid: Some(ctx.pid).filter(|p| *p > 0), exe: Some(ctx.exe.clone()) };
    if st.force_revoked.contains(&ctx.principal) {
        return Err(Reject::new(
            ErrorKind::Forbidden,
            "this binding was revoked (the revocation is enforced in memory until the journal is repaired)",
            "an admin runs `weaver mesh journal verify`",
        ));
    }
    let mut guard = st.core.lock().expect("core lock");
    let Core { journal, bindings } = &mut *guard;
    let state = match bindings.check(&ctx.principal, &key) {
        Check::Existing => BindState::Existing,
        Check::Conflict(r) => {
            st.conflicts.lock().expect("conflicts lock").insert(ctx.principal.clone(), key);
            return Err(Reject::conflict(r, uid));
        }
        c @ (Check::New | Check::Pending) => match st.cfg.bind_policy {
            BindPolicy::Tofu => match bindings.bind(journal, &ctx.principal, &key, BindHow::Tofu, meta.clone()) {
                Ok(()) => BindState::New,
                Err(BindError::ApprovalRequired) => {
                    // A revoked uid needs an approver: park the key.
                    let _ = bindings.bind_pending(journal, &ctx.principal, &key, meta);
                    return Err(Reject::pending(uid));
                }
                Err(e) => return Err(Reject::bind_error(&e)),
            },
            BindPolicy::Approve => {
                if c == Check::New {
                    bindings.bind_pending(journal, &ctx.principal, &key, meta).map_err(|e| Reject::bind_error(&e))?;
                }
                return Err(Reject::pending(uid));
            }
        },
    };
    let now = unix_now();
    let ttl = st.cfg.cert_ttl_s;
    // A reconnect reuses a certificate that still has over half its life and
    // was not revoked: no new journal record per reconnect.
    if let Some(c) = st.last_certs.lock().expect("certs lock").get(&user_id)
        && c.user_pubkey == key
        && c.not_after > now.saturating_add(ttl / 2)
        && !bindings.is_serial_revoked(&user_id, c.serial)
    {
        return Ok((state, c.clone()));
    }
    let serial = bindings
        .issue_cert(journal, &ctx.principal, now, now.saturating_add(ttl))
        .map_err(|e| Reject::bind_error(&e))?;
    drop(guard);
    let cert = UserCert::issue(&st.machine_key, key, serial, now, ttl);
    st.last_certs.lock().expect("certs lock").insert(user_id, cert.clone());
    Ok((state, cert))
}

/// Most `accept_from` entries a registration may carry.
pub const MAX_ACCEPT_FROM: usize = 32;

/// Keep valid entries (`*` or a 32-hex user id), at most [`MAX_ACCEPT_FROM`];
/// report the rest.
pub(crate) fn sanitize_accept_from(list: &[String]) -> (Vec<String>, Vec<clawft_mesh_local::proto::Rejected>) {
    let mut ok = Vec::new();
    let mut rejected = Vec::new();
    for e in list {
        let valid = e == "*" || clawft_mesh_local::hexser::decode::<16>(e).is_some();
        if !valid {
            rejected.push(clawft_mesh_local::proto::Rejected {
                what: format!("accept_from {}", e.chars().take(40).collect::<String>()),
                reason: "not `*` or a 32 hex character user id".into(),
            });
        } else if ok.len() >= MAX_ACCEPT_FROM {
            rejected.push(clawft_mesh_local::proto::Rejected {
                what: "accept_from".into(),
                reason: format!("more than {MAX_ACCEPT_FROM} entries; the rest are ignored"),
            });
            break;
        } else if !ok.contains(e) {
            ok.push(e.clone());
        }
    }
    (ok, rejected)
}

fn in_use(holder_pid: u32) -> Reject {
    let mut r = Reject::new(
        ErrorKind::AddressInUse,
        format!("another daemon for this user is already registered (pid {holder_pid})"),
        "stop the other daemon, or keep using it",
    );
    r.data = Some(json!({"holder_pid": holder_pid}));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_from_keeps_valid_entries_dedupes_and_caps_at_32() {
        let ids: Vec<String> = (0..40u8).map(|n| node_id_from_pubkey(&[n; 32])).collect();
        let (ok, rej) = sanitize_accept_from(&ids);
        assert_eq!(ok.len(), MAX_ACCEPT_FROM);
        assert_eq!(rej.len(), 1, "one note that the rest were ignored");
        let (ok, rej) = sanitize_accept_from(&["*".into(), "*".into(), "ZZ".into(), ids[0].to_uppercase()]);
        assert_eq!(ok, vec!["*".to_string()]);
        assert_eq!(rej.len(), 2, "garbage and uppercase hex are refused");
    }
}
