//! Read-only enumerations of the folded state (service listings and signed
//! facts). Additive to the package J API; nothing here mutates.

use super::*;

impl Bindings {
    /// Every currently bound principal with its key, ordered by user id.
    /// Empty when degraded (partial state is never served).
    pub fn bound_principals(&self) -> Vec<(Principal, [u8; 32])> {
        if self.degraded.is_some() {
            return Vec::new();
        }
        let mut v: Vec<_> = self.by_principal.iter().map(|(p, k)| (p.clone(), *k)).collect();
        v.sort_by_key(|(_, k)| node_id_from_pubkey(k));
        v
    }

    /// Binds awaiting admin approval.
    pub fn pending_principals(&self) -> Vec<(Principal, [u8; 32])> {
        let mut v: Vec<_> = self.pending.iter().map(|(p, k)| (p.clone(), *k)).collect();
        v.sort_by_key(|(_, k)| node_id_from_pubkey(k));
        v
    }

    /// The key a principal has pending approval, if any.
    pub fn pending_key(&self, principal: &Principal) -> Option<[u8; 32]> {
        self.pending.get(principal).copied()
    }

    /// Principals whose binding was revoked and not yet re-approved.
    pub fn revoked_principal_list(&self) -> Vec<Principal> {
        self.revoked_principals.iter().cloned().collect()
    }

    /// `(user_id, through)` for every user whose serials up to and including
    /// `through` are revoked. Unlike [`Bindings::revoked_serials`] this also
    /// covers users a quarantine proved revoked whose issued serials were lost
    /// with the tail. An error when degraded.
    pub fn revoked_ranges(&self) -> Result<Vec<(String, u64)>, BindError> {
        self.refuse_degraded()?;
        let mut v: Vec<_> = self.revoked_through.iter().map(|(u, t)| (u.clone(), *t)).collect();
        v.sort();
        Ok(v)
    }
}
