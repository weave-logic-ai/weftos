//! Context-graft seam for the agent loop (ADR-058 Phase 5.1).
//!
//! The loop never gets a parallel store; it gets a **graft hook**. A
//! [`ContextGraftProvider`] is consulted once per turn, *after* the
//! prefix-stable system head is assembled and *before* generation, to splice a
//! block of L2-recalled context into the prompt. The kernel-backed
//! implementation (a per-conversation `SessionView` over ExoChain's ECC
//! indexes) lives in the bridge crate `clawft-service-agent`; this crate only
//! defines the seam so the layering stays clean (clawft-core does not depend on
//! clawft-kernel).
//!
//! Placement honors the ADR's cache contract: `[stable head] → [graft block] →
//! [rolling tail]`. The graft block sits *after* the leading run of system
//! messages (system prompt + identity + skills + memory) and *before* the
//! dialogue tail, so the byte-identical head's KV is still reused.

use std::sync::Arc;

use async_trait::async_trait;

use crate::pipeline::traits::LlmMessage;

/// Per-conversation context-graft hook (ADR-058 Phase 5.1).
///
/// Returns the messages to splice into this turn's prompt. An empty vector
/// means "nothing to graft". Implementations MUST be non-fatal: any retrieval
/// error should be swallowed internally and surface as an empty block — a graft
/// failure must never abort the turn.
#[async_trait]
pub trait ContextGraftProvider: Send + Sync + 'static {
    /// Grafted context for `conv_id` relevant to `query` (the user's message).
    async fn graft_block(&self, conv_id: &str, query: &str) -> Vec<LlmMessage>;
}

/// Splice `block` into `messages` at the prefix-stable graft position: directly
/// after the leading run of `system` messages, before everything else.
///
/// This keeps the stable head (system prompt + identity + skills + memory)
/// byte-identical turn-to-turn — the graft block, which varies per query, is
/// inserted *after* it — so prefix-KV reuse is preserved. No-op on an empty
/// block.
pub(crate) fn splice_graft_block(messages: &mut Vec<LlmMessage>, block: Vec<LlmMessage>) {
    if block.is_empty() {
        return;
    }
    let head_len = messages.iter().take_while(|m| m.role == "system").count();
    for (idx, gm) in (head_len..).zip(block) {
        messages.insert(idx, gm);
    }
}

/// A [`ContextGraftProvider`] that never grafts anything — the default when no
/// L2 tier is wired (CLI / tests). Keeps the loop's call site unconditional.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullGraftProvider;

#[async_trait]
impl ContextGraftProvider for NullGraftProvider {
    async fn graft_block(&self, _conv_id: &str, _query: &str) -> Vec<LlmMessage> {
        Vec::new()
    }
}

/// Convenience: an `Arc<dyn ContextGraftProvider>` that grafts nothing.
pub fn null_graft_provider() -> Arc<dyn ContextGraftProvider> {
    Arc::new(NullGraftProvider)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> LlmMessage {
        LlmMessage {
            role: role.into(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    #[test]
    fn splice_after_leading_system_run() {
        let mut messages = vec![
            msg("system", "identity"),
            msg("system", "memory"),
            msg("user", "hello"),
            msg("assistant", "hi"),
        ];
        splice_graft_block(
            &mut messages,
            vec![
                msg("system", "# Recalled\n\nfact A"),
                msg("system", "fact B"),
            ],
        );
        // Head (2 system) intact and unchanged; graft inserted at index 2,3;
        // dialogue tail follows.
        assert_eq!(messages[0].content, "identity");
        assert_eq!(messages[1].content, "memory");
        assert_eq!(messages[2].content, "# Recalled\n\nfact A");
        assert_eq!(messages[3].content, "fact B");
        assert_eq!(messages[4].content, "hello");
        assert_eq!(messages[5].content, "hi");
    }

    #[test]
    fn splice_empty_block_is_noop() {
        let mut messages = vec![msg("system", "s"), msg("user", "u")];
        splice_graft_block(&mut messages, vec![]);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "s");
        assert_eq!(messages[1].content, "u");
    }

    #[test]
    fn splice_with_no_system_head_inserts_at_front() {
        let mut messages = vec![msg("user", "u1"), msg("assistant", "a1")];
        splice_graft_block(&mut messages, vec![msg("system", "grafted")]);
        assert_eq!(messages[0].content, "grafted");
        assert_eq!(messages[1].content, "u1");
    }

    #[tokio::test]
    async fn null_provider_grafts_nothing() {
        let p = NullGraftProvider;
        assert!(p.graft_block("c1", "q").await.is_empty());
    }

    /// ADR-058 Phase 5.3 (deterministic precondition): grafting must NOT disturb
    /// the stable head, so its prefix-KV stays reusable turn-to-turn even as the
    /// graft block (and tail) change. The live latency / actual KV-reuse /
    /// cosine-parity measurements are #[ignore]'d against Hermes :8090.
    #[test]
    fn graft_preserves_head_bytes_for_kv_reuse() {
        let head = || {
            vec![
                msg("system", "sys prompt"),
                msg("system", "# Relevant Memory"),
            ]
        };

        // Turn 1: head + one user turn, grafted with block A.
        let mut t1 = head();
        t1.push(msg("user", "q1"));
        splice_graft_block(&mut t1, vec![msg("system", "graft A")]);

        // Turn 2: same head, longer tail, grafted with a DIFFERENT block.
        let mut t2 = head();
        t2.push(msg("user", "q1"));
        t2.push(msg("assistant", "a1"));
        t2.push(msg("user", "q2"));
        splice_graft_block(&mut t2, vec![msg("system", "graft B is different")]);

        // The leading system head is byte-identical across turns -> KV reusable.
        assert_eq!(t1[0].content, t2[0].content);
        assert_eq!(t1[1].content, t2[1].content);
        // The graft sits AFTER the head (index 2), never before it.
        assert_eq!(t1[2].content, "graft A");
        assert_eq!(t2[2].content, "graft B is different");
    }
}
