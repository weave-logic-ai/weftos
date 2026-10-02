//! Verbs of a registered connection: renew, addresses, prefixes, `send`,
//! `verdict.reply` and `journal.head`.
//!
//! `verdict.reply` is not an RPC with authority of its own: the broker accepts
//! it only from the connection it asked (`verdicts.rs`). Nothing here
//! evaluates governance.

use std::str::FromStr;
use std::sync::Arc;

use clawft_mesh_local::framing::FrameError;
use clawft_mesh_local::proto::{ErrorKind, Message, SERVICE_ID_FLAG};
use clawft_mesh_local::{UserCert, WeftAddr};
use clawft_kernel::ipc::KernelMessage;

use crate::local_server::{Conn, ConnCtx, Step};
use crate::registry::Registration;
use crate::router::SendError;
use crate::state::{unix_now, Core};
use crate::verdicts::Reply;

/// A certificate younger than this is returned as is instead of re-issued, so
/// a daemon cannot grow the journal by spamming `renew`: a quarter of the
/// lifetime, at most a minute (clients renew at half the lifetime).
pub fn min_renew_interval(ttl_s: u64) -> u64 {
    (ttl_s / 4).clamp(1, 60)
}

impl Conn {
    pub(crate) async fn handle_registered(&mut self, id: Option<u64>, m: Message) -> Result<Step, FrameError> {
        let Some(reg) = self.reg.clone() else {
            return self.error_step(id, ErrorKind::Forbidden, "register first", "").await;
        };
        match m {
            Message::Renew {} => self.renew(id, &reg).await,
            Message::AddressAdd(b) => {
                let r = self.st.registry.add_project(&reg.user_id, &b.project_id);
                self.ack_or(id, r.map_err(String::from)).await
            }
            Message::AddressRemove(b) => {
                let ok = self.st.registry.remove_project(&reg.user_id, &b.project_id);
                self.ack_or(id, if ok { Ok(()) } else { Err("project is not registered by you".into()) }).await
            }
            Message::Subscribe { prefix } => {
                let r = self.st.registry.add_prefix(&reg.user_id, &prefix);
                self.ack_or(id, r.map_err(String::from)).await
            }
            Message::Unsubscribe { prefix } => {
                let ok = self.st.registry.remove_prefix(&reg.user_id, &prefix);
                self.ack_or(id, if ok { Ok(()) } else { Err("prefix is not registered by you".into()) }).await
            }
            Message::Send { dest, message, .. } => self.send_message(id, &reg, &dest, message).await,
            Message::VerdictReply { allow, ttl_s, reason, rule_hash } => {
                match id {
                    Some(i) if i & SERVICE_ID_FLAG != 0 => {
                        let reply = Reply { allow, ttl_s, reason, rule_hash };
                        if !self.st.verdicts.on_reply(self.conn_id, i, reply) {
                            tracing::warn!(conn = self.conn_id, "dropping an unsolicited verdict.reply");
                        }
                    }
                    _ => tracing::warn!(conn = self.conn_id, "dropping a verdict.reply without a service id"),
                }
                Ok(Step::Continue)
            }
            Message::JournalHead {} => self.journal_head(id).await,
            Message::Register(_) => {
                self.error_step(id, ErrorKind::Forbidden, "already registered on this connection", "").await
            }
            other => self.handle_common(id, other).await,
        }
    }

    async fn ack_or(&mut self, id: Option<u64>, r: Result<(), String>) -> Result<Step, FrameError> {
        match r {
            Ok(()) => self.send(id, Message::Ack {}).await?,
            Err(why) => self.error(id, ErrorKind::Forbidden, why, "").await?,
        }
        Ok(Step::Continue)
    }

