//! Read accessors the mesh exchange needs from the stores (ADR-106 phase 1b):
//! what is held, for the cheap pre-verify filters, and what to serve in a
//! catch-up sync. Nothing here changes state.

use std::ops::Bound;

use super::{
    ApprovalStore, CheckoutGrantStore, SignedApproval, SignedBinding, SignedEnvelope, SignedGrant,
};

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

    /// A page of the highest-`seq` grant per (cog, version), as `(seq, cog,
    /// version, grant)` ordered by that tuple and strictly after `after`,
    /// within `max_entries` and `max_bytes`; and whether more follow. Empty
    /// unless a binding is in effect. Expired and withdrawn grants are
    /// included: they are `seq` tombstones. Only the keys of the whole set
    /// are sorted; only the page's envelopes are cloned.
    pub fn sync_grants_page(
        &self,
        after: Option<(u64, &str, &str)>,
        max_entries: usize,
        max_bytes: usize,
    ) -> (Vec<(u64, String, String, SignedGrant)>, bool) {
        self.run(|g, ev| {
            if self.binding_in_effect(g, ev).is_err() {
                return (Vec::new(), false);
            }
            let mut keys: Vec<(u64, &String, &String)> = g
                .slots
                .iter()
                .filter_map(|((cog, ver), s)| Some((s.current.as_ref()?.body.seq, cog, ver)))
                .filter(|(seq, cog, ver)| {
                    after.is_none_or(|(s, c, v)| (*seq, cog.as_str(), ver.as_str()) > (s, c, v))
                })
                .collect();
            keys.sort();
            let (mut out, mut bytes) = (Vec::new(), 0usize);
            for (seq, cog, ver) in &keys {
                let h = g.slots[&((*cog).clone(), (*ver).clone())].current.as_ref();
                let Some(h) = h else { continue };
                let n = env_size(&h.signed);
                if out.len() >= max_entries || bytes + n > max_bytes {
                    return (out, true);
                }
                bytes += n;
                out.push((*seq, (*cog).clone(), (*ver).clone(), h.signed.clone()));
            }
            (out, false)
        })
    }
}

/// Wire size of an envelope, for the sync caps.
pub(super) fn env_size(e: &SignedEnvelope) -> usize {
    e.payload.len() + e.public_key.len() + e.signature.len()
}

impl ApprovalStore {
    /// True when an approval with this content key is held.
    pub fn holds(&self, content_key: &str) -> bool {
        self.lock().0.contains_key(content_key)
    }

    /// A page of active approvals (for the local mesh) as `(content key,
    /// approval)` ordered by content key and strictly after `after`, within
    /// `max_entries` and `max_bytes`; and whether more follow. Empty while
    /// poisoned.
    pub fn sync_approvals_page(
        &self,
        after: Option<&str>,
        max_entries: usize,
        max_bytes: usize,
    ) -> (Vec<(String, SignedApproval)>, bool) {
        let Some(local) = self.local.get().map(|m| m.to_hex()) else {
            return (Vec::new(), false);
        };
        let g = self.lock();
        if g.1.is_some() {
            return (Vec::new(), false);
        }
        let from = after.map_or(Bound::Unbounded, |a| Bound::Excluded(a.to_owned()));
        let (mut out, mut bytes) = (Vec::new(), 0usize);
        for (k, (s, a)) in g.0.range((from, Bound::Unbounded)) {
            if a.mesh_id != local {
                continue;
            }
            let n = env_size(s);
            if out.len() >= max_entries || bytes + n > max_bytes {
                return (out, true);
            }
            bytes += n;
            out.push((k.clone(), s.clone()));
        }
        (out, false)
    }
}
