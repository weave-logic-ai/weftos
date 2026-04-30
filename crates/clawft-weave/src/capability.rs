//! Per-method capability gating for daemon RPC dispatch (WEFT-479).
//!
//! Today the daemon's UDS listener accepts every JSON-RPC verb from
//! every caller. There is no notion of "this caller may call
//! `kernel.shutdown` but not `agent.spawn`". This module is the
//! minimum honest gate: each method declares a required
//! [`Capability`], and an effective capability set is computed for
//! the caller (anonymous by default, escalated when a Bearer header
//! is presented and matches a token issued by the kernel's
//! [`AuthService`](clawft_kernel::AuthService)).
//!
//! # Capability classes
//!
//! - [`Capability::Read`] — read-only verbs that don't mutate state
//!   (`kernel.status`, `kernel.ps`, `agent.list`, ...). Anonymous
//!   callers always have this.
//! - [`Capability::Chat`] — conversational verbs that the LLM-side
//!   integration needs (`agent.chat`, `agent.chat.cancel`). Granted
//!   to anonymous callers by default; an operator can tighten this
//!   later by removing `Chat` from the anonymous baseline.
//! - [`Capability::Write`] — mutating verbs that change agent or
//!   substrate state (`agent.spawn`, `agent.stop`, `agent.send`,
//!   `memory.delete`, `substrate.publish`, ...). Requires
//!   authentication.
//! - [`Capability::Admin`] — destructive verbs that affect the
//!   daemon process itself (`kernel.shutdown`, `kernel.kill-process`,
//!   `kernel.restart-service`). Requires authentication AND the
//!   token's scope must include `admin`.
//!
//! # Posture
//!
//! Default-permissive on read; default-deny on admin. The
//! anonymous baseline includes `Read` and `Chat` (back-compat
//! posture for existing UDS callers). `Write` and `Admin` require an
//! authenticated token. The wire format adds an optional `auth`
//! field to the JSON-RPC request envelope; absent or empty `auth`
//! defaults to anonymous.

use std::collections::HashSet;

/// A discrete privilege the daemon RPC dispatcher checks before
/// executing a method handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Read-only inspection (no state change).
    Read,
    /// Conversational verbs (agent.chat, etc.).
    Chat,
    /// Mutating writes to agents / substrate / memory.
    Write,
    /// Destructive admin verbs (shutdown, kill, restart).
    Admin,
}

/// Look up the [`Capability`] required by a given JSON-RPC method.
///
/// Methods not in the table default to [`Capability::Read`]. This
/// keeps unknown verbs callable by anonymous clients (so a typo
/// surfaces as "unknown method" from the dispatcher rather than
/// silent permission-denied), while ensuring the four classes of
/// dangerous verbs we DO know about are gated correctly. As new
/// verbs land, the author should add them here.
pub fn required_capability(method: &str) -> Capability {
    match method {
        // ── Admin: destructive verbs that affect the daemon itself ─
        "kernel.shutdown" => Capability::Admin,
        "kernel.kill-process" => Capability::Admin,
        "kernel.restart-service" => Capability::Admin,
        "cluster.join" => Capability::Admin,
        "cluster.leave" => Capability::Admin,
        "chain.checkpoint" => Capability::Admin,

        // ── Write: state-mutating verbs ─────────────────────────────
        "agent.register" => Capability::Write,
        "agent.spawn" => Capability::Write,
        "agent.stop" => Capability::Write,
        "agent.restart" => Capability::Write,
        "agent.send" => Capability::Write,
        "node.register" => Capability::Write,
        "memory.delete" => Capability::Write,
        "substrate.publish" => Capability::Write,
        "substrate.canonical_publish_payload" => Capability::Write,
        "substrate.notify" => Capability::Write,
        "control.set_enabled" => Capability::Write,
        "terminal.spawn" => Capability::Write,
        "terminal.write" => Capability::Write,
        "terminal.resize" => Capability::Write,
        "terminal.close" => Capability::Write,

        // ── Chat: LLM-conversational verbs ──────────────────────────
        "agent.chat" => Capability::Chat,
        "agent.chat.cancel" => Capability::Chat,
        "llm.prompt" => Capability::Chat,

        // ── Read: everything else explicitly classified ─────────────
        "kernel.status"
        | "kernel.ps"
        | "kernel.services"
        | "kernel.logs"
        | "cluster.status"
        | "cluster.nodes"
        | "cluster.health"
        | "cluster.shards"
        | "chain.status"
        | "chain.local"
        | "chain.verify"
        | "agent.inspect"
        | "agent.list"
        | "control.list"
        | "node.identity"
        | "substrate.read"
        | "substrate.list"
        | "substrate.subscribe"
        | "ipc.subscribe_stream" => Capability::Read,

        // Default for anything we haven't explicitly classified.
        // Read is the safest baseline — the verb still goes through
        // its own per-handler validation. New verbs should be added
        // to this table as they're written.
        _ => Capability::Read,
    }
}

/// The effective capability set for a single RPC caller.
///
/// Built per-request from the optional `auth` field on the JSON-RPC
/// envelope:
///
/// - No `auth` (or empty string) → [`Self::anonymous`]: `{Read, Chat}`.
/// - `auth` matches a kernel-issued token → token scopes are mapped
///   to the corresponding capabilities (e.g. scope `"admin"` →
///   `Capability::Admin`).
/// - `auth` present but does NOT match any token → [`Self::denied`]:
///   empty set (every gated verb fails). Anonymous would have been
///   the safer default but a token that LOOKED valid getting silently
///   downgraded would mask a misconfiguration; deny instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerCapabilities {
    granted: HashSet<Capability>,
}

