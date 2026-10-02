//! `register`: prove key possession, bind the uid to the key, issue a
//! certificate and take the address (plan 1.2, 1.3).

use clawft_mesh_local::proto::{
    verify_register_sig, Accepted, BindState, ErrorBody, ErrorKind, Message, RegisterAck,
    RegisterReq,
};
use clawft_mesh_local::{node_id_from_pubkey, UserCert};
use serde_json::{json, Value};

use crate::config::BindPolicy;
use crate::local_server::{Conn, Step};
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
        let (bind, cert) = match self.bind_and_issue(&req) {
            Ok(x) => x,
            Err(r) => return self.reject(id, r).await,
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
        reg.set_cert(cert.clone());
        self.reg = Some(reg);
        self.cert = Some(cert.clone());
        let ack = RegisterAck {
            user_id,
            cert,
            accepted: Accepted { addresses: outcome.addresses, topic_prefixes: outcome.topic_prefixes },
            rejected: outcome.rejected,
            bind,
        };
        self.send(id, Message::RegisterAck(ack)).await?;
        Ok(Step::Registered(Box::new((out_rx, killed))))
    }

    /// Bind (per policy) and issue a certificate, under the core lock.
    fn bind_and_issue(&mut self, req: &RegisterReq) -> Result<(BindState, UserCert), Reject> {
        let st = self.st.clone();
        let key = req.user_pubkey;
        let uid = self.uid;
        let meta = BindMeta { by: None, peer_pid: Some(self.pid), exe: Some(self.exe.clone()) };
        let mut guard = st.core.lock().expect("core lock");
        let Core { journal, bindings } = &mut *guard;
        let state = match bindings.check(&self.principal, &key) {
            Check::Existing => BindState::Existing,
            Check::Conflict(r) => {
                st.conflicts.lock().expect("conflicts lock").insert(self.principal.clone(), key);
                return Err(Reject::conflict(r, uid));
            }
            c @ (Check::New | Check::Pending) => match st.cfg.bind_policy {
                BindPolicy::Tofu => {
                    match bindings.bind(journal, &self.principal, &key, BindHow::Tofu, meta.clone()) {
                        Ok(()) => BindState::New,
                        Err(BindError::ApprovalRequired) => {
                            // A revoked uid needs an approver: park the key.
                            let _ = bindings.bind_pending(journal, &self.principal, &key, meta);
                            return Err(Reject::pending(uid));
                        }
                        Err(e) => return Err(Reject::bind_error(&e)),
                    }
                }
                BindPolicy::Approve => {
                    if c == Check::New {
                        bindings
                            .bind_pending(journal, &self.principal, &key, meta)
                            .map_err(|e| Reject::bind_error(&e))?;
                    }
                    return Err(Reject::pending(uid));
                }
            },
        };
        let now = unix_now();
        let ttl = st.cfg.cert_ttl_s;
        let serial = bindings
            .issue_cert(journal, &self.principal, now, now.saturating_add(ttl))
            .map_err(|e| Reject::bind_error(&e))?;
        drop(guard);
        if state == BindState::New {
            st.policy.note_bind(uid);
        }
        Ok((state, UserCert::issue(&st.machine_key, key, serial, now, ttl)))
    }
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
