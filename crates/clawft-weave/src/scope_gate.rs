//! D12 scope gate (ADR-103, Phase 1 package G).
//!
//! Two gates registered in `rpc_ext::GATES`:
//!
//! * [`scope_gate`]: when a request is *outside any project*, apply the
//!   `kernel.governance.outside_project` policy. Unset, it is `read_only`
//!   for the `--profile user` daemon and `allow_all` for every other root
//!   (ADR-103 D12, amendment pending; see
//!   `clawft_types::config::default_outside_policy`).
//!   Under `read_only` only the explicit [`READ_ONLY_ALLOW`] list passes;
//!   every other method, including methods added later, is denied with
//!   `error_kind = "project_required"`. There is deliberately no "default
//!   read" fallback: `capability.rs` defaults unlisted methods to `Read`,
//!   which would make a new mutating verb anonymous-callable outside a
//!   project.
//! * [`voice_gate`]: R4 decision. The in-process voice principal holds
//!   `read,chat,write`, and `cron.add|remove|enable|disable` are `Write`,
//!   so a spoken command could schedule recurring agent jobs that outlive
//!   the utterance and run unattended. Cron mutations are therefore
//!   denied to [`Principal::InternalVoice`] regardless of project or
//!   policy ([`VOICE_DENIED`]). Listing cron jobs stays allowed.
//!
//! The in-process voice principal is *inside* for the scope gate: it is
//! the daemon itself acting for a present operator, not a client that can
//! name a project, so `read_only` must not strip its Write verbs.
//! `voice_gate` is what limits it.
//!
//! # Outside a project
//!
//! A request is **inside** a project when either
//!
//! 1. the daemon is bound to a project (`handshake_rpc`) and the request
//!    claims no project or claims exactly the bound one; or
//! 2. the daemon is unbound and the request claims a project whose
//!    manifest `<manifests_dir>/<id>.toml` exists, parses, and is
//!    `Active`.
//!
//! Anything else is **outside**: no claim on an unbound daemon; a claim
//! that differs from the binding (the envelope check already refuses
//! these on the wire, the gate does not rely on that); a claim with no
//! manifest, an unreadable manifest, or an archived/missing one. The
//! claim (`Request.project`) is never trusted by itself.
//!
//! # Honest limit
//!
//! Verification proves the claimed project is *registered*, not that the
//! caller belongs to it. `project` is client-declared and every local
//! process of one uid can name any registered project, so in Phase 1 this
//! guards against operating in the wrong place by accident; it is not an
//! authorisation boundary between local processes.

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use clawft_types::config::OutsideProjectPolicy;
use clawft_types::project::{ProjectState, read_manifest};

use crate::capability::Capability;
use crate::rpc_ext::{Denial, GateFuture, GateRequest, Principal};

/// Error text for a scope denial.
pub const SCOPE_MESSAGE: &str = "not in a project; run `weft project init` or pass `--project`";

/// Methods callable outside a project under `read_only`.
///
/// Explicit and reviewed, never derived from `capability.rs`: each entry
/// reads state and mutates nothing. A test pins that every entry is
/// `Capability::Read`, and the population test in `scope_gate_tests.rs`
/// checks every daemon dispatch arm against this list.
pub const READ_ONLY_ALLOW: &[&str] = &[
    "kernel.handshake",
    "kernel.status",
    "kernel.ps",
    "kernel.services",
    "kernel.logs",
    "ping",
    "cluster.status",
    "cluster.nodes",
    "cluster.health",
    "chain.status",
    "chain.verify",
    // The gateway facade (`DaemonKernelFacade`, ADR-102) forwards these
    // read-only calls on a user-level daemon, so they must work outside a
    // project. `chain.local` / `chain.export` stay excluded.
    "chain.tail",
    // Streaming form of `chain.tail` (ADR-103 P2 D).
    "chain.subscribe",
    "ecc.status",
    "ecc.search",
    "ecc.calibrate",
    "agent.list",
    "agent.inspect",
    "control.list",
    "llm.models",
    "mcp.list",
    "tools.mcp",
    "auth.token.validate",
    "project.list",
    "project.show",
];

/// User-level operations: callable outside a project, but only by a caller
/// holding `Admin` (token and project registration are per-user by nature,
/// they do not belong to any one project). Denied under `deny_all`. The
/// routes are registered by other Phase 1 packages; the names are stable.
pub const USER_LEVEL_ALLOW: &[&str] = &[
    "auth.token.issue",
    "auth.token.revoke",
    "auth.token.list",
    "project.register",
    "project.cert.show",
    "project.cert.challenge",
    "project.identity.repair",
    "project.rekey",
    "project.revoke",
    "governance.parent.push",
    "governance.parent.update",
    "governance.reload",
    "project.anchor.restore",
    // Signature-authenticated; a project kernel calls it with its project
    // claim, so it is inside a project and needs no Admin.
    "project.anchor.submit",
];

