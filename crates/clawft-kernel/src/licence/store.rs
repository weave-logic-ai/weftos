//! [`CheckoutGrantStore`]: the accepted binding, the held grants and the
//! clock floor (ADR-106 sections 3 and 4). Persisted atomically, 0600,
//! size-capped, and fail-closed: a file that cannot be read or re-verified
//! poisons the store, which then serves nothing and never overwrites it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use super::floor::FloorState;
use super::persist::write_atomic;
use super::store_load::{SlotFile, StoreFile, load};
use super::{
    AdmissionPosture, BindState, BindingExtraCheck, BindingRecord, CheckoutGrant, Clock,
    GRANT_SKEW_SECS, GrantArtifact, LicenceError, LicenceEvent, LicenceEventSink, LocalMeshId,
    MAX_GRANT_SLOTS, NoopSink, Outcome, SignedBinding, SignedEnvelope, SignedGrant,
    verify_binding_member, verify_grant,
};
use crate::revocation::{RevocationKind, RevocationList};
use crate::workload_pkg::TrustAnchors;
use crate::workload_pkg::codec::hex_decode_exact;

/// File name inside the store directory.
pub const GRANTS_FILE: &str = "checkout_grants.json";
/// Persist the clock high-water mark when it moves this far past the saved one.
const HW_PERSIST_STEP: u64 = 60;

#[derive(Clone)]
pub(super) struct Held<T> {
    pub(super) signed: SignedEnvelope,
    pub(super) body: T,
}

/// The grants held for one (cog, version).
#[derive(Default)]
pub(super) struct Slot {
    /// The highest-`seq` accepted grant.
    pub(super) current: Option<Held<CheckoutGrant>>,
    /// The newest accepted grant below `current` (restored on a conflict).
    pub(super) previous: Option<Held<CheckoutGrant>>,
    /// Seqs at which two different payloads were seen; refused for good.
    pub(super) conflicted: Vec<u64>,
}

#[derive(Default)]
pub(super) struct Inner {
    pub(super) binding: Option<Held<BindingRecord>>,
    pub(super) slots: BTreeMap<(String, String), Slot>,
    pub(super) floors: BTreeMap<String, FloorState>,
    pub(super) persisted_hw: u64,
    pub(super) orphan_reported: bool,
    pub(super) poisoned: Option<String>,
}

/// A grant that passed every check and is valid right now. Only the store
/// makes one; [`crate::mesh_artifact::ArtifactExchange::grant_checkout`]
/// takes it.
#[derive(Debug, Clone)]
pub struct VerifiedCheckoutGrant {
    grant: CheckoutGrant,
    grant_pubkey_hex: String,
}

impl VerifiedCheckoutGrant {
    /// A verified grant for the test grant key, without going through a store.
    #[cfg(test)]
    pub(super) fn for_test(grant: CheckoutGrant) -> Self {
        let pk = super::tests_common::pk_hex(&super::tests_common::grant_key());
        Self { grant, grant_pubkey_hex: pk }
    }

    /// The grant.
    pub fn grant(&self) -> &CheckoutGrant {
        &self.grant
    }

    /// The bound grant key, lower-case hex (the signer a key revocation names).
    pub fn grant_pubkey_hex(&self) -> &str {
        &self.grant_pubkey_hex
    }
}

/// Persisted binding and grant state of one node.
pub struct CheckoutGrantStore {
    path: PathBuf,
    anchors: Arc<TrustAnchors>,
    local: LocalMeshId,
    clock: Clock,
    sink: RwLock<Arc<dyn LicenceEventSink>>,
    revocations: RwLock<Option<Arc<RevocationList>>>,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for CheckoutGrantStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckoutGrantStore").finish_non_exhaustive()
    }
}

fn grant_valid(g: &CheckoutGrant, eff_now: u64) -> bool {
    !g.is_withdrawal()
        && eff_now < g.expires_at
        && eff_now < g.licence.expires
        && g.issued_at <= eff_now.saturating_add(GRANT_SKEW_SECS)
}

impl CheckoutGrantStore {
    /// Open the store in `dir`. A missing file is an empty store; a file that
    /// cannot be read, parsed or re-verified is an error (nothing is written).
    pub fn open(
        dir: &Path,
        anchors: Arc<TrustAnchors>,
        local: LocalMeshId,
        clock: Clock,
    ) -> Result<Self, LicenceError> {
        let path = dir.join(GRANTS_FILE);
        let inner = load(&path, &anchors)?;
        Ok(Self::build(path, anchors, local, clock, inner))
    }

