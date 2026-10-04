//! The method allow-list of a project token (ADR-103 A6, Phase 2 package G).
//!
//! A project token ([`TokenScope::Project`]) resolves to the `write`
//! capability, which on its own would let a child call every Write method of
//! the user daemon (agent spawn, cron, workload placement, ...). The token
//! serves the child's [`ParentLink`] and an explicit `weave.master` nested
//! project lifecycle surface, so only those calls are allowed:
//!
//! | method | why |
//! |---|---|
//! | `shared.embed`, `shared.llm.chat`, `shared.llm.models` | the shared services |
//! | `kernel.handshake` | the link's liveness probe |
//! | `project.token.refresh` | renewing the token itself |
//! | `project.nested.register/start/stop` | master-only nested project lifecycle; the handler revalidates master identity, root and parentage |
//!
//! Everything else is refused with `project_token_method_denied` before the
//! handler runs. The child's other calls to the parent (`mesh.*`,
//! `project.anchor.submit`) do not present the token: they are authenticated
//! by the spawn nonce and project-key signatures.
//!
//! Honest limit: a same-uid caller that sends the literal scope `admin`
//! (ADR-070, the local-owner shortcut) bypasses this and every other
//! capability check, except that a peer inside a supervised child's process
//! group is treated as anonymous (`child_peer`, ADR-103 A14). A hostile
//! same-uid process outside those groups, or one that leaves its group, is
//! not stopped here; that is a separate uid or a Phase 4 sandbox.
//!
//! [`TokenScope::Project`]: clawft_kernel::token_authority::TokenScope::Project
//! [`ParentLink`]: crate::parent_link::ParentLink

/// Error kind for a project token calling a method it may not.
pub const DENIED_KIND: &str = "project_token_method_denied";

/// Exact methods a project token may call.
pub const ALLOWED_METHODS: &[&str] = &[
    "kernel.handshake", "project.token.refresh",
    "project.nested.register", "project.nested.start", "project.nested.stop",
];

/// Prefixes (ending in `.`) a project token may call under.
pub const ALLOWED_PREFIXES: &[&str] = &["shared."];

/// May a project token call `method`?
pub fn allows(method: &str) -> bool {
    ALLOWED_METHODS.contains(&method) || ALLOWED_PREFIXES.iter().any(|p| method.starts_with(p))
}

/// Methods reachable through the separate child-only parent socket. This
/// ceiling applies even if a child presents a stolen owner token.
pub fn child_endpoint_allows(method: &str) -> bool {
    matches!(method,
        "mesh.challenge" | "mesh.register" | "mesh.heartbeat" | "mesh.unregister"
        | "project.anchor.submit" | "kernel.handshake" | "project.token.refresh"
        | "shared.embed" | "shared.llm.chat" | "shared.llm.models"
        | "project.nested.register" | "project.nested.start" | "project.nested.stop"
    )
}

/// These methods must carry a live project token on the child endpoint;
/// nonce/signature-authenticated mesh and anchor calls are separate.
pub fn child_endpoint_requires_project_token(method: &str) -> bool {
    method.starts_with("shared.") || method == "project.token.refresh" || method.starts_with("project.nested.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_the_parent_link_calls() {
        for m in ["shared.embed", "shared.llm.chat", "shared.llm.models", "kernel.handshake", "project.token.refresh", "project.nested.register", "project.nested.start", "project.nested.stop"] {
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

    #[test]
    fn child_endpoint_ceiling_ignores_the_presented_capability() {
        for method in [
            "mesh.challenge", "mesh.register", "mesh.heartbeat", "mesh.unregister",
            "project.anchor.submit", "kernel.handshake", "project.token.refresh",
            "shared.embed", "shared.llm.chat", "shared.llm.models",
            "project.nested.register", "project.nested.start", "project.nested.stop",
        ] {
            assert!(child_endpoint_allows(method), "{method}");
        }
        for method in ["auth.token.issue", "project.revoke", "kernel.shutdown", "project.start", "shared.fake"] {
            assert!(!child_endpoint_allows(method), "{method}");
        }
        for method in ["shared.embed", "project.token.refresh", "project.nested.register"] {
            assert!(child_endpoint_requires_project_token(method), "{method}");
        }
        assert!(!child_endpoint_requires_project_token("mesh.register"));
    }
}
