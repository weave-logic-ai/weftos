//! Admin verbs (`role: "admin"` connections from root or `admin_uids`).
//!
//! Admin connections hold no keys: they are authorised by the peer
//! credential alone, checked at connect time against a configuration that
//! cannot change while the service runs. Every state change is a journal
//! record naming `by`, and it is journalled *first*: state changes only after
//! the append succeeded, so an append failure (read-only journal, I/O) is the
//! verb's error and nothing was applied. The exception is tightening
//! (`peer.revoke`, `bind.revoke`): a broken journal must not stop an operator
//! from cutting someone off, so those apply first and journal best-effort,
//! returning `{"warning": ...}` when the record could not be written. Verbs
//! run on a blocking thread (fsync, `verify_dir`).

use clawft_mesh_local::framing::FrameError;
use clawft_mesh_local::proto::{ErrorKind, Message};
use clawft_mesh_local::{hexser, node_id_from_pubkey, Principal};
use serde_json::{json, Value};

use crate::bindings_view::bindings_json;
use crate::local_server::{Conn, ConnCtx, Step};
use crate::register::Reject;
use crate::state::{admission_str, parse_admission, Core};
use crate::{verify_dir, AdminAck, BindError, BindHow, BindMeta, JournalError};

pub(crate) fn is_admin_verb(m: &Message) -> bool {
    matches!(
        m,
        Message::BindingsList {}
            | Message::BindApprove { .. }
            | Message::BindRevoke { .. }
            | Message::BindRebind { .. }
            | Message::PeerRevoke { .. }
            | Message::PeerUnrevoke { .. }
            | Message::PolicySet { .. }
            | Message::JournalVerify {}
            | Message::JournalAcceptTruncate { .. }
    )
}

fn bad(why: impl Into<String>) -> Reject {
    Reject::new(ErrorKind::BadRequest, why, "")
}

fn admin_err(e: &BindError) -> Reject {
    match e {
        BindError::NotBound => bad("that uid has no binding"),
        BindError::NothingToAccept | BindError::NoSuchQuarantine(_) | BindError::FloorTooHigh { .. } => {
            bad(e.to_string())
        }
        other => Reject::bind_error(other),
    }
}

fn valid_node_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'))
}

fn persisted_note(saved: &std::io::Result<()>) -> String {
    match saved {
        Ok(()) => "recorded in force-revoked.json, which survives a restart".into(),
        Err(e) => format!("force-revoked.json could not be written ({e}): in memory only until restart"),
    }
}

fn journal_err(e: &JournalError) -> Reject {
    Reject::journal_error(e)
}

fn read_only_err() -> Reject {
    journal_err(&JournalError::ReadOnly)
}

impl Conn {
    pub(crate) async fn admin_verb(&mut self, id: Option<u64>, m: Message) -> Result<Step, FrameError> {
        let ctx = self.ctx();
        let r = tokio::task::spawn_blocking(move || admin_dispatch(&ctx, m))
            .await
            .unwrap_or_else(|_| Err(Reject::new(ErrorKind::BadRequest, "internal error", "")));
        match r {
            Ok(Value::Null) => self.send(id, Message::Ack {}).await?,
            Ok(data) => self.send(id, Message::Reply { data }).await?,
            // Refused, not fatal: the admin may try again.
            Err(r) => self.error(id, r.kind, r.message, r.remedy).await?,
        }
        Ok(Step::Continue)
    }
}

fn meta(ctx: &ConnCtx) -> BindMeta {
    BindMeta {
        by: Some(ctx.principal.clone()),
        peer_pid: Some(ctx.pid).filter(|p| *p > 0),
        exe: Some(ctx.exe.clone()),
    }
}

fn by_json(ctx: &ConnCtx) -> Value {
    serde_json::to_value(&ctx.principal).unwrap_or(Value::Null)
}

/// An owner revocation or rebind ends the authority cached verdicts came from.
fn owner_changed(ctx: &ConnCtx, uid: u32) {
    if ctx.st.policy.owner_uid() == Some(uid) {
        ctx.st.verdicts.clear();
    }
}

