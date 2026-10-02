//! Paths of a per-project child kernel (ADR-103 A6, Phase 2 package A).
//!
//! A child kernel splits its files across two owners:
//!
//! - **user-daemon-owned, ephemeral**, under `~/.weftos/run/<id>/`:
//!   `kernel.sock kernel.pid kernel.lock kernel.log spawn.json
//!   parent-policy.json state.json` (plus the rest of the run dir);
//! - **project-owned, durable** (moves with the project), under
//!   `<project_root>/.weftos/`: `project.key` (0600), `project.cert.json`,
//!   `chain/{chain.json,chain.rvf,chain.tree.json,anchors.jsonl}`,
//!   `state/{workloads.json,apps.json}` and the committed `overlay.toml`.
//!
//! The node key and the chain signing key are the same file, `project.key`
//! (one project key). The project walk-up in [`resolve_root`] never yields a
//! child; the only way to get one is [`RuntimePaths::child_at`] /
//! [`RuntimePaths::child_with`].
//!
//! [`resolve_root`]: super::resolve_root

use std::path::{Path, PathBuf};

use super::{RootSource, RuntimePaths, user_runtime_root};

/// Spawn handshake written by the user daemon (0600, 60 s expiry).
pub const SPAWN_JSON_FILE: &str = "spawn.json";
/// Signed parent policy written by the user daemon.
pub const PARENT_POLICY_FILE: &str = "parent-policy.json";
/// Supervisor state machine snapshot.
pub const STATE_JSON_FILE: &str = "state.json";
/// The one project key (node key, chain key, anchor and PoP key).
pub const PROJECT_KEY_FILE: &str = "project.key";
/// The project certificate mirror inside the project.
pub const PROJECT_CERT_FILE: &str = "project.cert.json";
/// The committed overlay (only the owner writes it).
pub const OVERLAY_FILE: &str = "overlay.toml";