impl CallerCapabilities {
    /// Anonymous baseline: `{Read, Chat}`.
    pub fn anonymous() -> Self {
        let mut granted = HashSet::new();
        granted.insert(Capability::Read);
        granted.insert(Capability::Chat);
        Self { granted }
    }

    /// Empty set — every gated verb fails. Use when an `auth` token
    /// was presented but did not validate.
    pub fn denied() -> Self {
        Self {
            granted: HashSet::new(),
        }
    }

    /// Construct from an explicit set of scope strings (typically
    /// from `AuthToken::scopes`). Recognised scopes:
    ///
    /// - `"read"` → [`Capability::Read`]
    /// - `"chat"` → [`Capability::Chat`]
    /// - `"write"` → [`Capability::Write`]
    /// - `"admin"` → [`Capability::Admin`] (also implies the others)
    ///
    /// Unknown scopes are ignored. An authenticated caller always
    /// gets at least `Read`.
    pub fn from_scopes<I, S>(scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut granted = HashSet::new();
        granted.insert(Capability::Read);
        for scope in scopes {
            match scope.as_ref() {
                "read" => {
                    granted.insert(Capability::Read);
                }
                "chat" => {
                    granted.insert(Capability::Chat);
                }
                "write" => {
                    granted.insert(Capability::Write);
                }
                "admin" => {
                    granted.insert(Capability::Read);
                    granted.insert(Capability::Chat);
                    granted.insert(Capability::Write);
                    granted.insert(Capability::Admin);
                }
                _ => {}
            }
        }
        Self { granted }
    }

    /// True when this caller may invoke a method requiring `cap`.
    pub fn allows(&self, cap: Capability) -> bool {
        self.granted.contains(&cap)
    }

    /// True when this caller may invoke `method`.
    pub fn allows_method(&self, method: &str) -> bool {
        self.allows(required_capability(method))
    }
}

impl Default for CallerCapabilities {
    fn default() -> Self {
        Self::anonymous()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_can_read_and_chat() {
        let caps = CallerCapabilities::anonymous();
        assert!(caps.allows(Capability::Read));
        assert!(caps.allows(Capability::Chat));
        assert!(!caps.allows(Capability::Write));
        assert!(!caps.allows(Capability::Admin));
    }

    #[test]
    fn anonymous_method_gating() {
        let caps = CallerCapabilities::anonymous();
        assert!(caps.allows_method("kernel.status"));
        assert!(caps.allows_method("agent.list"));
        assert!(caps.allows_method("agent.chat"));
        // gated:
        assert!(!caps.allows_method("agent.spawn"));
        assert!(!caps.allows_method("memory.delete"));
        assert!(!caps.allows_method("kernel.shutdown"));
        assert!(!caps.allows_method("kernel.kill-process"));
    }

    #[test]
    fn denied_set_blocks_everything_gated() {
        let caps = CallerCapabilities::denied();
        // Read still works because Read is in the empty set's view —
        // wait, no: denied is empty, so even Read fails. This is the
        // intended behaviour: a presented-but-invalid token is a
        // misconfiguration that should NOT silently fall back to
        // anonymous.
        assert!(!caps.allows(Capability::Read));
        assert!(!caps.allows_method("kernel.status"));
        assert!(!caps.allows_method("agent.spawn"));
        assert!(!caps.allows_method("kernel.shutdown"));
    }

    #[test]
    fn admin_scope_implies_all() {
        let caps = CallerCapabilities::from_scopes(["admin"]);
        assert!(caps.allows(Capability::Read));
        assert!(caps.allows(Capability::Chat));
        assert!(caps.allows(Capability::Write));
        assert!(caps.allows(Capability::Admin));
        assert!(caps.allows_method("kernel.shutdown"));
        assert!(caps.allows_method("agent.spawn"));
    }

    #[test]
    fn write_scope_does_not_imply_admin() {
        let caps = CallerCapabilities::from_scopes(["write"]);
        assert!(caps.allows_method("agent.spawn"));
        assert!(caps.allows_method("memory.delete"));
        assert!(!caps.allows_method("kernel.shutdown"));
        assert!(!caps.allows_method("kernel.kill-process"));
    }

    #[test]
    fn unknown_scopes_ignored() {
        let caps = CallerCapabilities::from_scopes(["bogus", "read"]);
        assert!(caps.allows(Capability::Read));
        assert!(!caps.allows(Capability::Write));
    }

    #[test]
    fn unknown_methods_default_to_read() {
        assert_eq!(required_capability("zzz.never_heard_of_this"), Capability::Read);
    }

    #[test]
    fn admin_methods_classified_correctly() {
        for m in [
            "kernel.shutdown",
            "kernel.kill-process",
            "kernel.restart-service",
            "cluster.join",
            "cluster.leave",
            "chain.checkpoint",
        ] {
            assert_eq!(
                required_capability(m),
                Capability::Admin,
                "method {m} should be Admin",
            );
        }
    }

    #[test]
    fn write_methods_classified_correctly() {
        for m in [
            "agent.register",
            "agent.spawn",
            "agent.stop",
            "agent.send",
            "memory.delete",
            "substrate.publish",
            "terminal.spawn",
        ] {
            assert_eq!(
                required_capability(m),
                Capability::Write,
                "method {m} should be Write",
            );
        }
    }
}
