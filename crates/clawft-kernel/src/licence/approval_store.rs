//! [`ApprovalStore`]: the operator hash approvals this node holds.
//!
//! Additive and content-addressed (no `seq`): a duplicate is idempotent and a
//! new approval never replaces an older one. Persisted atomically, 0600,
//! size-capped, fail-closed. An approval whose `mesh_id` is not the local one
//! (a `mesh_nonce` change) stays on disk but is inactive and listed by
//! [`ApprovalStore::orphaned`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::approval::{Approval, SignedApproval, verify_approval, verify_approval_signature};
use super::persist::{read_capped, write_atomic};
use super::{LicenceError, LocalMeshId, MAX_APPROVALS, Outcome};
use crate::workload_pkg::TrustAnchors;

/// File name inside the store directory.
pub const APPROVALS_FILE: &str = "checkout_approvals.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalsFile {
    v: u32,
    approvals: Vec<SignedApproval>,
}

/// Held approvals by content key, with the verified body.
pub(super) type Held = BTreeMap<String, (SignedApproval, Approval)>;

/// Persisted operator hash approvals of one node.
pub struct ApprovalStore {
    path: PathBuf,
    anchors: Arc<TrustAnchors>,
    pub(super) local: LocalMeshId,
    inner: Mutex<(Held, Option<String>)>,
}

impl ApprovalStore {
    /// Open the store in `dir`. A missing file is an empty store; a bad file
    /// is an error and nothing is written.
    pub fn open(
        dir: &Path,
        anchors: Arc<TrustAnchors>,
        local: LocalMeshId,
    ) -> Result<Self, LicenceError> {
        let path = dir.join(APPROVALS_FILE);
        let held = load(&path, &anchors)?;
        Ok(Self { path, anchors, local, inner: Mutex::new((held, None)) })
    }

    /// [`Self::open`], but a bad file gives a poisoned store: it has no
    /// active approvals, refuses writes and leaves the file alone.
    pub fn open_or_poisoned(dir: &Path, anchors: Arc<TrustAnchors>, local: LocalMeshId) -> Self {
        let path = dir.join(APPROVALS_FILE);
        let (held, poison) = match load(&path, &anchors) {
            Ok(h) => (h, None),
            Err(e) => (Held::new(), Some(e.to_string())),
        };
        Self { path, anchors, local, inner: Mutex::new((held, poison)) }
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, (Held, Option<String>)> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Why the store is poisoned, if it is.
    pub fn poisoned(&self) -> Option<String> {
        self.lock().1.clone()
    }

    /// Accept an approval: pinned operator signature, `approval.mesh_id`
    /// equal to the local mesh id. A repeat is [`Outcome::Duplicate`].
    pub fn accept(&self, signed: &SignedApproval) -> Result<Outcome, LicenceError> {
        let mut g = self.lock();
        if let Some(p) = &g.1 {
            return Err(LicenceError::Poisoned(p.clone()));
        }
        let local = self.local.get().ok_or(LicenceError::NoLocalMesh)?;
        let approval = verify_approval(signed, &self.anchors, &local)?;
        let key = approval.content_key();
        if g.0.contains_key(&key) {
            return Ok(Outcome::Duplicate);
        }
        if g.0.len() >= MAX_APPROVALS {
            return Err(LicenceError::Full);
        }
        g.0.insert(key, (signed.clone(), approval));
        save(&self.path, &g.0)?;
        Ok(Outcome::Applied)
    }

    /// The content key of an active approval for `cog_id` `version` that
    /// lists `sha256`.
    pub fn covering(&self, cog_id: &str, version: &str, sha256: &str) -> Option<String> {
        let g = self.lock();
        let local = self.local.get()?.to_hex();
        if g.1.is_some() {
            return None;
        }
        g.0.iter()
            .find(|(_, (_, a))| {
                a.mesh_id == local && a.cog_id == cog_id && a.version == version && a.covers(sha256)
            })
            .map(|(k, _)| k.clone())
    }

    /// Content keys of approvals for another mesh id (`weaver doctor`).
    pub fn orphaned(&self) -> Vec<String> {
        let g = self.lock();
        let local = self.local.get().map(|m| m.to_hex());
        g.0.iter()
            .filter(|(_, (_, a))| Some(&a.mesh_id) != local.as_ref())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Number of approvals held, active or not.
    pub fn len(&self) -> usize {
        self.lock().0.len()
    }

    /// True when no approval is held.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn save(path: &Path, held: &Held) -> Result<(), LicenceError> {
    let file = ApprovalsFile { v: 1, approvals: held.values().map(|(s, _)| s.clone()).collect() };
    let bytes = serde_json::to_vec(&file).map_err(|e| LicenceError::Persist(e.to_string()))?;
    write_atomic(path, &bytes)
}

fn load(path: &Path, anchors: &TrustAnchors) -> Result<Held, LicenceError> {
    let Some(bytes) = read_capped(path)? else {
        return Ok(Held::new());
    };
    let file: ApprovalsFile =
        serde_json::from_slice(&bytes).map_err(|e| LicenceError::Malformed(e.to_string()))?;
    if file.v != 1 || file.approvals.len() > MAX_APPROVALS {
        return Err(LicenceError::Malformed("approvals version or size".into()));
    }
    let mut held = Held::new();
    for s in file.approvals {
        let a = verify_approval_signature(&s, anchors)?;
        held.insert(a.content_key(), (s, a));
    }
    Ok(held)
}
