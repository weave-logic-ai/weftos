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
//!
//! After a `user.revoke` the same principal may not TOFU-bind a new key: it
//! needs an `Approved` bind (carrying `by`), because the revocation usually
//! means the account or key was compromised. A rebind (principal still bound)
//! is the normal key-replacement path.
//!
//! If the fold itself fails (a validly signed but semantically invalid
//! record), [`Bindings::fold_lenient`] yields the state up to that record,
//! marked [`Bindings::degraded`]; every mutator then refuses, so the service
//! can come up read-only with a loud error instead of refusing to start.

use std::collections::{HashMap, HashSet};

use clawft_mesh_local::{node_id_from_pubkey, Principal};
use crate::bind_events::{
    id_matches, now, AcceptBody, BindBody, CertBody, Event, PendingBody, RevokeBody,
};
pub use crate::bind_events::{BindError, Check, ConflictReason, BindHow, KIND_BIND, KIND_BIND_PENDING, KIND_CERT_ISSUE, KIND_REVOKE};
use crate::journal::{AdminAck, Journal, JournalError, Record};

/// Optional context recorded with a bind.
#[derive(Debug, Clone, Default)]
pub struct BindMeta {
    pub by: Option<Principal>,
    pub peer_pid: Option<u32>,
    pub exe: Option<String>,
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
    /// Principals whose binding was revoked and not yet re-approved.
    revoked_principals: HashSet<Principal>,
    /// Users a quarantine proved revoked (their keys are never bindable).
    lost_revoked_ids: HashSet<String>,
    /// Seqs of `journal.quarantine` records not yet accepted.
    pending_quarantines: std::collections::BTreeSet<u64>,
    degraded: Option<String>,
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

    /// Like [`Bindings::fold`] but never fails: on an invalid record the state
    /// so far is returned, marked degraded (all mutators refuse).
    pub fn fold_lenient(journal: &Journal) -> Self {
        let mut b = Bindings::default();
        for rec in journal.iter() {
            if b.degraded.is_some() {
                // Quarantine facts only ever tighten the state; keep applying them.
                match Event::parse(rec) {
                    Ok(ev @ Event::Quarantine(_)) => {
                        b.commit(&ev);
                        b.pending_quarantines.insert(rec.seq);
                    }
                    Ok(ev @ Event::Accept(_)) => b.commit(&ev),
                    _ => {}
                }
                continue;
            }
            if let Err(e) = b.apply(rec) {
                b.degraded = Some(e.to_string());
            }
        }
        b
    }

    /// Why the fold stopped early, if it did.
    pub fn degraded(&self) -> Option<&str> {
        self.degraded.as_deref()
    }

    /// Apply the signed facts of a quarantine: serials never go below the
    /// (clamped) high-water mark, every revoked user's serials up to it are
    /// revoked, and those users' keys can never be bound again, even if the
    /// bind itself was lost with the tail.
    pub(crate) fn apply_lost(&mut self, high_water: u64, revoked_user_ids: &[String]) {
        self.last_serial = self.last_serial.max(high_water);
        for id in revoked_user_ids {
            self.lost_revoked_ids.insert(id.clone());
            let bound = self.user_ids.get(id).and_then(|k| self.by_key.get(k).map(|p| (p.clone(), *k)));
            if let Some((p, key)) = bound {
                self.drop_key(&p, &key);
                self.revoked_principals.insert(p);
            }
            let t = self.revoked_through.entry(id.clone()).or_insert(0);
            *t = (*t).max(high_water);
        }
    }

    /// Admin acknowledgement of one quarantined tail, named by the seq of its
    /// `journal.quarantine` record (see `Journal::pending_quarantines`): the
    /// gate is lifted only for that quarantine. `floor` is an explicit
    /// admin-chosen serial floor; it can only raise the floor (default: the
    /// clamped quarantine value already in the state) and may exceed the
    /// current last serial by at most 2^32. The constraining facts come from
    /// the signed quarantine record, never from the marker file.
    pub fn accept_truncate(
        &mut self,
        journal: &mut Journal,
        ack: AdminAck,
        quarantine_seq: u64,
        floor: Option<u64>,
    ) -> Result<(), BindError> {
        self.refuse_degraded()?;
        let max = self.last_serial.saturating_add(MAX_FLOOR_JUMP);
        let floor = floor.unwrap_or(self.last_serial);
        if floor > max {
            return Err(BindError::FloorTooHigh { floor, max });
        }
        let quarantine = journal.lost().map(|l| l.quarantine.clone()).unwrap_or_default();
        let ev = Event::Accept(AcceptBody { quarantine_seq, serial_floor: floor, quarantine, by: ack.by.clone() });
        self.write(journal, false, ev)?;
        journal.clear_lost(&ack)?;
        Ok(())
    }

    fn refuse_degraded(&self) -> Result<(), BindError> {
        match &self.degraded {
            Some(m) => Err(BindError::Degraded(m.clone())),
            None => Ok(()),
        }
    }

    /// Apply one journal record (validating it against the current state).
    pub fn apply(&mut self, rec: &Record) -> Result<(), BindError> {
        let ev = Event::parse(rec)?;
        self.validate(&ev)
            .map_err(|e| BindError::Replay { seq: rec.seq, source: Box::new(e) })?;
        self.commit(&ev);
        if matches!(ev, Event::Quarantine(_)) {
            self.pending_quarantines.insert(rec.seq);
        }
        Ok(())
    }

