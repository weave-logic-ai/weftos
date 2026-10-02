//! Principal-to-key bindings, a pure fold of the machine journal (plan 1.3).
//!
//! The journal is the only source of truth; this state is rebuilt by
//! [`Bindings::fold`] and never persisted. Live mutators validate against the
//! current state, append the record, then apply it, so the in-memory state is
//! by construction what a fold of the journal would produce.
//!
//! Invariants, enforced here and not in callers: one key per principal, one
//! principal per key, a revoked key is never accepted again, and a rebind
//! revokes every serial issued to the old key.

use std::collections::{HashMap, HashSet};

use clawft_mesh_local::{node_id_from_pubkey, Principal};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::journal::{Journal, JournalError, Record};

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

/// Optional context recorded with a bind.
#[derive(Debug, Clone, Default)]
pub struct BindMeta {
    pub by: Option<Principal>,
    pub peer_pid: Option<u32>,
    pub exe: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReason {
    /// The principal is already bound to a different key.
    PrincipalHasOtherKey,
    /// The key is bound to a different principal.
    KeyBoundToOtherPrincipal,
    /// The key was revoked (or replaced by a rebind) and is never reusable.
    KeyRevoked,
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
    #[error("certificate serial {got} is not above the last issued serial {last}")]
    SerialNotMonotonic { got: u64, last: u64 },
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
struct BindBody {
    principal: Principal,
    #[serde(with = "clawft_mesh_local::hexser::hex32")]
    user_pubkey: [u8; 32],
    user_id: String,
    how: BindHow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    by: Option<Principal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    peer_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exe: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct PendingBody {
    principal: Principal,
    #[serde(with = "clawft_mesh_local::hexser::hex32")]
    user_pubkey: [u8; 32],
    user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    peer_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exe: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct CertBody {
    user_id: String,
    serial: u64,
    issued_at: u64,
    not_after: u64,
}

#[derive(Serialize, Deserialize, Clone)]
struct RevokeBody {
    principal: Principal,
    user_id: String,
    reason: String,
    by: Principal,
    serials_revoked_through: Option<u64>,
}

#[derive(Clone)]
enum Event {
    Bind(BindBody),
    Pending(PendingBody),
    Cert(CertBody),
    Revoke(RevokeBody),
    /// Kinds this fold does not interpret (peer.*, policy.*, ...).
    Other,
}

impl Event {
    fn parse(rec: &Record) -> Result<Event, BindError> {
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
            _ => Event::Other,
        })
    }

    fn kind_body(&self) -> Result<(&'static str, Value), serde_json::Error> {
        Ok(match self {
            Event::Bind(b) => (KIND_BIND, serde_json::to_value(b)?),
            Event::Pending(b) => (KIND_BIND_PENDING, serde_json::to_value(b)?),
            Event::Cert(b) => (KIND_CERT_ISSUE, serde_json::to_value(b)?),
            Event::Revoke(b) => (KIND_REVOKE, serde_json::to_value(b)?),
            Event::Other => ("", Value::Null),
        })
    }
}

/// The folded state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings {
    by_principal: HashMap<Principal, [u8; 32]>,
    by_key: HashMap<[u8; 32], Principal>,
    /// user_id of every currently bound key.
    user_ids: HashMap<String, [u8; 32]>,
    pending: HashMap<Principal, [u8; 32]>,
    revoked_keys: HashSet<[u8; 32]>,
    issued: HashMap<String, Vec<u64>>,
    revoked_through: HashMap<String, u64>,
    last_serial: u64,
}

impl Bindings {
    /// Rebuild the state from the whole journal.
    pub fn fold(journal: &Journal) -> Result<Self, BindError> {
        let mut b = Bindings::default();
        for rec in journal.iter() {
            b.apply(rec)?;
        }
        Ok(b)
    }

    /// Apply one journal record (validating it against the current state).
    pub fn apply(&mut self, rec: &Record) -> Result<(), BindError> {
        let ev = Event::parse(rec)?;
        self.validate(&ev)
            .map_err(|e| BindError::Replay { seq: rec.seq, source: Box::new(e) })?;
        self.commit(&ev);
        Ok(())
    }

    pub fn check(&self, principal: &Principal, key: &[u8; 32]) -> Check {
        match self.by_principal.get(principal) {
            Some(k) if k == key => return Check::Existing,
            Some(_) => return Check::Conflict(ConflictReason::PrincipalHasOtherKey),
            None => {}
        }
        if self.by_key.contains_key(key) {
            return Check::Conflict(ConflictReason::KeyBoundToOtherPrincipal);
        }
        if self.revoked_keys.contains(key) {
            return Check::Conflict(ConflictReason::KeyRevoked);
        }
        if self.pending.get(principal) == Some(key) {
            return Check::Pending;
        }
        Check::New
    }