/// Synchronous on purpose: runs on a blocking thread and nothing awaits while
/// the core lock is held.
fn admin_dispatch(ctx: &ConnCtx, m: Message) -> Result<Value, Reject> {
    debug_assert!(ctx.admin, "admin verbs are only reachable behind the admin gate");
    let st = ctx.st.clone();
    match m {
        Message::BindingsList {} => {
            let c = st.core.lock().expect("core lock");
            Ok(bindings_json(&c, &st.registry, &st.force_revoked.list()))
        }
        Message::BindApprove { uid, user_id } => {
            let p = Principal::Uid(uid);
            let mut g = st.core.lock().expect("core lock");
            let Core { journal, bindings } = &mut *g;
            let key = bindings
                .pending_key(&p)
                .ok_or_else(|| bad(format!("nothing is pending approval for uid {uid}")))?;
            let pending_id = node_id_from_pubkey(&key);
            if let Some(expected) = user_id
                && expected != pending_id
            {
                return Err(bad(format!(
                    "the key pending for uid {uid} is {pending_id}, not {expected}; it changed since you looked"
                )));
            }
            bindings.bind(journal, &p, &key, BindHow::Approved, meta(ctx)).map_err(|e| admin_err(&e))?;
            st.force_revoked.remove(&p);
            Ok(Value::Null)
        }
        Message::BindRevoke { uid, reason } => {
            let p = Principal::Uid(uid);
            let mut warning = None;
            {
                let mut g = st.core.lock().expect("core lock");
                let Core { journal, bindings } = &mut *g;
                match bindings.revoke(journal, &p, &reason, &ctx.principal) {
                    Ok(()) => {}
                    // The record could not be written, but cutting someone
                    // off must not wait for the journal: enforce in memory.
                    Err(BindError::Journal(e)) if bindings.key_of(&p).is_some() => {
                        let saved = st.force_revoked.insert(&p);
                        warning = Some(format!(
                            "journal record failed ({e}); the revocation is enforced by the service ({})",
                            persisted_note(&saved)
                        ));
                    }
                    Err(BindError::Degraded(w)) => {
                        let saved = st.force_revoked.insert(&p);
                        warning = Some(format!(
                            "bindings are degraded ({w}); the revocation is enforced by the service ({})",
                            persisted_note(&saved)
                        ));
                    }
                    Err(e) => return Err(admin_err(&e)),
                }
            }
            if let Some(r) = st.registry.by_principal(&p) {
                r.kill(format!("binding revoked: {reason}"));
            }
            owner_changed(ctx, uid);
            st.refresh_facts();
            Ok(warning.map_or(Value::Null, |w| json!({"warning": w})))
        }
        Message::BindRebind { uid, user_pubkey } => {
            let p = Principal::Uid(uid);
            let key = match user_pubkey {
                Some(h) => hexser::decode::<32>(&h).ok_or_else(|| bad("user_pubkey must be 64 hex characters"))?,
                None => st.conflicts.lock().expect("conflicts lock").get(&p).copied().ok_or_else(|| {
                    bad(format!(
                        "no key given and uid {uid} has not offered a conflicting key since the service started"
                    ))
                })?,
            };
            {
                let mut g = st.core.lock().expect("core lock");
                let Core { journal, bindings } = &mut *g;
                bindings.rebind(journal, &p, &key, meta(ctx)).map_err(|e| admin_err(&e))?;
            }
            st.conflicts.lock().expect("conflicts lock").remove(&p);
            st.force_revoked.remove(&p);
            if let Some(r) = st.registry.by_principal(&p) {
                r.kill("binding replaced; register with the new key");
            }
            owner_changed(ctx, uid);
            st.refresh_facts();
            Ok(Value::Null)
        }
        Message::PeerRevoke { node_id, reason } => {
            if !valid_node_id(&node_id) {
                return Err(bad("node_id is empty or has unexpected characters"));
            }
            // Tightening: apply first (revoked.json, disconnect), then journal
            // best-effort. A broken journal must not delay cutting a peer off.
            st.revocations.revoke_host(&node_id, &reason);
            if let Some(rt) = st.router.runtime() {
                rt.disconnect_peer(&node_id);
            }
            let noted = st.try_note("peer.revoke", json!({"node_id": node_id, "reason": reason, "by": by_json(ctx)}));
            Ok(match noted {
                Ok(()) => Value::Null,
                Err(e) => json!({"warning": format!("journal record failed ({e}); the peer is revoked but the revocation is not in the journal")}),
            })
        }
        Message::PeerUnrevoke { node_id } => {
            if !valid_node_id(&node_id) {
                return Err(bad("node_id is empty or has unexpected characters"));
            }
            if st.journal_read_only() {
                return Err(read_only_err());
            }
            st.try_note("peer.unrevoke", json!({"node_id": node_id, "reason": "unrevoke", "by": by_json(ctx)}))
                .map_err(|e| journal_err(&e))?;
            st.revocations.unrevoke_host(&node_id);
            Ok(Value::Null)
        }
        Message::PolicySet { admission, cluster_owner_uid } => {
            if st.journal_read_only() {
                return Err(read_only_err());
            }
            // Validate everything before changing anything.
            let mode = match &admission {
                Some(a) => Some(parse_admission(a).ok_or_else(|| bad("admission must be off, observe or enforce"))?),
                None => None,
            };
            if let Some(m) = mode {
                st.gate.validate_mode(m).map_err(bad)?;
                if m == clawft_types::config::MeshAdmissionMode::Enforce
                    && cluster_owner_uid.or(st.policy.owner_uid()).is_none()
                {
                    return Err(bad(
                        "enforce needs an explicit cluster owner: set cluster_owner_uid first (it is never inferred)",
                    ));
                }
            }
            // Both records are appended before any part is applied; if the
            // second append fails, the part that is in the journal is applied
            // (so state matches what a restart would replay) and the verb errors.
            let mut apply_owner = None;
            let mut apply_mode = None;
            let mut failure = None;
            if let Some(u) = cluster_owner_uid {
                let old = st.policy.owner_uid();
                match st.try_note(
                    "policy.set",
                    json!({"key": "cluster_owner_uid", "old": old, "new": u, "by": by_json(ctx)}),
                ) {
                    Ok(()) => apply_owner = Some(u),
                    Err(e) => failure = Some(e),
                }
            }
            if let (Some(m), None) = (mode, &failure) {
                let old = admission_str(st.policy.admission());
                match st.try_note(
                    "policy.set",
                    json!({"key": "admission", "old": old, "new": admission_str(m), "by": by_json(ctx)}),
                ) {
                    Ok(()) => apply_mode = Some(m),
                    Err(e) => failure = Some(e),
                }
            }
            if let Some(u) = apply_owner {
                st.policy.set_owner(u);
            }
            if let Some(m) = apply_mode {
                st.gate.set_mode(m).map_err(bad)?;
                st.policy.set_admission(m);
                if let Some(rt) = st.router.runtime() {
                    rt.set_enforcing(m == clawft_types::config::MeshAdmissionMode::Enforce);
                }
            }
            if apply_owner.is_some() || apply_mode.is_some() {
                st.verdicts.clear();
            }
            if let Some(e) = failure {
                let mut r = journal_err(&e);
                let applied = match (apply_owner.is_some(), apply_mode.is_some(), cluster_owner_uid.is_some(), mode.is_some()) {
                    (true, false, _, true) => "cluster_owner_uid was changed, admission was not",
                    (false, false, _, _) => "nothing was changed",
                    _ => "the requested changes were applied except the ones the journal refused",
                };
                r.message = format!("{} ({applied})", r.message);
                return Err(r);
            }
            Ok(json!({
                "admission": admission_str(st.policy.admission()),
                "cluster_owner_uid": st.policy.owner_uid(),
                "note": "mesh.toml is not rewritten; the journal holds the override. Listener connection limits follow the mode at the next restart.",
            }))
        }
        Message::JournalVerify {} => {
            let g = st.core.lock().expect("core lock");
            let report = verify_dir(g.journal.dir(), &st.machine_key).map_err(|e| bad(e.to_string()))?;
            Ok(json!({
                "ok": report.bad.is_none(),
                "records": report.records, "head_seq": report.head_seq, "head_hash": report.head_hash,
                "bad": report.bad.map(|(f, r)| json!({"file": f, "reason": r})),
                "read_only": g.journal.read_only(),
                "last_auto_accept": g.journal.last_auto_accept().map(|(seq, at)| json!({"seq": seq, "at": at})),
                "pending_quarantines": g.journal.pending_quarantines(),
                "latest_pending_quarantine": g.journal.latest_pending_quarantine(),
                "degraded": g.bindings.degraded(),
                "lost": g.journal.lost(),
            }))
        }
        Message::JournalAcceptTruncate { quarantine_seq, floor } => {
            {
                let mut g = st.core.lock().expect("core lock");
                let Core { journal, bindings } = &mut *g;
                let seq = quarantine_seq.or_else(|| journal.latest_pending_quarantine());
                // Only reachable behind the admin gate in `handle_common`.
                let ack = AdminAck::admin_verified(ctx.principal.clone());
                bindings.accept_truncate(journal, ack, seq, floor).map_err(|e| admin_err(&e))?;
            }
            st.refresh_facts();
            Ok(Value::Null)
        }
        _ => Err(bad("not an admin verb")),
    }
}
