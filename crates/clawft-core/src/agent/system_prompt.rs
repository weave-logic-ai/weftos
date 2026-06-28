//! Identity-aware system-prompt builder (agent-core-v1 Phase D1).
//!
//! Produces the leading `system` message for each turn by composing
//! `SOUL.md` + `IDENTITY.md` + workspace context + a binding-thread
//! integrity status. Mirrors the shape of the spike's
//! `build_concierge_system_prompt` in `clawft-weave::daemon` so the
//! Phase D3 cutover preserves the user-visible prompt semantics.
//!
//! ```text
//! [identity]
//! {soul}
//!
//! {identity}
//!
//! [binding-thread-status]
//! {ok | mismatch}
//!
//! [workspace]
//! {workspace_path}
//!
//! [hash]
//! {sha256_hex}
//! ```
//!
//! The hash and binding-thread status are pinned to the trailing
//! lines so they're observable in logs without crowding the persona
//! content the LLM actually consumes for its voice.
//!
//! Plan reference: `docs/plans/agent-core-v1.md` Phase D1.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::warn;

use crate::agent::identity::{BINDING_THREAD_EXCERPT, Identity, IdentityError, IdentityProvider};

/// Status of the binding-thread integrity check.
///
/// The check is non-blocking: a `Mismatch` does not abort the turn;
/// it just annotates the prompt and emits a `warn!` log so an operator
/// can decide whether to roll back the SOUL.md edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingThreadStatus {
    /// Loaded SOUL.md contains [`BINDING_THREAD_EXCERPT`] verbatim.
    Ok,
    /// Loaded SOUL.md does NOT contain [`BINDING_THREAD_EXCERPT`].
    /// The agent still runs but the system prompt is annotated and
    /// a `warn!` is logged.
    Mismatch,
}

impl BindingThreadStatus {
    /// Stable string label for embedding in the system prompt.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Mismatch => "mismatch",
        }
    }

    /// Compute the status by substring-matching `soul` against the
    /// compile-time excerpt. Splitting this out keeps the check
    /// deterministically testable without going through the full
    /// `build()` path.
    pub fn from_soul(soul: &str) -> Self {
        if soul.contains(BINDING_THREAD_EXCERPT) {
            Self::Ok
        } else {
            Self::Mismatch
        }
    }
}

/// Builds an identity-aware system message body for a turn.
///
/// Construction is cheap; reuse one builder per [`AgentLoop`]. The
/// builder calls `provider.current()` on every `build()` invocation
/// so a `FileIdentityProvider` cached load picks up cross-turn edits
/// without explicit invalidation.
///
/// See module documentation for the output layout.
pub struct SystemPromptBuilder {
    identity_provider: Arc<dyn IdentityProvider>,
    workspace: PathBuf,
}

impl SystemPromptBuilder {
    /// Wire the builder against an [`IdentityProvider`] and the
    /// workspace path used in the `[workspace]` section.
    pub fn new(identity_provider: Arc<dyn IdentityProvider>, workspace: PathBuf) -> Self {
        Self {
            identity_provider,
            workspace,
        }
    }

