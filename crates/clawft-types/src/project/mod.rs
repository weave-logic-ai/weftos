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
//! Everything here is pure over injected paths; nothing reads `HOME` or the
//! environment. Callers (the daemon, CLI, resolver) pass `~/.weftos/projects`
//! and `~/.clawft/workspaces.json` in.

mod adopt;
mod error;
mod ids;
mod schema;
mod seed;
mod store;

pub use adopt::adopt_or_init;
pub use error::ProjectError;
pub use ids::{new_id, validate_id};
pub use schema::{
    BinaryInfo, ChainSection, LegacySection, ProjectManifest, ProjectState, ProjectToml,
    ProjectTomlPresence, SCHEMA_VERSION, SeedSection, ServeSection, ServeVia, WeaveSection,
};
pub use seed::{SeedReport, seed_from_registry, seed_from_workspaces};
pub use store::{
    ManifestListing, PROJECT_DIR, PROJECT_TOML, find_by_id, find_by_root, find_project_toml,
    list_manifests, manifest_path, project_toml_path, read_manifest, read_project_toml,
    write_manifest, write_project_toml,
};

#[cfg(test)]
mod tests;
