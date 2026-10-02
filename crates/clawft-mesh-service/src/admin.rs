//! Admin verbs (`role: "admin"` connections from root or `admin_uids`).
//!
//! Admin connections hold no keys: they are authorised by the peer
//! credential alone, checked at connect time against a configuration that
//! cannot change while the service runs. Every state change is a journal
//! record naming `by`.

use clawft_mesh_local::framing::FrameError;
use clawft_mesh_local::proto::{ErrorKind, Message};
use clawft_mesh_local::{hexser, Principal};
use serde_json::{json, Value};

use crate::bindings_view::bindings_json;
use crate::local_server::{Conn, Step};
use crate::register::Reject;
use crate::state::{admission_str, parse_admission, Core};
use crate::{verify_dir, AdminAck, BindError, BindHow, BindMeta};

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

impl Conn {
    pub(crate) async fn admin_verb(&mut self, id: Option<u64>, m: Message) -> Result<Step, FrameError> {
        match self.admin_dispatch(m) {
            Ok(Value::Null) => self.send(id, Message::Ack {}).await?,
            Ok(data) => self.send(id, Message::Reply { data }).await?,
            // Refused, not fatal: the admin may try again.
            Err(r) => self.error(id, r.kind, r.message, r.remedy).await?,
        }
        Ok(Step::Continue)
    }

    fn meta(&self) -> BindMeta {
        BindMeta { by: Some(self.principal.clone()), peer_pid: Some(self.pid), exe: Some(self.exe.clone()) }
    }

    fn by_json(&self) -> Value {
        serde_json::to_value(&self.principal).unwrap_or(Value::Null)
    }

    /// Synchronous on purpose: nothing awaits while the core lock is held.
    fn admin_dispatch(&mut self, m: Message) -> Result<Value, Reject> {
        let st = self.st.clone();
        match m {
            Message::BindingsList {} => {
                let c = st.core.lock().expect("core lock");
                Ok(bindings_json(&c, &st.registry))
            }
            Message::BindApprove { uid } => {
                let p = Principal::Uid(uid);
                {
                    let mut g = st.core.lock().expect("core lock");
                    let Core { journal, bindings } = &mut *g;
                    let key = bindings
                        .pending_key(&p)
                        .ok_or_else(|| bad(format!("nothing is pending approval for uid {uid}")))?;
                    bindings.bind(journal, &p, &key, BindHow::Approved, self.meta()).map_err(|e| admin_err(&e))?;
                }
                st.policy.note_bind(uid);
                Ok(Value::Null)
            }
            Message::BindRevoke { uid, reason } => {
                let p = Principal::Uid(uid);
                {
                    let mut g = st.core.lock().expect("core lock");
                    let Core { journal, bindings } = &mut *g;
                    bindings.revoke(journal, &p, &reason, &self.principal).map_err(|e| admin_err(&e))?;
                }
                if let Some(r) = st.registry.by_principal(&p) {
                    r.kill(format!("binding revoked: {reason}"));
                }
                st.refresh_facts();
                Ok(Value::Null)
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
                    bindings.rebind(journal, &p, &key, self.meta()).map_err(|e| admin_err(&e))?;
                }
                st.conflicts.lock().expect("conflicts lock").remove(&p);
                if let Some(r) = st.registry.by_principal(&p) {
                    r.kill("binding replaced; register with the new key");
                }
                st.refresh_facts();
                Ok(Value::Null)
            }
            Message::PeerRevoke { node_id, reason } => {
                if !valid_node_id(&node_id) {
                    return Err(bad("node_id is empty or has unexpected characters"));
                }
                st.revocations.revoke_host(&node_id, &reason);
                st.note("peer.revoke", json!({"node_id": node_id, "reason": reason, "by": self.by_json()}));
                if let Some(rt) = st.router.runtime() {
                    rt.disconnect_peer(&node_id);
                }
                Ok(Value::Null)
            }
            Message::PeerUnrevoke { node_id } => {
                if !valid_node_id(&node_id) {
                    return Err(bad("node_id is empty or has unexpected characters"));
                }
                st.revocations.unrevoke_host(&node_id);
                st.note("peer.unrevoke", json!({"node_id": node_id, "reason": "unrevoke", "by": self.by_json()}));
                Ok(Value::Null)
            }
            Message::PolicySet { admission, cluster_owner_uid } => {
                if let Some(a) = admission {
                    let mode = parse_admission(&a).ok_or_else(|| bad("admission must be off, observe or enforce"))?;
                    let old = admission_str(st.policy.admission());
                    st.gate.set_mode(mode).map_err(bad)?;
                    st.policy.set_admission(mode);
                    st.note(
                        "policy.set",
                        json!({"key": "admission", "old": old, "new": admission_str(mode), "by": self.by_json()}),
                    );
                }
                if let Some(u) = cluster_owner_uid {
                    let old = st.policy.owner_uid();
                    st.policy.set_owner(u);
                    st.note(
                        "policy.set",
                        json!({"key": "cluster_owner_uid", "old": old, "new": u, "by": self.by_json()}),
                    );
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
                    let ack = AdminAck::admin_verified(self.principal.clone());
                    bindings.accept_truncate(journal, ack, seq, floor).map_err(|e| admin_err(&e))?;
                }
                st.refresh_facts();
                Ok(Value::Null)
            }
            _ => Err(bad("not an admin verb")),
        }
    }
}