    /// Borrow the workspace path the builder advertises in prompts.
    pub fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }

    /// Build the system message body.
    ///
    /// Returns the rendered prompt on success; an
    /// [`IdentityError::NotFound`] when the underlying provider
    /// cannot resolve identity content (callers should surface this
    /// to the chat client as `agent: identity load failed: ...`).
    pub async fn build(&self) -> Result<String, IdentityError> {
        let identity = self.identity_provider.current().await?;
        Ok(self.render(&identity))
    }

    /// Render an in-memory [`Identity`] into the prompt body. Split
    /// out so unit tests can exercise the formatting deterministically
    /// without going through an [`IdentityProvider`].
    pub fn render(&self, identity: &Identity) -> String {
        let status = BindingThreadStatus::from_soul(&identity.soul);
        if status == BindingThreadStatus::Mismatch {
            warn!(
                hash = %identity.hash,
                source = identity.source,
                "binding-thread mismatch: SOUL.md does not contain the \
                 compile-time BINDING_THREAD_EXCERPT — running in \
                 degraded mode"
            );
        }

        // Reserve roughly: identity bodies + ~512 bytes of scaffolding.
        let mut s = String::with_capacity(identity.soul.len() + identity.identity.len() + 512);
        s.push_str("[identity]\n");
        s.push_str(&identity.soul);
        // Make sure there's a blank line between the two bodies even
        // if SOUL.md doesn't end in a newline.
        if !identity.soul.ends_with('\n') {
            s.push('\n');
        }
        s.push('\n');
        s.push_str(&identity.identity);
        if !identity.identity.ends_with('\n') {
            s.push('\n');
        }
        s.push('\n');

        s.push_str("[binding-thread-status]\n");
        s.push_str(status.as_str());
        s.push('\n');
        if status == BindingThreadStatus::Mismatch {
            s.push_str(
                "note: SOUL.md does not match the compile-time \
                 binding-thread excerpt; running in degraded mode.\n",
            );
        }
        s.push('\n');

        s.push_str("[workspace]\n");
        s.push_str(&self.workspace.display().to_string());
        s.push('\n');
        s.push('\n');

        s.push_str("[hash]\n");
        s.push_str(&identity.hash);
        s.push('\n');

        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::{FileIdentityProvider, sha256_identity_hash};
    use async_trait::async_trait;

    /// In-memory identity provider for tests.
    struct StubProvider(Identity);

    #[async_trait]
    impl IdentityProvider for StubProvider {
        async fn current(&self) -> Result<Identity, IdentityError> {
            Ok(self.0.clone())
        }
    }

    /// Provider that always errors — exercises the propagation path.
    struct FailingProvider;

    #[async_trait]
    impl IdentityProvider for FailingProvider {
        async fn current(&self) -> Result<Identity, IdentityError> {
            Err(IdentityError::NotFound)
        }
    }

    fn ok_identity() -> Identity {
        let soul = format!(
            "# SOUL.md\n\nWe carry the binding thread: \
             {BINDING_THREAD_EXCERPT}.\n"
        );
        let identity = "# IDENTITY.md\n\nI am clawft.".to_string();
        let hash = sha256_identity_hash(&soul, &identity);
        Identity {
            soul,
            identity,
            hash,
            source: "test",
        }
    }

    #[tokio::test]
    async fn build_includes_soul_identity_workspace_and_hash() {
        let id = ok_identity();
        let expected_soul = id.soul.clone();
        let expected_identity = id.identity.clone();
        let expected_hash = id.hash.clone();
        let provider = Arc::new(StubProvider(id));
        let builder = SystemPromptBuilder::new(provider, PathBuf::from("/tmp/ws"));

        let prompt = builder.build().await.expect("build ok");

        assert!(prompt.contains("[identity]"));
        assert!(prompt.contains(expected_soul.trim()));
        assert!(prompt.contains(expected_identity.trim()));
        assert!(prompt.contains("[binding-thread-status]\nok"));
        assert!(prompt.contains("[workspace]\n/tmp/ws"));
        assert!(prompt.contains("[hash]\n"));
        assert!(prompt.contains(&expected_hash));
    }

    #[tokio::test]
    async fn build_marks_mismatch_when_excerpt_absent() {
        let soul = "# SOUL.md\n\nNothing distinctive in here.\n".to_string();
        let identity = "# IDENTITY.md\n\nI am clawft.".to_string();
        let id = Identity {
            hash: sha256_identity_hash(&soul, &identity),
            soul,
            identity,
            source: "test",
        };
        let provider = Arc::new(StubProvider(id));
        let builder = SystemPromptBuilder::new(provider, PathBuf::from("/tmp/ws"));

        let prompt = builder.build().await.unwrap();
        assert!(prompt.contains("[binding-thread-status]\nmismatch"));
        assert!(prompt.contains("degraded mode"));
    }

    #[tokio::test]
    async fn build_propagates_provider_error() {
        let provider = Arc::new(FailingProvider);
        let builder = SystemPromptBuilder::new(provider, PathBuf::from("/tmp"));
        let err = builder.build().await.unwrap_err();
        assert!(matches!(err, IdentityError::NotFound));
    }

    #[test]
    fn binding_status_helper_matches_constant() {
        let with_excerpt = format!("prefix\n{BINDING_THREAD_EXCERPT}\nsuffix");
        assert_eq!(
            BindingThreadStatus::from_soul(&with_excerpt),
            BindingThreadStatus::Ok
        );
        assert_eq!(
            BindingThreadStatus::from_soul("nothing relevant"),
            BindingThreadStatus::Mismatch
        );
    }

    #[tokio::test]
    async fn integrates_with_file_identity_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let clawft = tmp.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        std::fs::write(
            clawft.join("SOUL.md"),
            format!("# SOUL\n{BINDING_THREAD_EXCERPT}\n"),
        )
        .unwrap();
        std::fs::write(clawft.join("IDENTITY.md"), "# IDENTITY\nclawft").unwrap();

        let provider: Arc<dyn IdentityProvider> = Arc::new(FileIdentityProvider::new(tmp.path()));
        let builder = SystemPromptBuilder::new(provider, tmp.path().to_path_buf());

        let prompt = builder.build().await.expect("build ok");
        assert!(prompt.contains("[binding-thread-status]\nok"));
        assert!(prompt.contains(&tmp.path().display().to_string()));
    }
}