    async fn renew(&mut self, id: Option<u64>, reg: &Arc<Registration>) -> Result<Step, FrameError> {
        let (ctx, reg2, current) = (self.ctx(), Arc::clone(reg), self.cert.clone());
        // Journal writes fsync: off the async workers.
        let issued = tokio::task::spawn_blocking(move || renew_blocking(&ctx, &reg2, current.as_ref())).await;
        match issued {
            Ok(None) => {
                self.error_step(id, ErrorKind::Forbidden, "this binding was revoked or replaced", "re-register")
                    .await
            }
            Ok(Some(Ok(cert))) => {
                reg.set_cert(cert.clone());
                self.cert = Some(cert.clone());
                self.send(id, Message::Cert { cert }).await?;
                Ok(Step::Continue)
            }
            Ok(Some(Err(e))) => {
                let r = crate::register::Reject::bind_error(&e);
                self.error_step(id, r.kind, r.message, r.remedy).await
            }
            Err(_) => self.error_step(id, ErrorKind::Forbidden, "internal error while renewing", "").await,
        }
    }

    async fn send_message(
        &mut self,
        id: Option<u64>,
        reg: &Arc<Registration>,
        dest: &str,
        message: serde_json::Value,
    ) -> Result<Step, FrameError> {
        if !reg.allow_send() {
            self.error(id, ErrorKind::RateLimited, "too many sends", "slow down").await?;
            return Ok(Step::Continue);
        }
        let addr = match WeftAddr::from_str(dest) {
            Ok(a) => a,
            Err(e) => return self.error_step(id, ErrorKind::BadRequest, format!("bad address: {e}"), "").await,
        };
        let msg: KernelMessage = match serde_json::from_value(message) {
            Ok(m) => m,
            Err(e) => {
                return self
                    .error_step(id, ErrorKind::BadRequest, format!("message is not a KernelMessage: {e}"), "")
                    .await;
            }
        };
        match self.st.router.route_outbound(reg, &addr, msg).await {
            Ok(()) => self.send(id, Message::Ack {}).await?,
            Err(SendError::Unreachable(w)) => self.error(id, ErrorKind::PeerUnreachable, w, "").await?,
            Err(SendError::UnknownScope(w)) => {
                self.error(id, ErrorKind::UnknownScope, format!("no registration for {w}"), "").await?
            }
            Err(SendError::Forbidden(w)) => self.error(id, ErrorKind::Forbidden, w, "ask the recipient to set accept_from").await?,
            Err(SendError::Failed(w)) => self.error(id, ErrorKind::BadRequest, w, "").await?,
        }
        Ok(Step::Continue)
    }

    async fn journal_head(&mut self, id: Option<u64>) -> Result<Step, FrameError> {
        let head = {
            let c = self.st.core.lock().expect("core lock");
            c.journal.head().and_then(|h| {
                c.journal.iter().last().map(|r| (h.seq, h.hash, r.ts, r.sig.clone()))
            })
        };
        match head {
            Some((seq, hash, ts, sig)) => {
                self.send(id, Message::JournalHeadReply { seq, hash, ts, sig }).await?;
                Ok(Step::Continue)
            }
            None => self.error_step(id, ErrorKind::BadRequest, "the journal is empty", "").await,
        }
    }
}

/// `None`: the binding was revoked or replaced. Otherwise the current or a
/// freshly issued certificate (a renew inside the throttle window returns the
/// current one without a journal record).
fn renew_blocking(
    ctx: &ConnCtx,
    reg: &Registration,
    current: Option<&UserCert>,
) -> Option<Result<UserCert, crate::BindError>> {
    let st = &ctx.st;
    let now = unix_now();
    let ttl = st.cfg.cert_ttl_s;
    let mut guard = st.core.lock().expect("core lock");
    let Core { journal, bindings } = &mut *guard;
    let serial_revoked = current.is_some_and(|c| bindings.is_serial_revoked(&reg.user_id, c.serial));
    let forced = st.force_revoked.contains(&ctx.principal);
    if bindings.key_of(&ctx.principal) != Some(reg.user_pubkey) || serial_revoked || forced {
        return None;
    }
    if let Some(c) = current
        && now.saturating_sub(c.issued_at) < min_renew_interval(ttl)
    {
        return Some(Ok(c.clone()));
    }
    let cert = bindings
        .issue_cert(journal, &ctx.principal, now, now.saturating_add(ttl))
        .map(|serial| UserCert::issue(&st.machine_key, reg.user_pubkey, serial, now, ttl));
    if let Ok(c) = &cert {
        st.last_certs.lock().expect("certs lock").insert(reg.user_id.clone(), c.clone());
    }
    Some(cert)
}
