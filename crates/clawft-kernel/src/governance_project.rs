//! Project attribution of governance requests (ADR-103 A6, Phase 2 package I).
//!
//! A kernel serves at most one project. The daemon records that project once
//! at boot from the only source it trusts for it (the kernel's own bound
//! project, [`AttestSource::BoundKernel`]) with [`set_instance_project`].
//! Every [`GovernanceRequest`] is then attributed from that attestation, and
//! never from request parameters:
//!
//! * [`GovernanceRequest::new`] and the production struct-literal sites call
//!   [`GovernanceRequest::attributed`], which sets `principal.project_id` /
//!   `instance_id` and the `project_id` context key from the attestation, and
//!   drops any `project_id` the caller put in the context;
//! * [`GovernanceRequest::with_context_entry`] refuses the reserved keys
//!   ([`RESERVED_CONTEXT_KEYS`]), so a client-supplied map cannot forge them;
//! * [`GovernanceRequest::resolved_principal`] (what the engine evaluates and
//!   the chain records) re-stamps from the same attestation.
//!
//! Honest limit: an attestation is only as strong as the transport that
//! produced it. A same-uid local process can still lie through
//! `Request.project` on the claim path of a daemon that has no binding; this
//! is an isolation guard between one user's projects, not a boundary against
//! a hostile local process (Phase 3 peer credentials, Phase 4 sandboxes).
//!
//! # GovernanceRequest construction sites (production code)
//!
//! None derives a project from request parameters. The population test
//! `weave/tests/project_principal_population.rs` greps for new ones.
//!
//! | Site | How it is attributed |
//! |---|---|
//! | `GovernanceRequest::new` (`http_api`, `profile_store`, `hnsw_service`, `causal`, `wasm_runner`) | `attributed()` inside `new` |
//! | `gate::GovernanceGate::check` | literal; strips reserved context keys, `attributed()` |
//! | `workload_governance::gate::WorkloadGate::decide` | literal; `attributed_with` the gate's / instance attestation |
//! | `governance.rs` unit tests | explicit, test-only |

use std::sync::OnceLock;

use crate::governance::{AttestSource, GovernanceRequest, ProjectAttestation};

/// Context keys only the kernel may set.
pub const RESERVED_CONTEXT_KEYS: &[&str] = &["project_id", "instance_id"];

static INSTANCE: OnceLock<(ProjectAttestation, String)> = OnceLock::new();

/// Record the project this kernel serves and its node id (daemon boot,
/// once). Refuses anything but a [`AttestSource::BoundKernel`] attestation;
/// returns `false` if one is already set.
pub fn set_instance_project(att: ProjectAttestation, instance_id: impl Into<String>) -> bool {
    att.source() == AttestSource::BoundKernel && INSTANCE.set((att, instance_id.into())).is_ok()
}

/// The project this kernel serves, if it has been attested.
pub fn instance_project() -> Option<(&'static ProjectAttestation, &'static str)> {
    INSTANCE.get().map(|(a, i)| (a, i.as_str()))
}

impl GovernanceRequest {
    /// Attribute this request to the kernel's attested project (if any).
    pub fn attributed(self) -> Self {
        let inst = instance_project();
        self.attributed_with(inst.map(|(a, _)| a), inst.map(|(_, i)| i))
    }

    /// [`attributed`](Self::attributed) with an explicit attestation. With
    /// `None` the request ends up with no `project_id` in its context and its
    /// principal untouched.
    pub fn attributed_with(mut self, att: Option<&ProjectAttestation>, instance: Option<&str>) -> Self {
        for k in RESERVED_CONTEXT_KEYS {
            self.context.remove(*k);
        }
        let mut p = self.base_principal();
        if let Some(att) = att {
            p = p.with_project(att);
            self.context.insert("project_id".into(), att.project_id().to_owned());
        }
        if let Some(i) = instance {
            p = p.with_instance(i);
            self.context.insert("instance_id".into(), i.to_owned());
        }
        self.principal = Some(p);
        self
    }
}

#[cfg(test)]
#[path = "governance_project_tests.rs"]
mod tests;