    pub fn key_of(&self, principal: &Principal) -> Option<[u8; 32]> {
        self.by_principal.get(principal).copied()
    }

    pub fn principal_of(&self, key: &[u8; 32]) -> Option<&Principal> {
        self.by_key.get(key)
    }

    /// Serials ever issued to the principal's current key.
    pub fn serials(&self, principal: &Principal) -> Vec<u64> {
        self.key_of(principal)
            .and_then(|k| self.issued.get(&node_id_from_pubkey(&k)))
            .cloned()
            .unwrap_or_default()
    }

    /// Every issued serial that is revoked, ascending (rides in the signed facts).
    pub fn revoked_serials(&self) -> Vec<u64> {
        let mut out: Vec<u64> = self
            .revoked_through
            .iter()
            .flat_map(|(uid, through)| {
                self.issued.get(uid).into_iter().flatten().copied().filter(move |s| s <= through)
            })
            .collect();
        out.sort_unstable();
        out
    }

    pub fn is_serial_revoked(&self, user_id: &str, serial: u64) -> bool {
        self.revoked_through.get(user_id).is_some_and(|t| serial <= *t)
    }

    pub fn last_serial(&self) -> u64 {
        self.last_serial
    }

    // ---- live mutators: validate, append, apply -------------------------

    /// First contact binding (`tofu` or `approved`). Idempotent for an
    /// existing identical binding (nothing is appended).
    pub fn bind(
        &mut self,
        journal: &mut Journal,
        principal: &Principal,
        key: &[u8; 32],
        how: BindHow,
        meta: BindMeta,
    ) -> Result<(), BindError> {
        if how == BindHow::Rebind {
            return Err(BindError::InvalidHow);
        }
        match self.check(principal, key) {
            Check::Existing => return Ok(()),
            Check::Conflict(r) => return Err(BindError::Conflict(r)),
            Check::New | Check::Pending => {}
        }
        self.write(journal, true, Event::Bind(bind_body(principal, key, how, meta)))
    }

    /// Replace the principal's key. Revokes the old key's serials and the old
    /// key itself in the same record.
    pub fn rebind(
        &mut self,
        journal: &mut Journal,
        principal: &Principal,
        new_key: &[u8; 32],
        meta: BindMeta,
    ) -> Result<(), BindError> {
        if self.key_of(principal).as_ref() == Some(new_key) {
            return Ok(());
        }
        self.write(journal, true, Event::Bind(bind_body(principal, new_key, BindHow::Rebind, meta)))
    }

    /// Record a bind awaiting admin approval (policy `approve`).
    pub fn bind_pending(
        &mut self,
        journal: &mut Journal,
        principal: &Principal,
        key: &[u8; 32],
        meta: BindMeta,
    ) -> Result<(), BindError> {
        match self.check(principal, key) {
            Check::Pending | Check::Existing => return Ok(()),
            Check::Conflict(r) => return Err(BindError::Conflict(r)),
            Check::New => {}
        }
        let ev = Event::Pending(PendingBody {
            principal: principal.clone(),
            user_pubkey: *key,
            user_id: node_id_from_pubkey(key),
            peer_pid: meta.peer_pid,
            exe: meta.exe,
        });
        self.write(journal, true, ev)
    }

    /// Record a certificate issue and return its serial. Refused while the
    /// journal is read-only.
    pub fn issue_cert(
        &mut self,
        journal: &mut Journal,
        principal: &Principal,
        issued_at: u64,
        not_after: u64,
    ) -> Result<u64, BindError> {
        let key = self.key_of(principal).ok_or(BindError::NotBound)?;
        let serial = self.last_serial + 1;
        let ev = Event::Cert(CertBody {
            user_id: node_id_from_pubkey(&key),
            serial,
            issued_at,
            not_after,
        });
        self.write(journal, true, ev)?;
        Ok(serial)
    }

    /// Revoke the principal's binding and every serial issued to its key.
    /// Allowed while read-only (it only reduces trust).
    pub fn revoke(
        &mut self,
        journal: &mut Journal,
        principal: &Principal,
        reason: &str,
        by: &Principal,
    ) -> Result<(), BindError> {
        let key = self.key_of(principal).ok_or(BindError::NotBound)?;
        let user_id = node_id_from_pubkey(&key);
        let through = self.issued.get(&user_id).and_then(|v| v.iter().max().copied());
        let ev = Event::Revoke(RevokeBody {
            principal: principal.clone(),
            user_id,
            reason: reason.to_string(),
            by: by.clone(),
            serials_revoked_through: through,
        });
        self.write(journal, false, ev)
    }