    /// [`Self::open`], but a bad file gives a poisoned store (it serves
    /// nothing, refuses writes and leaves the file alone) so the daemon can
    /// still install its policy, which then behaves like `ManifestPolicy`.
    pub fn open_or_poisoned(
        dir: &Path,
        anchors: Arc<TrustAnchors>,
        local: LocalMeshId,
        clock: Clock,
    ) -> Self {
        let path = dir.join(GRANTS_FILE);
        let inner = load(&path, &anchors).unwrap_or_else(|e| Inner {
            poisoned: Some(e.to_string()),
            ..Inner::default()
        });
        Self::build(path, anchors, local, clock, inner)
    }

    fn build(
        path: PathBuf,
        anchors: Arc<TrustAnchors>,
        local: LocalMeshId,
        clock: Clock,
        inner: Inner,
    ) -> Self {
        Self {
            path,
            anchors,
            local,
            clock,
            sink: RwLock::new(Arc::new(NoopSink)),
            revocations: RwLock::new(None),
            inner: Mutex::new(inner),
        }
    }

    /// Report [`LicenceEvent`]s to `sink`.
    pub fn set_sink(&self, sink: Arc<dyn LicenceEventSink>) {
        *self.sink.write().unwrap_or_else(|p| p.into_inner()) = sink;
    }

    /// Use `list` for `SignerKey` and `ArtifactHash` revocations.
    pub fn attach_revocations(&self, list: Arc<RevocationList>) {
        *self.revocations.write().unwrap_or_else(|p| p.into_inner()) = Some(list);
    }

    /// The live local mesh id handle.
    pub fn local_mesh_id(&self) -> &LocalMeshId {
        &self.local
    }

