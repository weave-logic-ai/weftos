//! Concierge agent identity loader.
//!
//! Resolves the WeftOS Concierge's persona content (`SOUL.md`,
//! `IDENTITY.md`) for use as the system-prompt foundation.
//!
//! ## Resolution
//!
//! Per-instance only: `<workspace>/.clawft/SOUL.md` and
//! `<workspace>/.clawft/IDENTITY.md`. The directory is materialized
//! by `weaver init` (Phase F1), so every initialized workspace boots
//! with both files in place. If they're missing the loader returns
//! [`IdentityError::NotFound`] and the daemon's chat path surfaces
//! `agent: identity load failed: ...`.
//!
//! Phase F1 deleted the previous `docs/skills/clawft/` fallback the
//! spike used while `weaver init` did not yet seed `.clawft/`.
//!
//! ## What this module DOES (Phase D1 + F1, agent-core-v1)
//!
//! - SHA-256 (hex) hash of `SOUL.md + "\n" + IDENTITY.md` as the
//!   identity descriptor surfaced in logs and the system prompt.
//! - [`IdentityProvider`] async trait so [`AgentLoop`] is testable
//!   without filesystem IO. [`FileIdentityProvider`] caches the most
//!   recent successful load.
//! - [`BINDING_THREAD_EXCERPT`] compile-time constant that the
//!   `SystemPromptBuilder` checks against the loaded SOUL.md content.
//!   Mismatch downgrades the prompt to a "binding-thread mismatch"
//!   annotation but does not refuse to run.
//!
//! ## What this is NOT (yet)
//!
//! - **No SOUL.journal write path** — F1 seeds the empty journal
//!   file and stamps the `soul_journal` derived-write grant; F2's
//!   `weaver soul promote` reads it, diffs, and applies on
//!   confirmation. The journal is not consulted on every-turn loads.
//! - **No hot-reload watcher** — the cached `FileIdentityProvider`
//!   re-reads on every call (small files; cheap). A `notify`-driven
//!   watcher arrives when measurement says it earns its keep.
//!
//! Plan reference: `docs/plans/agent-core-v1.md` Phase D1, F1.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Distinctive paragraph from the canonical `SOUL.md` used as the
/// compile-time witness for the binding-thread integrity check.
///
/// The check is operated by
/// [`SystemPromptBuilder`](crate::agent::system_prompt::SystemPromptBuilder):
/// if the loaded `SOUL.md` does not contain this excerpt, the prompt
/// is annotated `binding-thread-status: mismatch` and a `warn!` log is
/// emitted, but the agent still runs. Hard refusal is a v1.1 follow-up.
///
/// Source: `docs/skills/clawft/SOUL.md` §"Core Personality Traits" /
/// "The Binding Thread" — quoted verbatim so the substring search is
/// stable across whitespace-only edits to the surrounding paragraph.
pub const BINDING_THREAD_EXCERPT: &str =
    "an agent must not diminish human capability, or by inaction allow it to be diminished";

/// Loaded identity content.
#[derive(Debug, Clone)]
pub struct Identity {
    /// `SOUL.md` content — persona, ethical constraints, values.
    pub soul: String,
    /// `IDENTITY.md` content — operational identity, skills, tone.
    pub identity: String,
    /// SHA-256 (hex, lowercase) of `soul + "\n" + identity`. Surfaced
    /// in logs and as the trailing `[hash]` line of the system prompt.
    /// Phase D1 replaced the spike's `len(soul)+len(identity)`
    /// placeholder.
    pub hash: String,
    /// Source of the loaded files. Always `"clawft"` after F1 (the
    /// `docs/skills/clawft/` fallback was removed). The field is
    /// retained as a `&'static str` so a future substrate-backed
    /// provider can introduce new variants without touching callers.
    pub source: &'static str,
}

/// Errors emitted by the identity load path.
///
/// Today only signals the "files missing" case; in future a
/// substrate-backed loader will need to distinguish IO from
/// deserialization errors. Variants stay shaped for forward
/// compatibility.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// `<workspace>/.clawft/SOUL.md` or `IDENTITY.md` (or both) are
    /// missing. Callers treat this as a hard failure for the chat
    /// path. Run `weaver init` to materialize the seed files.
    #[error(
        "identity load failed: <workspace>/.clawft/{{SOUL.md,IDENTITY.md}} missing — run `weaver init`"
    )]
    NotFound,
}

