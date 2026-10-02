//! The method allow-list of a project token (ADR-103 A6, Phase 2 package G).
//!
//! A project token ([`TokenScope::Project`]) resolves to the `write`
//! capability, which on its own would let a child call every Write method of
//! the user daemon (agent spawn, cron, workload placement, ...). The token
//! exists for one purpose, the child's [`ParentLink`], so only what that
//! link calls is allowed:
//!
//! | method | why |
//! |---|---|
//! | `shared.embed`, `shared.llm.chat`, `shared.llm.models` | the shared services |
//! | `kernel.handshake` | the link's liveness probe |
//! | `project.token.refresh` | renewing the token itself |
//!
//! Everything else is refused with `project_token_method_denied` before the
//! handler runs. The child's other calls to the parent (`mesh.*`,
//! `project.anchor.submit`) do not present the token: they are authenticated
//! by the spawn nonce and project-key signatures.
//!
//! Honest limit: a same-uid caller that sends the literal scope `admin`
//! (ADR-070, the local-owner shortcut) bypasses this and every other
//! capability check, except that a peer inside a supervised child's process
//! group is treated as anonymous (`child_peer`, ADR-103 A12). A hostile
//! same-uid process outside those groups, or one that leaves its group, is
//! not stopped here; that is a separate uid or a Phase 4 sandbox.
//!
//! [`TokenScope::Project`]: clawft_kernel::token_authority::TokenScope::Project
//! [`ParentLink`]: crate::parent_link::ParentLink

/// Error kind for a project token calling a method it may not.
pub const DENIED_KIND: &str = "project_token_method_denied";

/// Exact methods a project token may call.
pub const ALLOWED_METHODS: &[&str] = &["kernel.handshake", "project.token.refresh"];

/// Prefixes (ending in `.`) a project token may call under.
pub const ALLOWED_PREFIXES: &[&str] = &["shared."];

/// May a project token call `method`?
pub fn allows(method: &str) -> bool {
    ALLOWED_METHODS.contains(&method) || ALLOWED_PREFIXES.iter().any(|p| method.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_the_parent_link_calls() {
        for m in ["shared.embed", "shared.llm.chat", "shared.llm.models", "kernel.handshake", "project.token.refresh"] {
            assert!(allows(m), "{m}");
        }
        for m in [
            "agent.spawn", "cron.add", "workload.place", "kernel.shutdown", "kernel.status", "auth.token.issue",
            "project.start", "project.anchor.submit", "mesh.register", "chain.tail", "sharedx", "shared",
            "",
        ] {
            assert!(!allows(m), "{m}");
        }
    }
}