    pub fn check(&self, principal: &Principal, key: &[u8; 32]) -> Check {
        if self.degraded.is_some() {
            return Check::Conflict(ConflictReason::Degraded);
        }
        match self.by_principal.get(principal) {
            Some(k) if k == key => return Check::Existing,
            Some(_) => return Check::Conflict(ConflictReason::PrincipalHasOtherKey),
            None => {}
        }
        if self.by_key.contains_key(key) {
            return Check::Conflict(ConflictReason::KeyBoundToOtherPrincipal);
        }
        if self.revoked_keys.contains(key) || self.lost_revoked_ids.contains(&node_id_from_pubkey(key)) {
            return Check::Conflict(ConflictReason::KeyRevoked);
        }
        if self.pending.get(principal) == Some(key) {
            return Check::Pending;
        }
        Check::New
    }

    /// The principal's bound key. `None` when degraded (partial state is never served).
    pub fn key_of(&self, principal: &Principal) -> Option<[u8; 32]> {
        if self.degraded.is_some() {
            return None;
        }
        self.by_principal.get(principal).copied()
    }

    /// Internal lookup that ignores degradation (mutators refuse separately).
    fn bound_key(&self, principal: &Principal) -> Option<[u8; 32]> {
        self.by_principal.get(principal).copied()
    }

    /// `None` when degraded.
    pub fn principal_of(&self, key: &[u8; 32]) -> Option<&Principal> {
        if self.degraded.is_some() {
            return None;
        }
        self.by_key.get(key)
    }

    /// Serials ever issued to the principal's current key (empty when degraded).
    pub fn serials(&self, principal: &Principal) -> Vec<u64> {
        self.key_of(principal)
            .and_then(|k| self.issued.get(&node_id_from_pubkey(&k)))
            .cloned()
            .unwrap_or_default()
    }

    /// Every issued serial that is revoked, ascending (rides in the signed
    /// facts). An error when degraded, so partial state can never be
    /// published as complete facts.
    pub fn revoked_serials(&self) -> Result<Vec<u64>, BindError> {
        self.refuse_degraded()?;
        let mut out: Vec<u64> = self
            .revoked_through
            .iter()
            .flat_map(|(uid, through)| {
                self.issued.get(uid).into_iter().flatten().copied().filter(move |s| s <= through)
            })
            .collect();
        out.sort_unstable();
        Ok(out)
    }

    pub fn is_serial_revoked(&self, user_id: &str, serial: u64) -> bool {
        self.degraded.is_some() || self.revoked_through.get(user_id).is_some_and(|t| serial <= *t)
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
        if self.bound_key(principal).as_ref() == Some(new_key) {
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
        let key = self.bound_key(principal).ok_or(BindError::NotBound)?;
        let serial = self.last_serial.checked_add(1).ok_or(BindError::SerialExhausted)?;
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
        let key = self.bound_key(principal).ok_or(BindError::NotBound)?;
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
        self.refuse_degraded()?;
        if needs_rw && journal.read_only() {
            return Err(JournalError::ReadOnly.into());
        }
        self.validate(&ev)?;
        let (kind, body) = ev.kind_body().map_err(JournalError::from)?;
        journal.append_raw(now(), kind, body)?;
        self.commit(&ev);
        Ok(())
    }

    // ---- the state machine ------------------------------------------------

    fn validate(&self, ev: &Event) -> Result<(), BindError> {
        match ev {
            Event::Other | Event::Quarantine(_) => Ok(()),
            Event::Accept(a) => {
                if self.pending_quarantines.contains(&a.quarantine_seq) {
                    Ok(())
                } else {
                    Err(BindError::NoSuchQuarantine(a.quarantine_seq))
                }
            }
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
                if b.how == BindHow::Approved && b.by.is_none() {
                    return Err(BindError::ApprovalWithoutApprover);
                }
                if b.how == BindHow::Tofu && self.revoked_principals.contains(&b.principal) {
                    return Err(BindError::ApprovalRequired);
                }
                if b.how == BindHow::Rebind {
                    let old = self.by_principal.get(&b.principal).ok_or(BindError::NotBound)?;
                    if *old == b.user_pubkey {
                        return Err(BindError::AlreadyBound);
                    }
                    if self.by_key.contains_key(&b.user_pubkey) {
                        return Err(BindError::Conflict(ConflictReason::KeyBoundToOtherPrincipal));
                    }
                    if self.revoked_keys.contains(&b.user_pubkey) || self.lost_revoked_ids.contains(&b.user_id) {
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
                let expected = self.last_serial.checked_add(1).ok_or(BindError::SerialExhausted)?;
                if c.serial != expected {
                    return Err(BindError::SerialOutOfSequence { got: c.serial, expected });
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
            Event::Accept(a) => {
                self.last_serial = self.last_serial.max(a.serial_floor);
                self.pending_quarantines.remove(&a.quarantine_seq);
            }
            Event::Quarantine(q) => self.apply_lost(q.serial_high_water, &q.revoked_user_ids),
            Event::Pending(p) => {
                self.pending.insert(p.principal.clone(), p.user_pubkey);
            }
            Event::Bind(b) => {
                if let Some(old) = self.by_principal.get(&b.principal).copied() {
                    self.drop_key(&b.principal, &old);
                }
                self.pending.remove(&b.principal);
                self.revoked_principals.remove(&b.principal);
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
                self.revoked_principals.insert(r.principal.clone());
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

/// Largest jump an admin may impose on the serial floor in one acceptance.
pub const MAX_FLOOR_JUMP: u64 = 1 << 32;