/// Async interface for retrieving the agent's current identity.
///
/// Decouples `loop_core` and `SystemPromptBuilder` from the on-disk
/// loader so they can be exercised against in-memory fixtures. The
/// substrate-backed identity provider (Phase F1) will plug in here
/// without any caller-site changes.
#[async_trait]
pub trait IdentityProvider: Send + Sync + 'static {
    /// Return the current identity. Called once per turn; impls
    /// should be cheap (cached IO).
    async fn current(&self) -> Result<Identity, IdentityError>;
}

/// Filesystem-backed [`IdentityProvider`] that re-reads on every call
/// and caches the most recent successful load.
///
/// The cache lets repeated calls within a turn skip the disk hit;
/// cross-turn changes (the user editing `SOUL.md` between turns) are
/// picked up on the next call because the loader still tries the disk
/// first. The cache is only consulted as a fallback when both the
/// per-instance and fallback paths fail to resolve.
pub struct FileIdentityProvider {
    workspace: PathBuf,
    cached: RwLock<Option<Identity>>,
}

impl FileIdentityProvider {
    /// Build a provider rooted at the given workspace directory.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            cached: RwLock::new(None),
        }
    }

    /// Return a reference to the workspace root.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
}

#[async_trait]
impl IdentityProvider for FileIdentityProvider {
    async fn current(&self) -> Result<Identity, IdentityError> {
        let loader = IdentityLoader::new(self.workspace.clone());
        match loader.current() {
            Some(id) => {
                let mut cache = self.cached.write().await;
                *cache = Some(id.clone());
                Ok(id)
            }
            None => {
                // Disk read failed — surface the cached value if we
                // ever loaded one, otherwise propagate the error so
                // the daemon's chat path returns the "identity load
                // failed" RPC error.
                if let Some(cached) = self.cached.read().await.clone() {
                    warn!(
                        "identity provider: disk re-read failed; \
                         serving cached load (hash={})",
                        cached.hash
                    );
                    return Ok(cached);
                }
                Err(IdentityError::NotFound)
            }
        }
    }
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

    /// Load the current identity from `<workspace>/.clawft/`.
    ///
    /// Returns `None` when either `SOUL.md` or `IDENTITY.md` is
    /// absent. Callers treat that as a hard failure for the chat path
    /// (the daemon's `agent.chat` handler returns
    /// `agent: identity load failed: ...`).
    ///
    /// F1 removed the `docs/skills/clawft/` fallback the spike used
    /// while `weaver init` did not yet seed `.clawft/`. Every
    /// initialized workspace now boots with the seed files in place.
    pub fn current(&self) -> Option<Identity> {
        self.try_load_from(&self.workspace.join(".clawft"), "clawft")
    }

    fn try_load_from(&self, dir: &Path, source: &'static str) -> Option<Identity> {
        let soul_path = dir.join("SOUL.md");
        let identity_path = dir.join("IDENTITY.md");
        let soul = std::fs::read_to_string(&soul_path).ok()?;
        let identity = std::fs::read_to_string(&identity_path).ok()?;
        debug!(?soul_path, ?identity_path, source, "identity loaded");
        let hash = sha256_identity_hash(&soul, &identity);
        Some(Identity {
            soul,
            identity,
            hash,
            source,
        })
    }
}

/// Compute the SHA-256 (hex, lowercase) of `soul + "\n" + identity`.
///
/// Centralised so tests and the future substrate-backed identity
/// provider produce the exact same descriptor as the on-disk loader.
pub fn sha256_identity_hash(soul: &str, identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(soul.as_bytes());
    hasher.update(b"\n");
    hasher.update(identity.as_bytes());
    let digest = hasher.finalize();
    hex_lower(&digest)
}