    fn write(&mut self, journal: &mut Journal, needs_rw: bool, ev: Event) -> Result<(), BindError> {
        if needs_rw && journal.read_only() {
            return Err(JournalError::ReadOnly.into());
        }
        self.validate(&ev)?;
        let (kind, body) = ev.kind_body().map_err(JournalError::from)?;
        journal.append(kind, body)?;
        self.commit(&ev);
        Ok(())
    }

    // ---- the state machine ------------------------------------------------

    fn validate(&self, ev: &Event) -> Result<(), BindError> {
        match ev {
            Event::Other => Ok(()),
            Event::Pending(p) => {
                id_matches(&p.user_pubkey, &p.user_id)?;
                match self.check(&p.principal, &p.user_pubkey) {
                    Check::New | Check::Pending => Ok(()),
                    Check::Existing => Err(BindError::AlreadyBound),
                    Check::Conflict(r) => Err(BindError::Conflict(r)),
                }
            }
            Event::Bind(b) => {
                id_matches(&b.user_pubkey, &b.user_id)?;
                if b.how == BindHow::Rebind {
                    let old = self.by_principal.get(&b.principal).ok_or(BindError::NotBound)?;
                    if *old == b.user_pubkey {
                        return Err(BindError::AlreadyBound);
                    }
                    if self.by_key.contains_key(&b.user_pubkey) {
                        return Err(BindError::Conflict(ConflictReason::KeyBoundToOtherPrincipal));
                    }
                    if self.revoked_keys.contains(&b.user_pubkey) {
                        return Err(BindError::Conflict(ConflictReason::KeyRevoked));
                    }
                    return Ok(());
                }
                match self.check(&b.principal, &b.user_pubkey) {
                    Check::New | Check::Pending => Ok(()),
                    Check::Existing => Err(BindError::AlreadyBound),
                    Check::Conflict(r) => Err(BindError::Conflict(r)),
                }
            }
            Event::Cert(c) => {
                if !self.user_ids.contains_key(&c.user_id) {
                    return Err(BindError::UnknownUser);
                }
                if c.serial <= self.last_serial {
                    return Err(BindError::SerialNotMonotonic { got: c.serial, last: self.last_serial });
                }
                if c.not_after <= c.issued_at {
                    return Err(BindError::BadValidity);
                }
                Ok(())
            }
            Event::Revoke(r) => {
                let key = self.by_principal.get(&r.principal).ok_or(BindError::NotBound)?;
                if node_id_from_pubkey(key) != r.user_id {
                    return Err(BindError::UserIdMismatch);
                }
                let max = self.issued.get(&r.user_id).and_then(|v| v.iter().max().copied());
                if r.serials_revoked_through != max {
                    return Err(BindError::RevokeMismatch);
                }
                Ok(())
            }
        }
    }

    /// Apply an already validated event.
    fn commit(&mut self, ev: &Event) {
        match ev {
            Event::Other => {}
            Event::Pending(p) => {
                self.pending.insert(p.principal.clone(), p.user_pubkey);
            }
            Event::Bind(b) => {
                if let Some(old) = self.by_principal.get(&b.principal).copied() {
                    self.drop_key(&b.principal, &old);
                }
                self.pending.remove(&b.principal);
                self.by_principal.insert(b.principal.clone(), b.user_pubkey);
                self.by_key.insert(b.user_pubkey, b.principal.clone());
                self.user_ids.insert(b.user_id.clone(), b.user_pubkey);
            }
            Event::Cert(c) => {
                self.issued.entry(c.user_id.clone()).or_default().push(c.serial);
                self.last_serial = c.serial;
            }
            Event::Revoke(r) => {
                if let Some(key) = self.by_principal.get(&r.principal).copied() {
                    self.drop_key(&r.principal, &key);
                }
                self.pending.remove(&r.principal);
            }
        }
    }

    /// Unbind `key`, mark it revoked and revoke every serial issued to it.
    fn drop_key(&mut self, principal: &Principal, key: &[u8; 32]) {
        let uid = node_id_from_pubkey(key);
        if let Some(max) = self.issued.get(&uid).and_then(|v| v.iter().max().copied()) {
            self.revoked_through.insert(uid.clone(), max);
        }
        self.by_principal.remove(principal);
        self.by_key.remove(key);
        self.user_ids.remove(&uid);
        self.revoked_keys.insert(*key);
    }
}

fn id_matches(key: &[u8; 32], user_id: &str) -> Result<(), BindError> {
    if node_id_from_pubkey(key) == user_id {
        Ok(())
    } else {
        Err(BindError::UserIdMismatch)
    }
}

fn bind_body(principal: &Principal, key: &[u8; 32], how: BindHow, meta: BindMeta) -> BindBody {
    BindBody {
        principal: principal.clone(),
        user_pubkey: *key,
        user_id: node_id_from_pubkey(key),
        how,
        by: meta.by,
        peer_pid: meta.peer_pid,
        exe: meta.exe,
    }
}
