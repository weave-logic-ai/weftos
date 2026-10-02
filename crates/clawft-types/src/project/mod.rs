//! Project identity model (ADR-103 D4/D5/D6/D10/D13/D14, Phase 1 package B).
//!
//! Two on-disk schemas:
//!
//! - [`ProjectToml`] at `<root>/.weftos/project.toml`: the project's own,
//!   immutable identity (ULID), committed or gitignored at the owner's choice.
//! - [`ProjectManifest`] at `<manifests_dir>/<id>.toml` (normally
//!   `~/.weftos/projects/`): the user-level index entry the daemon and CLI
//!   resolve against.
//!
//! Cross-process manifest locking is unix-only (flock); `project.toml` ids stay
//! consistent everywhere through its exclusive hard-link publish.
//!
//! Everything here is pure over injected paths; nothing reads `HOME` or the
//! environment. Callers (the daemon, CLI, resolver) pass `~/.weftos/projects`
//! and `~/.clawft/workspaces.json` in.

mod adopt;
pub mod canon;
pub mod cert;
mod error;
mod ids;
mod schema;
mod seed;
pub mod spawn;
mod store;

pub use adopt::{adopt_or_init, reinit_fork};
pub use cert::{CertError, CertRequest, ProjectAnchorStmt, ProjectCert};
pub use error::ProjectError;
pub use ids::{new_id, validate_id};
pub use schema::{
    BinaryInfo, ChainSection, ChildState, DEFAULT_IDLE_STOP_SECS, DEFAULT_RESTART_MAX, DEFAULT_RESTART_WINDOW_SECS,
    LegacySection, ProjectManifest, ProjectState, ProjectToml,
    ProjectTomlPresence, SCHEMA_VERSION, SeedSection, ServeSection, ServeVia, WeaveSection,
};
pub use spawn::{SPAWN_TTL_SECS, SpawnError, SpawnFile};
pub use seed::{SeedReport, seed_from_registry, seed_from_workspaces};
pub use store::{
    ManifestListing, PROJECT_DIR, PROJECT_TOML, find_by_id, find_by_root, find_project_toml,
    list_manifests, manifest_path, project_toml_path, read_manifest, read_project_toml,
    write_manifest, write_project_toml,
};

#[cfg(test)]
mod cert_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_identity;
