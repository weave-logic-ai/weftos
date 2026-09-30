//! Serde schemas for `project.toml` and the per-project manifest.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Current schema version of both files.
pub const SCHEMA_VERSION: u32 = 1;

fn schema_v1() -> u32 {
    SCHEMA_VERSION
}

/// `[weave]` section of `project.toml` (ADR-103 D10).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeaveSection {
    /// This project is the weave master for its mesh. Default false.
    #[serde(default)]
    pub master: bool,
}

/// `<root>/.weftos/project.toml`: the project's immutable identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectToml {
    /// Schema version (1).
    #[serde(rename = "schema", default = "schema_v1")]
    pub schema_version: u32,
    /// ULID, immutable once written.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// When the identity was minted.
    pub created: DateTime<Utc>,
    /// Optional parent project ULID (nesting, D10); unused in Phase 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Optional governance overlay reference (path or id), resolved by the
    /// governance layer, opaque here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governance: Option<String>,
    /// Optional `[weave]` table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weave: Option<WeaveSection>,
    /// Unknown keys, preserved on rewrite.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl ProjectToml {
    /// True when `[weave] master = true`.
    pub fn is_weave_master(&self) -> bool {
        self.weave.as_ref().is_some_and(|w| w.master)
    }
}

/// Lifecycle state of a manifest entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectState {
    #[default]
    Active,
    /// Root no longer exists on disk. Never deleted automatically.
    Missing,
    Archived,
}

/// Whether the tree already holds a `project.toml`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectTomlPresence {
    #[default]
    Present,
    /// Seeded from the legacy registry; not yet written into the tree.
    Pending,
}

/// `[seed]`: where the entry came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedSection {
    /// `workspaces.json` or `project-init`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_name: Option<String>,
}

/// `[legacy]`: observation only, never acted on in Phase 1.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacySection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_dir: Option<PathBuf>,
}

/// How a project is served.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServeVia {
    #[default]
    UserDaemon,
    /// Phase 2.
    ChildKernel,
}

/// `[serve]`: read by the resolver (D14).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServeSection {
    #[serde(default)]
    pub via: ServeVia,
    /// Override pointing weft at a legacy project-local daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_dir: Option<PathBuf>,
    /// Stop a project kernel after this many idle seconds (Phase 2 consumer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_stop_secs: Option<u64>,
}

/// `[chain]`: chain location (D6). Absent means the in-tree default
/// `<root>/.weftos/chain/`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
}

/// `[binary]`: the kernel that last ran the project (update / doctor).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryInfo {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub version: String,
}

/// `~/.weftos/projects/<id>.toml`: one file per project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectManifest {
    #[serde(rename = "schema", default = "schema_v1")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    /// Canonicalised project root.
    pub root: PathBuf,
    #[serde(default)]
    pub state: ProjectState,
    pub created: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    #[serde(default)]
    pub project_toml: ProjectTomlPresence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<SeedSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy: Option<LegacySection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serve: Option<ServeSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<BinaryInfo>,
    /// Unknown keys, preserved on rewrite.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl ProjectManifest {
    /// Runtime dir override from `[serve]`, if any.
    pub fn runtime_dir_override(&self) -> Option<&std::path::Path> {
        self.serve.as_ref().and_then(|s| s.runtime_dir.as_deref())
    }

    /// In-tree chain dir per D6 unless `[chain] dir` overrides it.
    pub fn chain_dir(&self) -> PathBuf {
        self.chain
            .as_ref()
            .and_then(|c| c.dir.clone())
            .unwrap_or_else(|| self.root.join(".weftos").join("chain"))
    }
}
