//! `clawft-weave` — shared library surface for the `weaver` CLI.
//!
//! The CLI binary lives in [`main.rs`](../../../src/main.rs). This lib
//! target re-exports the modules used by integration tests (notably
//! [`daemon::handle_connection`] for driving a kernel listener from a
//! temp socket). Keeping these modules public under `pub` rather than
//! `pub(crate)` is the minimum change required to make integration
//! tests link against them.

/// `app.*` daemon RPC handlers (mesh-placement-06).
pub mod app_rpc;
pub mod capability;
/// Tracing → pending-buffer bridge for ExoChain (WEFT-597).
pub mod chain_bridge;
pub mod chain_subscribe_rpc;
pub mod client;
pub mod commands;
pub mod control;
pub mod conv_postmortem;
// Local RPC daemon: Unix UDS + Windows named pipes (WEFT-559).
#[cfg(any(unix, windows))]
pub mod daemon;
/// Boot refusals (exit 78) and the SIGHUP re-exec plan.
#[cfg(any(unix, windows))]
pub mod boot_refusal;
/// Single-instance advisory lock on `<runtime>/kernel.lock` (ADR-103 P0b).
#[cfg(any(unix, windows))]
pub mod instance_lock;
#[cfg(any(unix, windows))]
pub mod llm_service;
/// `kernel.handshake` and the request-envelope gate (ADR-103 D14).
#[cfg(any(unix, windows))]
pub mod handshake_rpc;
/// `project.*` RPCs over the per-user manifest store (ADR-103 Phase 1).
#[cfg(any(unix, windows))]
pub mod project_cert_rpc;
/// `project.anchor.submit`: a project's signed chain-head statement (ADR-103 A7).
#[cfg(any(unix, windows))]
pub mod anchor_rpc;
pub mod project_rpc;
/// The per-user daemon profile (`weaver kernel start --profile user`).
#[cfg(any(unix, windows))]
pub mod user_daemon;
/// The user key (`~/.weftos/user.key`) and its migration from `chain.key` (ADR-103 D-5).
pub mod user_key;
/// Mesh mode as reported in the handshake (ADR-103 P3-U).
pub mod mesh_state;
/// `weaver doctor` mesh-service checks (P3-H).
#[cfg(all(unix, feature = "mesh"))]
pub mod mesh_doctor;
/// Installer tiers: service/user/project skew and the printed service update lines (P3-H).
pub mod install_tiers;
/// Chain events the mesh link records (journal anchors, service binding), queued until appended.
pub mod mesh_local_chain;
/// Client side of the machine mesh service: link, registration, delivery, verdicts (P3-U).
#[cfg(all(unix, feature = "mesh"))]
pub mod mesh_local_glue;
/// Inbound delivery into the A2A router and outbound remote forwarding (P3-U).
#[cfg(all(unix, feature = "mesh"))]
pub mod mesh_local_sink;
/// Boot glue: mesh mode and node identity before the kernel, the link after it.
#[cfg(all(unix, feature = "mesh"))]
pub mod mesh_boot;
/// `verdict.request` answered by the governance gate (P3-U).
#[cfg(all(unix, feature = "mesh"))]
pub mod mesh_local_verdict;
/// Daemon RPC extension seam: method-prefix routes + pre-dispatch gates (ADR-103 D0).
#[cfg(any(unix, windows))]
pub mod rpc_ext;
/// `mesh.*` (mesh-local/1): the user daemon's child registry (ADR-103 Phase 2 H).
#[cfg(unix)]
pub mod mesh_local_registry;
/// `mesh.challenge|register|heartbeat|unregister` handlers (ADR-103 Phase 2 H).
#[cfg(unix)]
pub mod mesh_local_rpc;
/// Child bootstrap: spawn handshake, key, registration, certificate (ADR-103 Phase 2 H).
#[cfg(unix)]
pub mod project_boot;
/// The child's wire to the user daemon and the real anchor transport (ADR-103 Phase 2 H).
#[cfg(unix)]
pub mod project_boot_link;
/// The child's running half: genesis, heartbeat, anchors, shutdown (ADR-103 Phase 2 H).
#[cfg(unix)]
pub mod project_boot_run;
/// Seams `daemon::run` calls for the per-project kernel profile (ADR-103 A6).
#[cfg(any(unix, windows))]
pub mod project_hooks;
/// Read a running process's real environment (supervisor tests, ADR-103 Phase 2 F).
#[cfg(any(unix, windows))]
pub mod env_probe;
/// Parent-side client for the shared services of a `project`-profile kernel (ADR-103 Phase 2 F).
#[cfg(any(unix, windows))]
pub mod parent_link;
/// Remote embedder and LLM backend over the parent link (ADR-103 Phase 2 F).
#[cfg(any(unix, windows))]
pub mod parent_services;
/// The `project`-profile service adjustments (ADR-103 Phase 2 F).
#[cfg(any(unix, windows))]
pub mod project_profile;
/// Per-project rate limit and token budget for `shared.*` (ADR-103 Phase 2 F).
#[cfg(any(unix, windows))]
pub mod shared_meter;
/// Process state behind `shared.*`: meter, cached limits, permits, model allow-list (ADR-103 Phase 2 F).
#[cfg(any(unix, windows))]
pub mod shared_state;
/// `shared.*` RPCs: the user daemon's embedding and LLM services for its projects (ADR-103 Phase 2 F).
#[cfg(any(unix, windows))]
pub mod shared_rpc;
/// `VerifiedProject`: a project id with cryptographic provenance (ADR-103 A6).
#[cfg(any(unix, windows))]
pub mod verified_project;
/// User-signed forward header sign and verify (ADR-103 A6, package I).
#[cfg(any(unix, windows))]
pub mod project_forward;
/// Establishes the caller's `VerifiedProject` per request (package I).
#[cfg(any(unix, windows))]
pub mod caller_principal;
/// D12 scope gate: outside-project policy and the voice deny-list (ADR-103).
#[cfg(any(unix, windows))]
pub mod scope_gate;
/// `governance.parent.push|update` and `governance.reload` (ADR-103 D8).
#[cfg(any(unix, windows))]
pub mod governance_push;
/// The project supervisor: per-project child kernels under the user daemon
/// (ADR-103 A6, Phase 2 package G).
#[cfg(all(unix, feature = "exochain", feature = "placement"))]
pub mod project_supervisor;
/// The method allow-list of a project token (ADR-103 A6, package G).
#[cfg(any(unix, windows))]
pub mod project_token_scope;
/// Open-stream counter (idle-stop input of a project kernel).
#[cfg(any(unix, windows))]
pub mod open_streams;
/// `weaver project migrate-kernel` logic (ADR-103 A6, package G).
#[cfg(all(unix, feature = "exochain", feature = "placement"))]
pub mod project_migrate;
/// `project.start|stop|restart|status|ensure_running` (ADR-103 A6, package G).
#[cfg(all(unix, feature = "exochain", feature = "placement"))]
pub mod project_lifecycle_rpc;
/// The lifecycle routes' answer in a build without the supervisor. Always
/// compiled so default-feature tests exercise it (the self dev-dependency
/// turns the default features back on, so `--no-default-features` test runs
/// are not possible); it is [`project_lifecycle_rpc`] where the supervisor is
/// not built.
pub mod project_lifecycle_stub;
/// Without the supervisor (non-unix, or no `exochain`/`placement`) the
/// lifecycle routes still exist and answer `not_user_daemon`.
#[cfg(not(all(unix, feature = "exochain", feature = "placement")))]
pub use project_lifecycle_stub as project_lifecycle_rpc;
/// `auth.token.*` RPC handlers and the per-kernel token authority (ADR-102 D3).
#[cfg(any(unix, windows))]
pub mod token_rpc;
/// TCP-relay request sanitiser: strips self-asserted scope strings (ADR-102 D3).
#[cfg(unix)]
pub mod relay_auth;
/// Live MCP registry RPC handlers (WEFT-494 / ADR-070). Available on all
/// platforms so unit tests can exercise add/list/remove without UDS.
pub mod mcp_rpc;
pub mod node_identity;
/// Mic source node discovery for whisper / classify (ADR-103 D11 follow-up).
pub mod mic_source;
/// Node facts probe, signing, cache and `cluster.facts` (mesh-placement-03).
#[cfg(any(feature = "mesh", feature = "exochain"))]
pub mod node_facts_rpc;
pub mod protocol;
pub mod service_units;
pub mod service_units_system;
/// Governance-gate helper for daemon RPC families (mesh-placement-06).
pub mod rpc_gate;
// WEFT-720 residual: `spatial_rpc` dropped when BvhStore/CLI helpers
// diverged; reattach via SpatialService before re-enabling spatial_cli_e2e.
#[cfg(feature = "rvf-rpc")]
pub mod rvf_codec;
#[cfg(feature = "rvf-rpc")]
pub mod rvf_rpc;
#[cfg(feature = "exochain")]
pub mod turn_ledger;
pub mod voice_loop;
pub mod voice_router;
pub mod voice_trace;
/// Node-local workload catalog (ADR-099, mesh-placement-06).
pub mod workload_registry;
/// `workload.*` daemon RPC family (ADR-099, mesh-placement-06).
#[cfg(feature = "exochain")]
pub mod workload_rpc;
/// Placement control plane RPCs (ADR-099, mesh-placement-12).
#[cfg(all(feature = "placement", unix))]
pub mod workload_place_rpc;
/// Operator policy files for placement (mesh-placement-12).
#[cfg(all(feature = "placement", unix))]
pub mod workload_place_policy;
/// This node's `workload-host`, served to other controllers (mesh-placement-12).
#[cfg(all(feature = "placement", unix))]
pub mod workload_host_serve;
