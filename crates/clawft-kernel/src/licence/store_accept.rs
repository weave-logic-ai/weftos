//! Accepting bindings and grants into a [`CheckoutGrantStore`]
//! (ADR-106 sections 3 and 4). Every change is built on a copy, saved, and
//! only then made the state; records that only restrict (an unbind, a
//! withdrawal, a conflict) are applied even if the save fails.

use super::store::{CheckoutGrantStore, Held};
use super::{
    AdmissionPosture, BindState, BindingExtraCheck, CheckoutGrant, GRANT_SKEW_SECS, LicenceError,
    LicenceEvent, MAX_GRANT_SLOTS, Outcome, SignedBinding, SignedGrant, verify_binding_member,
    verify_grant,
};
use crate::workload_pkg::codec::hex_decode_exact;

impl CheckoutGrantStore {
    /// Accept a binding. `posture` is the node's admission state; `extra` is
    /// the steward profile hook (use [`super::NoExtraChecks`] for members).
    pub fn accept_binding(
        &self,
        signed: &SignedBinding,
        posture: AdmissionPosture,
        extra: &dyn BindingExtraCheck,
    ) -> Result<Outcome, LicenceError> {
        self.run(|inner, ev| {
            if let Some(p) = &inner.poisoned {
                return Err(LicenceError::Poisoned(p.clone()));
            }
            if let Err(e) = posture.check() {
                ev.push(LicenceEvent::BindingRefused(e.to_string()));
                return Err(e);
            }
            let local = self.local.get().ok_or(LicenceError::NoLocalMesh)?;
            let rec = verify_binding_member(signed, &self.anchors, &local)?;
            extra.check(&rec)?;
            let mut next = inner.clone();
            if let Some(cur) = &inner.binding {
                if rec.seq < cur.body.seq {
                    return Ok(Outcome::Ignored);
                }
                if rec.seq == cur.body.seq {
                    if cur.signed.payload == signed.payload {
                        return Ok(Outcome::Duplicate);
                    }
                    ev.push(LicenceEvent::BindingConflict(rec.seq));
                    return Err(LicenceError::Conflict(rec.seq));
                }
                if cur.body.grant_pubkey != rec.grant_pubkey {
                    next.slots.clear(); // grants under the old key are void
                }
            }
            let key = rec.grant_pubkey.clone();
            next.floors.retain(|k, _| *k == key); // floors of old keys are dead weight
            let restrictive = rec.state == BindState::Unbound;
            next.binding = Some(Held { signed: signed.clone(), body: rec });
            next.orphan_reported = false;
            if restrictive {
                return Ok(self.commit_restrictive(inner, next));
            }
            self.commit(inner, next)?;
            self.note_bound();
            Ok(Outcome::Applied)
        })
    }

    /// Accept a grant under the bound key (see ADR-106 section 4).
    pub fn accept_grant(&self, signed: &SignedGrant) -> Result<Outcome, LicenceError> {
        self.run(|inner, ev| {
            let b = self.binding_in_effect(inner, ev)?;
            let pk = hex_decode_exact::<32>(&b.grant_pubkey)
                .ok_or_else(|| LicenceError::Malformed("bound grant key".into()))?;
            if self.key_revoked(&b.grant_pubkey) {
                return Err(LicenceError::KeyRevoked);
            }
            let local = self.local.get().ok_or(LicenceError::NoLocalMesh)?;
            let g = verify_grant(signed, &pk, &local)?;
            let eff = self.eff_now(inner, &b.grant_pubkey);
            if g.issued_at > eff.saturating_add(GRANT_SKEW_SECS) {
                return Err(LicenceError::NotYetValid);
            }
            let key = (g.cog_id.clone(), g.version.clone());
            if !inner.slots.contains_key(&key) && inner.slots.len() >= MAX_GRANT_SLOTS {
                return Err(LicenceError::Full);
            }
            let held = Held { signed: signed.clone(), body: g };
            let seq = held.body.seq;
            let (cur, conflicted) = match inner.slots.get(&key) {
                Some(s) => (s.current.clone(), s.conflicted.contains(&seq)),
                None => (None, false),
            };
            if conflicted {
                return Err(LicenceError::Conflict(seq));
            }
            let mut next = inner.clone();
            if let Some(cur) = &cur {
                if seq < cur.body.seq {
                    return Ok(Outcome::Ignored);
                }
                if seq == cur.body.seq {
                    if cur.signed.payload == held.signed.payload {
                        return Ok(Outcome::Duplicate);
                    }
                    // Two payloads at one seq: refuse both. If either is a
                    // withdrawal, fail closed and keep it as the tombstone;
                    // otherwise fall back to the newest earlier grant.
                    let wd = [cur, &held].into_iter().find(|h| h.body.is_withdrawal()).cloned();
                    let slot = next.slots.entry(key).or_default();
                    slot.conflicted.push(seq);
                    match wd {
                        Some(w) => {
                            slot.current = Some(w);
                            slot.previous = None;
                        }
                        None => slot.current = slot.previous.take(),
                    }
                    ev.push(LicenceEvent::GrantConflict {
                        cog_id: held.body.cog_id,
                        version: held.body.version,
                        seq,
                    });
                    let _ = self.commit_restrictive(inner, next);
                    return Err(LicenceError::Conflict(seq));
                }
                check_union(&cur.body, &held.body)?;
            }
            let issued = held.body.issued_at;
            let restrictive = held.body.is_withdrawal();
            let slot = next.slots.entry(key).or_default();
            slot.previous = slot.current.take();
            slot.current = Some(held);
            next.floors.entry(b.grant_pubkey).or_default().note_issued(issued);
            if restrictive {
                return Ok(self.commit_restrictive(inner, next));
            }
            self.commit(inner, next)?;
            Ok(Outcome::Applied)
        })
    }
}

/// A newer grant carries the union of arches and never changes a held one.
fn check_union(cur: &CheckoutGrant, new: &CheckoutGrant) -> Result<(), LicenceError> {
    for a in &cur.artifacts {
        match new.artifact(&a.arch) {
            None => return Err(LicenceError::DropsArch(a.arch.clone())),
            Some(n) if n != a => return Err(LicenceError::ChangesArtifact(a.arch.clone())),
            Some(_) => {}
        }
    }
    Ok(())
}