    /// Why the store is poisoned, if it is.
    pub fn poisoned(&self) -> Option<String> {
        self.lock().poisoned.clone()
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn emit(&self, ev: LicenceEvent) {
        self.sink.read().unwrap_or_else(|p| p.into_inner()).emit(ev);
    }

    fn key_revoked(&self, pk_hex: &str) -> bool {
        let g = self.revocations.read().unwrap_or_else(|p| p.into_inner());
        g.as_ref()
            .is_some_and(|l| l.is_subject_revoked(RevocationKind::SignerKey, pk_hex))
    }

    /// True when `blake3_hex` is revoked as an `ArtifactHash`.
    pub fn is_hash_revoked(&self, blake3_hex: &str) -> bool {
        let g = self.revocations.read().unwrap_or_else(|p| p.into_inner());
        g.as_ref()
            .is_some_and(|l| l.is_subject_revoked(RevocationKind::ArtifactHash, blake3_hex))
    }

    fn save(&self, inner: &mut Inner) -> Result<(), LicenceError> {
        let file = StoreFile {
            v: 1,
            binding: inner.binding.as_ref().map(|b| b.signed.clone()),
            grants: inner
                .slots
                .iter()
                .map(|((cog_id, version), s)| SlotFile {
                    cog_id: cog_id.clone(),
                    version: version.clone(),
                    current: s.current.as_ref().map(|h| h.signed.clone()),
                    previous: s.previous.as_ref().map(|h| h.signed.clone()),
                    conflicted: s.conflicted.clone(),
                })
                .collect(),
            floors: inner.floors.clone(),
        };
        let bytes =
            serde_json::to_vec(&file).map_err(|e| LicenceError::Persist(e.to_string()))?;
        write_atomic(&self.path, &bytes)?;
        inner.persisted_hw = inner.floors.values().map(|f| f.hw).max().unwrap_or(0);
        Ok(())
    }

    /// The accepted binding, or why there is none in effect: not bound, bound
    /// to a mesh id this node no longer computes (orphaned: chained once),
    /// no local mesh id, or poisoned.
    pub(super) fn binding_in_effect(&self, inner: &mut Inner) -> Result<BindingRecord, LicenceError> {
        if let Some(p) = &inner.poisoned {
            return Err(LicenceError::Poisoned(p.clone()));
        }
        let local = self.local.get().ok_or(LicenceError::NoLocalMesh)?;
        let b = inner.binding.as_ref().ok_or(LicenceError::NoBinding)?.body.clone();
        if b.mesh_id != local.to_hex() {
            if !inner.orphan_reported {
                inner.orphan_reported = true;
                self.emit(LicenceEvent::BindingOrphaned {
                    stored: b.mesh_id.clone(),
                    local: local.to_hex(),
                });
            }
            return Err(LicenceError::Orphaned);
        }
        inner.orphan_reported = false;
        if b.state != BindState::Bound {
            return Err(LicenceError::Unbound);
        }
        Ok(b)
    }

    /// The time to judge `key`'s grants by: `max(now, floor)`, after
    /// recording the clock (and clamping a far-future mark).
    fn eff_now(&self, inner: &mut Inner, key: &str) -> u64 {
        let now = (self.clock)();
        let fs = inner.floors.entry(key.to_owned()).or_default();
        let clamped = fs.observe(now);
        let (eff, hw) = (fs.effective_now(now), fs.hw);
        if let Some((from, to)) = clamped {
            self.emit(LicenceEvent::FloorClamped { from, to });
            let _ = self.save(inner);
        } else if hw >= inner.persisted_hw.saturating_add(HW_PERSIST_STEP) {
            let _ = self.save(inner);
        }
        eff
    }

    /// The binding in effect, if any.
    pub fn active_binding(&self) -> Option<BindingRecord> {
        self.binding_status().ok()
    }

    /// The binding in effect, or the reason there is none.
    pub fn binding_status(&self) -> Result<BindingRecord, LicenceError> {
        let mut inner = self.lock();
        self.binding_in_effect(&mut inner)
    }

    /// Accept a binding. `posture` is the node's admission state; `extra` is
    /// the steward profile hook (use [`super::NoExtraChecks`] for members).
    pub fn accept_binding(
        &self,
        signed: &SignedBinding,
        posture: AdmissionPosture,
        extra: &dyn BindingExtraCheck,
    ) -> Result<Outcome, LicenceError> {
        let mut inner = self.lock();
        if let Some(p) = &inner.poisoned {
            return Err(LicenceError::Poisoned(p.clone()));
        }
        if let Err(e) = posture.check() {
            self.emit(LicenceEvent::BindingRefused(e.to_string()));
            return Err(e);
        }
        let local = self.local.get().ok_or(LicenceError::NoLocalMesh)?;
        let rec = verify_binding_member(signed, &self.anchors, &local)?;
        extra.check(&rec)?;
        if let Some(cur) = &inner.binding {
            if rec.seq < cur.body.seq {
                return Ok(Outcome::Ignored);
            }
            if rec.seq == cur.body.seq {
                if cur.signed.payload == signed.payload {
                    return Ok(Outcome::Duplicate);
                }
                self.emit(LicenceEvent::BindingConflict(rec.seq));
                return Err(LicenceError::Conflict(rec.seq));
            }
            if cur.body.grant_pubkey != rec.grant_pubkey {
                inner.slots.clear(); // grants under the old key are void
            }
        }
        inner.binding = Some(Held { signed: signed.clone(), body: rec });
        inner.orphan_reported = false;
        self.save(&mut inner)?;
        Ok(Outcome::Applied)
    }

    /// Accept a grant under the bound key (see ADR-106 section 4).
    pub fn accept_grant(&self, signed: &SignedGrant) -> Result<Outcome, LicenceError> {
        let mut inner = self.lock();
        let b = self.binding_in_effect(&mut inner)?;
        let pk = hex_decode_exact::<32>(&b.grant_pubkey)
            .ok_or_else(|| LicenceError::Malformed("bound grant key".into()))?;
        if self.key_revoked(&b.grant_pubkey) {
            return Err(LicenceError::KeyRevoked);
        }
        let local = self.local.get().ok_or(LicenceError::NoLocalMesh)?;
        let g = verify_grant(signed, &pk, &local)?;
        let eff = self.eff_now(&mut inner, &b.grant_pubkey);
        if g.issued_at > eff.saturating_add(GRANT_SKEW_SECS) {
            return Err(LicenceError::NotYetValid);
        }
        let key = (g.cog_id.clone(), g.version.clone());
        if !inner.slots.contains_key(&key) && inner.slots.len() >= MAX_GRANT_SLOTS {
            return Err(LicenceError::Full);
        }
        let held = Held { signed: signed.clone(), body: g };
        let slot = inner.slots.entry(key).or_default();
        let seq = held.body.seq;
        if slot.conflicted.contains(&seq) {
            return Err(LicenceError::Conflict(seq));
        }
        let issued = held.body.issued_at;
        if let Some(cur) = &slot.current {
            if seq < cur.body.seq {
                return Ok(Outcome::Ignored);
            }
            if seq == cur.body.seq {
                if cur.signed.payload == held.signed.payload {
                    return Ok(Outcome::Duplicate);
                }
                // Two payloads at one seq: refuse both, keep the earlier grant.
                slot.conflicted.push(seq);
                slot.current = slot.previous.take();
                self.emit(LicenceEvent::GrantConflict {
                    cog_id: held.body.cog_id,
                    version: held.body.version,
                    seq,
                });
                self.save(&mut inner)?;
                return Err(LicenceError::Conflict(seq));
            }
            check_union(&cur.body, &held.body)?;
        }
        slot.previous = slot.current.take();
        slot.current = Some(held);
        inner.floors.entry(b.grant_pubkey).or_default().note_issued(issued);
        self.save(&mut inner)?;
        Ok(Outcome::Applied)
    }

    /// Run `f` over every currently valid grant (binding in effect, grant key
    /// not revoked, grant unexpired by `max(now, floor)`).
    fn with_valid<T>(&self, mut f: impl FnMut(&CheckoutGrant, &str) -> Option<T>) -> Option<T> {
        let mut inner = self.lock();
        let b = self.binding_in_effect(&mut inner).ok()?;
        if self.key_revoked(&b.grant_pubkey) {
            return None;
        }
        let eff = self.eff_now(&mut inner, &b.grant_pubkey);
        inner
            .slots
            .values()
            .filter_map(|s| s.current.as_ref())
            .filter(|h| h.body.mesh_id == b.mesh_id && grant_valid(&h.body, eff))
            .find_map(|h| f(&h.body, &b.grant_pubkey))
    }

    /// A valid grant that lists BLAKE3 `blake3_hex` for `cog_id` `version`.
    pub fn valid_grant_covering(
        &self,
        blake3_hex: &str,
        cog_id: &str,
        version: &str,
    ) -> Option<CheckoutGrant> {
        self.with_valid(|g, _| {
            (g.cog_id == cog_id && g.version == version && g.artifact_by_blake3(blake3_hex).is_some())
                .then(|| g.clone())
        })
    }

    /// A valid grant for `cog_id` `version` that lists a binary with `sha256`.
    pub fn valid_grant_for_sha256(
        &self,
        cog_id: &str,
        version: &str,
        sha256: &str,
    ) -> Option<(CheckoutGrant, GrantArtifact)> {
        self.with_valid(|g, _| {
            if g.cog_id != cog_id || g.version != version {
                return None;
            }
            g.artifacts.iter().find(|a| a.sha256 == sha256).map(|a| (g.clone(), a.clone()))
        })
    }

    /// Every currently valid grant, ready for `grant_checkout`.
    pub fn verified_grants(&self) -> Vec<VerifiedCheckoutGrant> {
        let mut out = Vec::new();
        self.with_valid::<()>(|g, pk| {
            out.push(VerifiedCheckoutGrant { grant: g.clone(), grant_pubkey_hex: pk.to_owned() });
            None
        });
        out
    }

    /// The floor for the bound key, in unix seconds.
    pub fn floor(&self) -> Option<u64> {
        let mut inner = self.lock();
        let b = self.binding_in_effect(&mut inner).ok()?;
        self.eff_now(&mut inner, &b.grant_pubkey);
        inner.floors.get(&b.grant_pubkey).map(FloorState::floor)
    }

    /// Admin hook (`weaver cog checkout reset-floor`): restart the clock
    /// high-water mark at the current clock. The caller gates and chains it.
    pub fn reset_floor(&self) -> Result<(), LicenceError> {
        let mut inner = self.lock();
        let b = self.binding_in_effect(&mut inner)?;
        let now = (self.clock)();
        inner.floors.entry(b.grant_pubkey).or_default().reset(now);
        self.emit(LicenceEvent::FloorReset(now));
        self.save(&mut inner)
    }

    /// Record the clock now (the daemon calls this on its tick).
    pub fn tick(&self) {
        let mut inner = self.lock();
        if let Ok(b) = self.binding_in_effect(&mut inner) {
            self.eff_now(&mut inner, &b.grant_pubkey);
        }
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