/// Render a byte slice as a lowercase hex string. Avoids pulling
/// `hex` as a new dep.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_thread_excerpt_is_non_empty() {
        assert!(!BINDING_THREAD_EXCERPT.is_empty());
        assert!(BINDING_THREAD_EXCERPT.len() > 16);
    }

    #[test]
    fn sha256_hash_matches_known_vector() {
        // Reference: printf 'hello\nworld' | sha256sum
        //   26c60a61d01db5836ca70fefd44a6a016620413c8ef5f259a6c5612d4f79d3b8
        // Composition is `soul + "\n" + identity` so passing
        // soul="hello", identity="world" reproduces the canonical
        // "hello\nworld" digest.
        let h = sha256_identity_hash("hello", "world");
        assert_eq!(
            h,
            "26c60a61d01db5836ca70fefd44a6a016620413c8ef5f259a6c5612d4f79d3b8"
        );
        assert_eq!(h.len(), 64); // SHA-256 hex is 64 chars
        // Hash is deterministic — repeated calls return the same value.
        assert_eq!(h, sha256_identity_hash("hello", "world"));
        // Distinct inputs produce distinct hashes.
        assert_ne!(h, sha256_identity_hash("hello", "WORLD"));
    }

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
        // Hash must be SHA-256 hex of `"soul content" + "\n" + "identity content"`.
        assert_eq!(
            id.hash,
            sha256_identity_hash("soul content", "identity content")
        );
        assert_eq!(id.hash.len(), 64);
    }

    #[test]
    fn does_not_load_from_docs_skills_anymore() {
        // F1 removed the docs/skills/clawft/ fallback. Even when only
        // the docs path is populated, the loader returns None — the
        // chat path must emit `identity load failed: ... run `weaver
        // init`` rather than silently substituting bundled docs.
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("docs").join("skills").join("clawft");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("SOUL.md"), "doc soul").unwrap();
        std::fs::write(docs.join("IDENTITY.md"), "doc identity").unwrap();

        let loader = IdentityLoader::new(tmp.path());
        assert!(
            loader.current().is_none(),
            "post-F1: docs/skills/clawft/ must not satisfy the loader"
        );
    }

    #[test]
    fn returns_none_when_clawft_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let loader = IdentityLoader::new(tmp.path());
        assert!(loader.current().is_none());
    }

    #[test]
    fn returns_none_when_only_one_file_present() {
        // Half-populated .clawft/ should fail loud, not partial-load.
        let tmp = tempfile::tempdir().unwrap();
        let clawft = tmp.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        std::fs::write(clawft.join("SOUL.md"), "soul only").unwrap();

        let loader = IdentityLoader::new(tmp.path());
        assert!(loader.current().is_none());
    }

    // ── FileIdentityProvider tests ────────────────────────────────

    #[tokio::test]
    async fn file_provider_loads_and_caches() {
        let tmp = tempfile::tempdir().unwrap();
        let clawft = tmp.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        std::fs::write(clawft.join("SOUL.md"), "soul-1").unwrap();
        std::fs::write(clawft.join("IDENTITY.md"), "id-1").unwrap();

        let provider = FileIdentityProvider::new(tmp.path());
        let first = provider.current().await.expect("first load");
        assert_eq!(first.soul, "soul-1");

        // Mutate the files between calls — provider must observe the
        // change because every call re-reads from disk.
        std::fs::write(clawft.join("SOUL.md"), "soul-2").unwrap();
        let second = provider.current().await.expect("second load");
        assert_eq!(second.soul, "soul-2");
        assert_ne!(first.hash, second.hash);
    }

    #[tokio::test]
    async fn file_provider_serves_cache_when_disk_disappears() {
        let tmp = tempfile::tempdir().unwrap();
        let clawft = tmp.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        std::fs::write(clawft.join("SOUL.md"), "cached-soul").unwrap();
        std::fs::write(clawft.join("IDENTITY.md"), "cached-id").unwrap();

        let provider = FileIdentityProvider::new(tmp.path());
        let first = provider.current().await.expect("warm cache");

        // Remove the files; the cache should still resolve.
        std::fs::remove_file(clawft.join("SOUL.md")).unwrap();
        std::fs::remove_file(clawft.join("IDENTITY.md")).unwrap();

        let cached = provider.current().await.expect("cache fallback");
        assert_eq!(cached.soul, first.soul);
        assert_eq!(cached.hash, first.hash);
    }

    #[tokio::test]
    async fn file_provider_returns_not_found_with_no_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = FileIdentityProvider::new(tmp.path());
        let err = provider.current().await.unwrap_err();
        assert!(matches!(err, IdentityError::NotFound));
    }
}