/// What `deny_all` still permits: liveness, discovery and project lookup.
pub const DENY_ALL_ALLOW: &[&str] = &[
    "kernel.handshake",
    "kernel.status",
    "project.list",
    "project.show",
];

/// Cron mutations the voice principal may never call (R4).
///
/// A spoken verb must not be able to schedule recurring agent work. The
/// scheduler runs without a human present, so the usual "voice is an
/// operator opt-in for one-shot verbs" reasoning in
/// `CallerCtx::internal_voice` does not extend to it. `cron.list` is
/// read-only and stays reachable.
pub const VOICE_DENIED: &[&str] = &["cron.add", "cron.remove", "cron.enable", "cron.disable"];

/// Manifest registry directory the daemon verifies claims against; set at
/// startup by [`init`], else `~/.weftos/projects`.
static MANIFESTS_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Whether this daemon runs the `--profile user` root; selects the default
/// outside-project policy when none is configured.
static USER_PROFILE: AtomicBool = AtomicBool::new(false);

/// Record the manifest directory and profile (daemon startup; tests inject
/// a tempdir).
pub fn init(manifests_dir: Option<PathBuf>, is_user_profile: bool) {
    USER_PROFILE.store(is_user_profile, Ordering::Relaxed);
    *MANIFESTS_DIR.write().unwrap_or_else(|e| e.into_inner()) = manifests_dir;
}

pub(crate) fn manifests_dir() -> Option<PathBuf> {
    if let Some(d) = MANIFESTS_DIR.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return Some(d);
    }
    clawft_types::runtime_paths::home_dir().map(|h| clawft_rpc::resolve::manifests_dir(&h))
}

/// Whether a request is inside a project. See the module docs.
pub fn inside_project(bound: Option<&str>, claim: Option<&str>, manifests_dir: Option<&Path>) -> bool {
    match (bound, claim) {
        (Some(_), None) => true,
        (Some(b), Some(c)) => b == c,
        (None, None) => false,
        (None, Some(c)) => manifests_dir
            .and_then(|d| read_manifest(d, c).ok().flatten())
            .is_some_and(|m| m.state == ProjectState::Active),
    }
}

/// Apply `policy` to `method`. `inside` is only evaluated when the method
/// is not already allowed, so allow-listed calls never touch the registry.
pub fn decide(
    policy: OutsideProjectPolicy,
    method: &str,
    is_admin: bool,
    inside: impl FnOnce() -> bool,
) -> Result<(), Denial> {
    let allowed = match policy {
        OutsideProjectPolicy::AllowAll => return Ok(()),
        OutsideProjectPolicy::ReadOnly => READ_ONLY_ALLOW,
        OutsideProjectPolicy::DenyAll => DENY_ALL_ALLOW,
    };
    let user_level =
        policy == OutsideProjectPolicy::ReadOnly && is_admin && USER_LEVEL_ALLOW.contains(&method);
    if allowed.contains(&method) || user_level || inside() {
        return Ok(());
    }
    Err(Denial::new("project_required", SCOPE_MESSAGE))
}

/// Gate: the outside-project policy.
pub fn scope_gate<'a>(req: &'a GateRequest<'a>) -> GateFuture<'a> {
    Box::pin(async move {
        if req.principal == Principal::InternalVoice {
            return Ok(());
        }
        let policy = req
            .kernel
            .read()
            .await
            .kernel_config()
            .governance
            .effective_outside_project(USER_PROFILE.load(Ordering::Relaxed));
        decide(policy, req.method, req.caps.allows(Capability::Admin), || {
            let bound = crate::handshake_rpc::bound_project_id();
            inside_project(bound.as_deref(), req.project, manifests_dir().as_deref())
        })
    })
}

/// Gate: the voice principal may not mutate cron (R4).
pub fn voice_gate<'a>(req: &'a GateRequest<'a>) -> GateFuture<'a> {
    Box::pin(async move {
        if req.principal == Principal::InternalVoice && VOICE_DENIED.contains(&req.method) {
            return Err(Denial::new(
                "voice_denied",
                format!("'{}' cannot be invoked by voice", req.method),
            ));
        }
        Ok(())
    })
}

/// Serialises tests that set the process-global daemon binding.
#[cfg(test)]
pub(crate) static TEST_BOUND_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
#[path = "scope_gate_tests.rs"]
mod tests;
