//! Concierge agent identity loader.
//!
//! Resolves the WeftOS Concierge's persona content (`SOUL.md`,
//! `IDENTITY.md`) for use as the system-prompt foundation.
//!
//! ## Resolution chain (vertical-slice spike — commit 0)
//!
//! 1. Per-instance: `<workspace>/.clawft/SOUL.md`, `<workspace>/.clawft/IDENTITY.md`
//! 2. Fallback templates: `<workspace>/docs/skills/clawft/SOUL.md`,
//!    `<workspace>/docs/skills/clawft/IDENTITY.md`
//!
//! The fallback exists so the spike runs before `weaver init` has been
//! extended to materialize `.clawft/`. Post-spike (commit 1+), the loader
//! drops the fallback and refuses to start without `weaver init`-seeded
//! files.
//!
//! ## What this is NOT (yet)
//!
//! - **No SOUL.journal** — append-only self-observation lands in commit 1.
//! - **No binding-thread integrity check** — hash-pinned excerpt arrives
//!   in commit 1 (governance C8).
//! - **No hot-reload caching strategy** — every call re-reads (small
//!   files; cheap). A cache lands when measurement says it earns its keep.
//!
//! Plan reference: `docs/plans/chat-agent-v1.md` §7.

use std::path::{Path, PathBuf};

use tracing::{debug, warn};

/// Loaded identity content.
#[derive(Debug, Clone)]
pub struct Identity {
    /// `SOUL.md` content — persona, ethical constraints, values.
    pub soul: String,
    /// `IDENTITY.md` content — operational identity, skills, tone.
    pub identity: String,
    /// Short identity descriptor for logs and substrate meta. Spike-only
    /// hash: lengths of `soul` + `identity`. Commit 1 replaces with sha256.
    pub hash: String,
    /// Source of the loaded files — `"clawft"` for `.clawft/` or
    /// `"docs-fallback"` for `docs/skills/clawft/`. Used by the daemon
    /// log to surface when the user hasn't run `weaver init` yet.
    pub source: &'static str,
}

/// Resolves and reads identity content from disk.
pub struct IdentityLoader {
    workspace: PathBuf,
}

impl IdentityLoader {
    /// Build a loader rooted at the given workspace directory. The
    /// workspace is the daemon CWD by default (plan §15.4 — soon
    /// `agent.workspace_root` config key).
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    /// Load the current identity, applying the resolution chain.
    ///
    /// Returns `None` only when neither `.clawft/` nor the
    /// `docs/skills/clawft/` fallback contains both files. Callers
    /// should treat that as a hard failure for the chat path (the
    /// daemon's `agent.chat` handler returns
    /// `agent: identity load failed: ...`).
    pub fn current(&self) -> Option<Identity> {
        if let Some(id) = self.try_load_from(&self.workspace.join(".clawft"), "clawft") {
            return Some(id);
        }
        if let Some(id) = self.try_load_from(
            &self.workspace.join("docs").join("skills").join("clawft"),
            "docs-fallback",
        ) {
            warn!(
                "identity loaded from docs/skills/clawft/ fallback — \
                 run `weaver init` to materialize .clawft/"
            );
            return Some(id);
        }
        None
    }

    fn try_load_from(&self, dir: &Path, source: &'static str) -> Option<Identity> {
        let soul_path = dir.join("SOUL.md");
        let identity_path = dir.join("IDENTITY.md");
        let soul = std::fs::read_to_string(&soul_path).ok()?;
        let identity = std::fs::read_to_string(&identity_path).ok()?;
        debug!(?soul_path, ?identity_path, source, "identity loaded");
        let hash = format!("len:{}/{}", soul.len(), identity.len());
        Some(Identity {
            soul,
            identity,
            hash,
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_from_clawft_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let clawft = tmp.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        std::fs::write(clawft.join("SOUL.md"), "soul content").unwrap();
        std::fs::write(clawft.join("IDENTITY.md"), "identity content").unwrap();

        let loader = IdentityLoader::new(tmp.path());
        let id = loader.current().expect("should load");
        assert_eq!(id.soul, "soul content");
        assert_eq!(id.identity, "identity content");
        assert_eq!(id.source, "clawft");
    }

    #[test]
    fn falls_back_to_docs_when_clawft_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("docs").join("skills").join("clawft");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("SOUL.md"), "doc soul").unwrap();
        std::fs::write(docs.join("IDENTITY.md"), "doc identity").unwrap();

        let loader = IdentityLoader::new(tmp.path());
        let id = loader.current().expect("should load via fallback");
        assert_eq!(id.soul, "doc soul");
        assert_eq!(id.source, "docs-fallback");
    }

    #[test]
    fn returns_none_when_neither_present() {
        let tmp = tempfile::tempdir().unwrap();
        let loader = IdentityLoader::new(tmp.path());
        assert!(loader.current().is_none());
    }

    #[test]
    fn clawft_wins_over_docs_when_both_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let clawft = tmp.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        std::fs::write(clawft.join("SOUL.md"), "runtime soul").unwrap();
        std::fs::write(clawft.join("IDENTITY.md"), "runtime identity").unwrap();
        let docs = tmp.path().join("docs").join("skills").join("clawft");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("SOUL.md"), "doc soul").unwrap();
        std::fs::write(docs.join("IDENTITY.md"), "doc identity").unwrap();

        let loader = IdentityLoader::new(tmp.path());
        let id = loader.current().unwrap();
        assert_eq!(id.soul, "runtime soul");
        assert_eq!(id.source, "clawft");
    }
}
