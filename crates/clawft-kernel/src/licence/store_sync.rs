//! Read accessors the mesh exchange needs from the stores (ADR-106 phase 1b):
//! what is held, for the cheap pre-verify filters, and what to serve in a
//! catch-up sync. Nothing here changes state.

use super::{ApprovalStore, CheckoutGrantStore, SignedApproval, SignedBinding, SignedGrant};

impl CheckoutGrantStore {
    /// The stored binding for the local mesh, whatever its state, with its
    /// `seq`. An unbound record is still returned: its `seq` is what stops an
    /// older bind from being replayed.
    pub fn held_binding(&self) -> Option<(u64, SignedBinding)> {
        let local = self.local_mesh_id().get()?.to_hex();
        let g = self.lock();
        if g.poisoned.is_some() {
            return None;
        }
        let h = g.binding.as_ref().filter(|h| h.body.mesh_id == local)?;
        Some((h.body.seq, h.signed.clone()))
    }

    /// The grant key of the binding in effect, lower-case hex.
    pub fn bound_grant_key(&self) -> Option<String> {
        self.run(|g, ev| self.binding_in_effect(g, ev).ok().map(|b| b.grant_pubkey))
    }

    /// The current grant held for `cog_id` `version`, with its `seq`.
    pub fn held_grant(&self, cog_id: &str, version: &str) -> Option<(u64, SignedGrant)> {
        let g = self.lock();
        let h = g
            .slots
            .get(&(cog_id.to_owned(), version.to_owned()))?
            .current
            .as_ref()?;
        Some((h.body.seq, h.signed.clone()))
    }

    /// The highest-`seq` grant per (cog, version) as `(seq, cog, version,
    /// grant)`, sorted by that tuple. Empty unless a binding is in effect.
    /// Expired and withdrawn grants are included: they are `seq` tombstones.
    pub fn sync_grants(&self) -> Vec<(u64, String, String, SignedGrant)> {
        let mut out: Vec<_> = self.run(|g, ev| {
            if self.binding_in_effect(g, ev).is_err() {
                return Vec::new();
            }
            g.slots
                .iter()
                .filter_map(|((cog, ver), s)| {
                    let h = s.current.as_ref()?;
                    Some((h.body.seq, cog.clone(), ver.clone(), h.signed.clone()))
                })
                .collect()
        });
        out.sort_by(|a, b| (a.0, &a.1, &a.2).cmp(&(b.0, &b.1, &b.2)));
        out
    }
}

impl ApprovalStore {
    /// True when an approval with this content key is held.
    pub fn holds(&self, content_key: &str) -> bool {
        self.lock().0.contains_key(content_key)
    }

    /// Active approvals (for the local mesh) as `(content key, approval)`,
    /// sorted by content key. Empty while poisoned.
    pub fn sync_approvals(&self) -> Vec<(String, SignedApproval)> {
        let Some(local) = self.local.get().map(|m| m.to_hex()) else {
            return Vec::new();
        };
        let g = self.lock();
        if g.1.is_some() {
            return Vec::new();
        }
        g.0.iter()
            .filter(|(_, (_, a))| a.mesh_id == local)
            .map(|(k, (s, _))| (k.clone(), s.clone()))
            .collect()
    }
}
