//! Storage tiers and the capabilities a node advertises for the models it
//! holds (ADR-101 section 3; ADR-099 sections 2 and 6).
//!
//! `model.present` carries a `shards` list holding the BLAKE3 of every shard
//! the node has, plus one `model:<package id>` marker naming the attested
//! manifest, so a requirement can match one exact model with a single
//! `has` predicate. Only a complete, present model is `Available`; a partial
//! holding or a detached store is `Degraded` (not offered to the placer, but
//! visible, and the partial `shards` list still tells the fetch planner what
//! this node can serve). Paths, drive labels and mount points are never
//! advertised.

use std::path::{Path, PathBuf};

use clawft_types::placement::{AttrValue, Capability, CapabilityState, Provenance};

use super::body::MODEL_MARKER_PREFIX;
use super::check::{CheckMode, ModelState};
use super::registry::ModelRegistry;
use crate::node_facts::probe::{cap, str_list};

/// Which kind of storage a model root lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreTier {
    /// The machine's own disk.
    Internal,
    /// A removable or attached drive that can vanish.
    External,
}

/// Classifies a path into a [`StoreTier`] and finds its mount point.
///
/// A path under one of the `external_roots` (for example `/Volumes`) lies on
/// an external drive whose mount point is that root plus one component.
#[derive(Debug, Clone)]
pub struct TierResolver {
    external_roots: Vec<PathBuf>,
}

impl TierResolver {
    /// The conventional external mount parents for this OS.
    pub fn system_default() -> Self {
        let roots: &[&str] = if cfg!(target_os = "macos") {
            &["/Volumes"]
        } else {
            &["/media", "/mnt", "/run/media"]
        };
        Self::with_external_roots(roots.iter().map(PathBuf::from).collect())
    }

    /// Explicit external mount parents (tests use a temp directory).
    pub fn with_external_roots(external_roots: Vec<PathBuf>) -> Self {
        Self { external_roots }
    }

    /// The tier of `path` and, for an external path, its mount point.
    pub fn classify(&self, path: &Path) -> (StoreTier, Option<PathBuf>) {
        for root in &self.external_roots {
            if let Ok(rest) = path.strip_prefix(root)
                && let Some(first) = rest.components().next()
            {
                return (StoreTier::External, Some(root.join(first)));
            }
        }
        (StoreTier::Internal, None)
    }

    /// True when the mount point of an external path is present. `Volumes`
    /// entries disappear when a drive is ejected.
    pub fn mounted(&self, mount: &Path) -> bool {
        mount.is_dir()
    }
}

impl Default for TierResolver {
    fn default() -> Self {
        Self::system_default()
    }
}

/// The capabilities for every adopted model, from existence and size checks
/// only (this never reads weight content, so it is cheap enough for every
/// facts refresh). Also adds `store.tier.external` with `mounted = false`
/// (state `Degraded`) when a model's external drive is detached, so the
/// advertised state changes when the drive vanishes. All are `Probed`.
pub fn model_capabilities(registry: &ModelRegistry, tiers: &TierResolver) -> Vec<Capability> {
    let mut out = Vec::new();
    let mut detached = false;
    for (id, entry) in registry.entries() {
        let Ok(check) = registry.check(&id, CheckMode::Stat) else {
            continue;
        };
        let (tier, mount) = tiers.classify(&entry.root);
        let drive_gone = tier == StoreTier::External
            && mount.as_deref().is_some_and(|m| !tiers.mounted(m));
        detached |= drive_gone;
        let complete = check.present_shards.len() == check.total_shards;
        let state = match (&check.state, complete) {
            (ModelState::Ready, true) if !drive_gone => CapabilityState::Available,
            _ => CapabilityState::Degraded,
        };
        let mut shards: Vec<String> = Vec::new();
        if state == CapabilityState::Available || drive_gone {
            shards.push(format!("{MODEL_MARKER_PREFIX}{id}"));
        }
        if drive_gone && let Ok(body) = entry.body() {
            // Offline, but still the node's attested holding.
            shards.extend(body.shard_hashes().map(String::from));
        } else {
            shards.extend(check.present_shards.iter().cloned());
        }
        if shards.is_empty() {
            continue;
        }
        if let Some(c) = cap("model.present", Provenance::Probed) {
            out.push(c.with_attr("shards", str_list(&shards)).with_state(state));
        }
    }
    if detached
        && let Some(c) = cap("store.tier.external", Provenance::Probed)
    {
        out.push(
            c.with_attr("mounted", AttrValue::Bool(false))
                .with_state(CapabilityState::Degraded),
        );
    }
    out
}
