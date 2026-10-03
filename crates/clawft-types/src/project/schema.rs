//! Serde schemas for `project.toml` and the per-project manifest.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Current schema version of both files.
pub const SCHEMA_VERSION: u32 = 1;

fn schema_v1() -> u32 {
    SCHEMA_VERSION
}

/// Accept both quoted RFC 3339 strings and native (unquoted) TOML offset
/// datetimes. Serialization stays a quoted string for stability.
fn de_datetime<'de, D: serde::Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
    use serde::de::{Error, MapAccess, Visitor};

    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = DateTime<Utc>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an RFC 3339 timestamp (quoted or a native TOML datetime)")
        }
        fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
            DateTime::parse_from_rfc3339(v)
                .map(|t| t.with_timezone(&Utc))
                .map_err(E::custom)
        }
        // toml hands native datetimes over as a one-entry map whose value is
        // the RFC 3339 text.
        fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
            let (_k, v): (String, String) = m
                .next_entry()?
                .ok_or_else(|| A::Error::custom("empty datetime"))?;
            self.visit_str(&v)
        }
    }
    d.deserialize_any(V)
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
    #[serde(deserialize_with = "de_datetime")]
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
    /// A per-project kernel supervised by the user daemon (Phase 2).
    ChildKernel,
}

/// Default idle stop for `child-kernel` projects, seconds.
pub const DEFAULT_IDLE_STOP_SECS: u64 = 1800;
/// Default for [`ServeSection::restart_max`].
pub const DEFAULT_RESTART_MAX: u32 = 5;
/// Default for [`ServeSection::restart_window_secs`].
pub const DEFAULT_RESTART_WINDOW_SECS: u64 = 60;

/// State machine of a supervised child kernel (`<run>/<id>/state.json`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChildState {
    #[default]
    Stopped,
    Starting,
    Running,
    IdleStopping,
    Failed,
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
    /// Restarts allowed inside `restart_window_secs` before the child is
    /// marked failed. `None` means [`DEFAULT_RESTART_MAX`]; read it through
    /// [`ServeSection::restart_max`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_max: Option<u32>,
    /// Restart budget window. `None` means [`DEFAULT_RESTART_WINDOW_SECS`];
    /// read it through [`ServeSection::restart_window_secs`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_window_secs: Option<u64>,
    /// How long a child whose last heartbeat said it was busy may stay
    /// silent before the supervisor restarts it, in seconds. `None` uses the
    /// daemon default (10 x the plain grace); it never goes below the plain
    /// grace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_heartbeat_busy_ceiling_secs: Option<u64>,
    /// Kernel version last started for this project (written by the
    /// supervisor, never by the owner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_version: Option<String>,
    /// Kernel binary sha last started for this project (supervisor-written).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_sha: Option<String>,
}

impl ServeSection {
    /// Restart budget count, with the default applied.
    pub fn restart_max(&self) -> u32 {
        self.restart_max.unwrap_or(DEFAULT_RESTART_MAX)
    }

    /// Restart budget window in seconds, with the default applied.
    pub fn restart_window_secs(&self) -> u64 {
        self.restart_window_secs
            .unwrap_or(DEFAULT_RESTART_WINDOW_SECS)
    }

    /// Idle stop in seconds; 0 means never. Absent means
    /// [`DEFAULT_IDLE_STOP_SECS`] for a `child-kernel` project (owner
    /// decision 3) and never otherwise.
    pub fn idle_stop_secs(&self) -> u64 {
        match (self.idle_stop_secs, self.via) {
            (Some(n), _) => n,
            (None, ServeVia::ChildKernel) => DEFAULT_IDLE_STOP_SECS,
            (None, _) => 0,
        }
    }
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
    #[serde(deserialize_with = "de_datetime")]
    pub created: DateTime<Utc>,
    #[serde(deserialize_with = "de_datetime")]
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
