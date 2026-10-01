//! Workload-kind registry (cog-boundary audit M3; ADR-099 section 2).
//!
//! A workload kind owns the pieces of the placement path that depend on
//! what is being placed: manifest body validation, how a verified package
//! becomes a [`WorkloadSpec`], and which adapter routes can run it. The
//! rest of the path (signature checks, seeding, gating, scoring, dispatch)
//! is kind-agnostic and looks the kind up here instead of assuming `cog`.
//!
//! Registration is explicit: [`KindRegistry::builtin`] builds a registry
//! holding the in-tree kinds (`cog`), and callers add more with
//! [`KindRegistry::register`]. There is no global mutable state.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use clawft_types::placement::engine::WorkloadSpec;

#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use crate::workload_ctl::PlaneError;
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use crate::workload_pkg::TrustAnchors;
use crate::workload_pkg::{ManifestEnvelope, ManifestError};
#[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
use crate::workload_runtime::VerifiedWorkload;

mod cog;

#[cfg(test)]
mod tests;

pub use cog::CogKind;

/// No kind is registered under this id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown workload kind {0:?}")]
pub struct UnknownKind(pub String);

/// A kind id was registered twice.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("workload kind {0:?} is already registered")]
pub struct DuplicateKind(pub String);

/// What a workload kind contributes to validation and placement.
pub trait WorkloadKind: Send + Sync {
    /// The manifest `kind` string this implementation handles.
    fn id(&self) -> &'static str;

    /// Validate the envelope's kind-specific body (shape and limits; not
    /// signatures or file contents, which are kind-agnostic).
    fn validate(&self, envelope: &ManifestEnvelope) -> Result<(), ManifestError>;

    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    /// Verify the signed package in `package_dir` against `anchors` and
    /// load it as a runnable workload.
    fn load(
        &self,
        package_dir: &Path,
        anchors: &TrustAnchors,
    ) -> Result<VerifiedWorkload, PlaneError>;

    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    /// The placement requirements for a loaded workload of this kind.
    fn spec(&self, workload: &VerifiedWorkload) -> Result<WorkloadSpec, String>;

    #[cfg(all(feature = "workload-runtime", feature = "mesh", unix))]
    /// Adapter route names that can run this kind (`native`, `container`,
    /// `emulated`, `remote.api`, ...).
    fn adapters(&self) -> &'static [&'static str];
}

/// Kinds by id.
#[derive(Clone, Default)]
pub struct KindRegistry {
    kinds: BTreeMap<&'static str, Arc<dyn WorkloadKind>>,
}

impl KindRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// The in-tree kinds: `cog`.
    pub fn builtin() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(CogKind))
            .expect("builtin kind ids are distinct");
        r
    }

    /// Add a kind; refuses a second registration of the same id.
    pub fn register(&mut self, kind: Arc<dyn WorkloadKind>) -> Result<(), DuplicateKind> {
        let id = kind.id();
        if self.kinds.contains_key(id) {
            return Err(DuplicateKind(id.to_string()));
        }
        self.kinds.insert(id, kind);
        Ok(())
    }

    /// The kind registered under `id`.
    pub fn get(&self, id: &str) -> Option<&Arc<dyn WorkloadKind>> {
        self.kinds.get(id)
    }

    /// Like [`get`](Self::get) with a structured error for an unknown id.
    pub fn require(&self, id: &str) -> Result<&Arc<dyn WorkloadKind>, UnknownKind> {
        self.get(id).ok_or_else(|| UnknownKind(id.to_string()))
    }

    /// Registered ids in order.
    pub fn ids(&self) -> Vec<&'static str> {
        self.kinds.keys().copied().collect()
    }
}

impl std::fmt::Debug for KindRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.kinds.keys()).finish()
    }
}

/// Validate an envelope through the registry: the kind must be
/// registered and its body must pass that kind's validation.
pub fn validate_envelope(
    registry: &KindRegistry,
    envelope: &ManifestEnvelope,
) -> Result<(), ManifestError> {
    registry.require(&envelope.kind)?.validate(envelope)
}

/// The `kind` a package directory's manifest declares, if the manifest is
/// readable and parses far enough to say. `None` leaves the failure to
/// package verification, which reports it precisely.
pub fn peek_manifest_kind(package_dir: &Path) -> Option<String> {
    use crate::workload_pkg::{MANIFEST_FILE, MAX_MANIFEST_BYTES};
    let path = package_dir.join(MANIFEST_FILE);
    if std::fs::metadata(&path).ok()?.len() > MAX_MANIFEST_BYTES as u64 {
        return None;
    }
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    doc.get("kind")?.as_str().map(str::to_string)
}