/// True when `id` is usable as a single path component: non-empty, ASCII
/// alphanumeric only (a ULID qualifies). Rejects separators and `.`.
fn safe_component(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl RuntimePaths {
    /// A child kernel's paths with an explicit run dir (what the child sees
    /// as `$WEFTOS_RUNTIME_DIR`). `None` when `id` is not a safe path
    /// component.
    pub fn child_at(
        run_dir: impl Into<PathBuf>,
        id: &str,
        project_root: impl Into<PathBuf>,
    ) -> Option<Self> {
        safe_component(id).then(|| Self {
            root: run_dir.into(),
            source: RootSource::Child {
                id: id.to_owned(),
                project_root: project_root.into(),
            },
        })
    }

    /// A child kernel's paths under `<home>/.weftos/run/<id>/`. `None` when
    /// `id` is not a safe path component.
    pub fn child_with(home: &Path, id: &str, project_root: impl Into<PathBuf>) -> Option<Self> {
        Self::child_at(user_runtime_root(home).join(id), id, project_root)
    }

    /// `<project_root>/.weftos` when this is a child; `None` otherwise.
    pub(super) fn child_weftos_dir(&self) -> Option<PathBuf> {
        match &self.source {
            RootSource::Child { project_root, .. } => Some(project_root.join(".weftos")),
            _ => None,
        }
    }

    /// The child's project id, when this is a child.
    pub fn child_id(&self) -> Option<&str> {
        match &self.source {
            RootSource::Child { id, .. } => Some(id),
            _ => None,
        }
    }

    /// `<run>/spawn.json`.
    pub fn spawn_json(&self) -> PathBuf {
        self.file(SPAWN_JSON_FILE)
    }
    /// `<run>/parent-policy.json`.
    pub fn parent_policy(&self) -> PathBuf {
        self.file(PARENT_POLICY_FILE)
    }
    /// `<run>/state.json`.
    pub fn state_json(&self) -> PathBuf {
        self.file(STATE_JSON_FILE)
    }
    /// `<root>/.weftos/project.key` (children only).
    pub fn project_key(&self) -> Option<PathBuf> {
        self.child_weftos_dir().map(|d| d.join(PROJECT_KEY_FILE))
    }
    /// `<root>/.weftos/project.cert.json` (children only).
    pub fn project_cert(&self) -> Option<PathBuf> {
        self.child_weftos_dir().map(|d| d.join(PROJECT_CERT_FILE))
    }
    /// `<root>/.weftos/overlay.toml` (children only).
    pub fn overlay(&self) -> Option<PathBuf> {
        self.child_weftos_dir().map(|d| d.join(OVERLAY_FILE))
    }
    /// `<root>/.weftos/state/` (children only).
    pub fn state_dir(&self) -> Option<PathBuf> {
        self.child_weftos_dir().map(|d| d.join("state"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";

    fn child() -> RuntimePaths {
        RuntimePaths::child_with(Path::new("/h"), ID, "/work/p").unwrap()
    }

    #[test]
    fn layout_table() {
        let p = child();
        let run = PathBuf::from(format!("/h/.weftos/run/{ID}"));
        assert_eq!(p.root(), run);
        assert_eq!(p.child_id(), Some(ID));
        // Ephemeral, user-daemon-owned: the run dir.
        assert_eq!(p.socket(), run.join("kernel.sock"));
        assert_eq!(p.pid(), run.join("kernel.pid"));
        assert_eq!(p.lock(), run.join("kernel.lock"));
        assert_eq!(p.log(), run.join("kernel.log"));
        assert_eq!(p.spawn_json(), run.join("spawn.json"));
        assert_eq!(p.parent_policy(), run.join("parent-policy.json"));
        assert_eq!(p.state_json(), run.join("state.json"));
        // Durable, project-owned.
        let w = PathBuf::from("/work/p/.weftos");
        assert_eq!(p.project_key(), Some(w.join("project.key")));
        assert_eq!(p.project_cert(), Some(w.join("project.cert.json")));
        assert_eq!(p.overlay(), Some(w.join("overlay.toml")));
        assert_eq!(p.state_dir(), Some(w.join("state")));
        assert_eq!(p.chain_checkpoint(), w.join("chain/chain.json"));
        assert_eq!(p.chain_rvf(), w.join("chain/chain.rvf"));
        assert_eq!(p.chain_tree(), w.join("chain/chain.tree.json"));
        assert_eq!(p.chain_dir(), w.join("chain"));
        assert_eq!(p.anchors_ledger(), w.join("chain/anchors.jsonl"));
        assert_eq!(p.workloads(), w.join("state/workloads.json"));
        assert_eq!(p.apps(), w.join("state/apps.json"));
    }

    #[test]
    fn one_project_key_is_node_and_chain_key() {
        let p = child();
        assert_eq!(p.node_key(), p.chain_key());
        assert_eq!(Some(p.node_key()), p.project_key());
    }

    #[test]
    fn non_child_roots_keep_their_layout_and_have_no_project_files() {
        let p = RuntimePaths::at("/run/probe");
        assert_eq!(p.node_key(), PathBuf::from("/run/probe/node.key"));
        assert_eq!(p.chain_key(), PathBuf::from("/run/probe/chain.key"));
        assert_eq!(p.workloads(), PathBuf::from("/run/probe/workloads.json"));
        assert_eq!(p.child_id(), None);
        assert_eq!(p.project_key(), None);
        assert_eq!(p.overlay(), None);
    }

    #[test]
    fn walk_up_never_selects_a_child() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let root = t.path().join("proj");
        std::fs::create_dir_all(root.join(".weftos")).unwrap();
        std::fs::write(root.join(".weftos/project.toml"), "").unwrap();
        let run = home.join(".weftos/run").join(ID);
        std::fs::create_dir_all(&run).unwrap();
        for cwd in [&root, &run, &home] {
            let p = RuntimePaths::resolve_with(None, Some(cwd), Some(&home));
            assert!(!matches!(p.source(), RootSource::Child { .. }), "{cwd:?}");
        }
        let p = RuntimePaths::resolve_with(None, Some(&root), Some(&home));
        assert!(matches!(p.source(), RootSource::Project(_)));
    }

    #[test]
    fn unsafe_ids_are_refused() {
        for bad in ["", "..", "a/b", "a\\b", "x.y", "../etc"] {
            assert!(
                RuntimePaths::child_with(Path::new("/h"), bad, "/p").is_none(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn child_at_honours_explicit_run_dir() {
        let p = RuntimePaths::child_at("/iso/run", ID, "/p").unwrap();
        assert_eq!(p.socket(), PathBuf::from("/iso/run/kernel.sock"));
        assert_eq!(p.chain_dir(), PathBuf::from("/p/.weftos/chain"));
    }
}
