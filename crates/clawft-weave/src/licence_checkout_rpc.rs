//! `weaver cog checkout ...` on the daemon (ADR-106 phase 3).
//!
//! | method                          | capability | what it does |
//! |---------------------------------|------------|--------------|
//! | `workload.cog.checkout`         | Admin      | `{cog_id, version, arch}`: ask the steward (or this node's relay, on the steward) |
//! | `workload.cog.checkout.approve` | Admin      | `{cog_id, version, prepare: true}`: what the operator signs; `{signed: [..]}`: verify, store, flood |
//! | `workload.cog.checkout.status`  | Read       | binding, steward, relay, held grants (with the run gate per artifact) and approvals |
//!
//! `release`, `renew` and `list` are in `licence_checkout_verbs`.
//!
//! The operator key stays with the CLI: the daemon names the content
//! (mesh id, cog, version, the sha256 set from the held grant), the CLI signs
//! it under `weft-licence-v1/approval`, and the daemon verifies it (pinned
//! operator key, local mesh id), stores it and floods it through the licence
//! exchange (`issue_approval`). `reapprove_orphaned` does the same for every
//! approval a `mesh_nonce` change orphaned. Each step is chained:
//! `cog.checkout.request`, `cog.checkout.received` or `cog.checkout.refused`
//! (requester side; the steward's relay chains its own `granted` and
//! `refused`), and `cog.checkout.approved`.

use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::licence::{
    CheckoutGrant, CheckoutWire, LicenceExchange, RunRequest, RunVerdict, SignedApproval, SignedGrant,
    check_run,
};
use clawft_kernel::mesh_cog::CogMesh;
use clawft_rpc::Response;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::licence_boot::{self, LicenceRuntime};
use crate::licence_steward::{self, RelayState};

/// Methods served here.
pub const METHODS: &[&str] =
    &["workload.cog.checkout", "workload.cog.checkout.approve", "workload.cog.checkout.status"];

/// Every method this module routes (the verbs module adds release, renew, list).
pub fn handles_any(m: &str) -> bool {
    METHODS.contains(&m) || crate::licence_checkout_verbs::METHODS.contains(&m)
}

/// How long a member waits for the steward's answer (a first checkout fetches the bytes).
pub const STEWARD_TIMEOUT: Duration = Duration::from_secs(90);

/// True for the methods this module serves.
pub fn handles(m: &str) -> bool {
    handles_any(m)
}

/// Is the steward a licensed peer of this node right now?
pub type Reachable<'a> = &'a (dyn Fn(&str) -> bool + Send + Sync);

/// What one call needs besides its params.
pub struct Ctx<'a> {
    /// The daemon's licence runtime.
    pub rt: &'a LicenceRuntime,
    /// The installed cog mesh (`None` before placement started).
    pub mesh: Option<Arc<CogMesh>>,
    /// The licence exchange (holds the approval store; `None` without a mesh).
    pub exchange: Option<Arc<LicenceExchange>>,
    /// The relay state from placement start.
    pub relay: Option<RelayState>,
    /// Whether a node is a licensed peer now.
    pub reachable: Reachable<'a>,
    /// This node's arch, the default `arch`.
    pub arch: Option<&'a str>,
    /// Unix seconds.
    pub now: u64,
    /// Who is asking (a checkout on the steward is charged to and gated as
    /// this principal).
    pub principal: &'a str,
    /// The steward's renewer (release and renew use its path).
    pub renewer: Option<Arc<clawft_kernel::licence::Renewer>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckoutParams {
    cog_id: String,
    version: String,
    #[serde(default)]
    arch: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApproveParams {
    #[serde(default)]
    cog_id: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    prepare: bool,
    #[serde(default)]
    reapprove_orphaned: bool,
    /// Approve exactly these (the operator checked them); default: the held grant's set.
    #[serde(default)]
    sha256: Vec<String>,
    #[serde(default)]
    signed: Vec<SignedApproval>,
}

fn parse<T: serde::de::DeserializeOwned>(m: &str, params: Value) -> Result<T, String> {
    serde_json::from_value(params).map_err(|e| format!("invalid {m} params: {e}"))
}

fn chain(rt: &LicenceRuntime, kind: &str, payload: Value) {
    rt.chain.append(licence_boot::LICENCE_CHAIN_SOURCE, kind, Some(payload));
}

fn request_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

/// Serve one method.
pub async fn route(ctx: &Ctx<'_>, method: &str, params: Value) -> Response {
    let r = match method {
        "workload.cog.checkout" => match parse::<CheckoutParams>(method, params) {
            Ok(p) => checkout(ctx, p).await,
            Err(e) => Err(e),
        },
        "workload.cog.checkout.approve" => match parse::<ApproveParams>(method, params) {
            Ok(p) => approve(ctx, p).await,
            Err(e) => Err(e),
        },
        "workload.cog.checkout.status" => Ok(status(ctx)),
        "workload.cog.checkout.list" => Ok(crate::licence_checkout_verbs::list(ctx)),
        "workload.cog.checkout.release" => match parse(method, params) {
            Ok(p) => crate::licence_checkout_verbs::release(ctx, p).await,
            Err(e) => Err(e),
        },
        "workload.cog.checkout.renew" => match parse(method, params) {
            Ok(p) => crate::licence_checkout_verbs::renew(ctx, p).await,
            Err(e) => Err(e),
        },
        other => Err(format!("{other} is not a checkout method")),
    };
    match r {
        Ok(v) => Response::success(v),
        Err(e) => Response::error(e),
    }
}

fn grant_summary(signed: &SignedGrant) -> Value {
    match serde_json::from_str::<CheckoutGrant>(&signed.payload) {
        Ok(g) => json!({ "grant_id": g.grant_id, "seq": g.seq, "cog_id": g.cog_id, "version": g.version,
                         "artifacts": g.artifacts, "expires_at": g.expires_at }),
        Err(_) => Value::Null,
    }
}

async fn checkout(ctx: &Ctx<'_>, p: CheckoutParams) -> Result<Value, String> {
    let rt = ctx.rt;
    let arch = p.arch.or_else(|| ctx.arch.map(str::to_owned)).ok_or("give --arch (this node's arch is unknown)")?;
    let wire = CheckoutWire { request_id: request_id(), cog_id: p.cog_id, version: p.version, arch };
    wire.validate()?;
    let binding = rt
        .store()
        .active_binding()
        .ok_or("no Seed binding is in effect on this node (weaver workload node status)")?;
    let mesh = ctx.mesh.clone().ok_or("placement has not started on this node (no cog mesh)")?;
    let steward = binding.steward_node_id.clone();
    let local = steward == rt.steward_node_id;
    chain(rt, "cog.checkout.request", json!({
        "cog_id": wire.cog_id, "version": wire.version, "arch": wire.arch,
        "steward": steward, "via": if local { "local" } else { "steward" }, "principal": ctx.principal,
    }));
    let out = if local {
        if mesh.relay().is_none() {
            return Err(format!(
                "this node is the bound steward but runs no relay: configure {} in the runtime dir and restart",
                licence_steward::LINK_FILE
            ));
        }
        // The relay rate-limits, gates and chains (`granted` / `refused`) as this principal.
        mesh.checkout_as(ctx.principal, &wire).await
    } else {
        let r = mesh.request_checkout(&steward, wire.clone(), STEWARD_TIMEOUT).await;
        match &r {
            Ok(g) => chain(rt, "cog.checkout.received", json!({ "steward": steward, "grant": grant_summary(g) })),
            Err(e) => chain(rt, "cog.checkout.refused", json!({
                "side": "requester", "steward": steward, "cog_id": wire.cog_id,
                "version": wire.version, "arch": wire.arch, "code": e.code(),
            })),
        }
        r
    };
    match out {
        Ok(g) => Ok(json!({ "granted": grant_summary(&g), "steward": steward, "via": if local { "local" } else { "steward" } })),
        Err(e) => Err(format!("[{}] {e}", e.code())),
    }
}

/// The sha256 set an approval for `cog` `version` covers: the operator's
/// own list, else every artifact of the held grant.
fn approval_set(ctx: &Ctx<'_>, cog: &str, version: &str, given: &[String]) -> Result<(Vec<String>, Value), String> {
    let held = ctx.rt.store().held_grant(cog, version).map(|(_, g)| grant_summary(&g));
    if !given.is_empty() {
        let mut v = given.to_vec();
        v.sort();
        v.dedup();
        return Ok((v, held.unwrap_or(Value::Null)));
    }
    let held = held.ok_or_else(|| {
        format!("no grant is held for {cog}@{version}: check it out first, or name the hashes with --sha256")
    })?;
    let mut v: Vec<String> = held["artifacts"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["sha256"].as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    v.sort();
    v.dedup();
    Ok((v, held))
}

async fn approve(ctx: &Ctx<'_>, p: ApproveParams) -> Result<Value, String> {
    let mesh_id = ctx.rt.local.get().ok_or("no local mesh id (kernel.mesh.mesh_nonce)")?.to_hex();
    let ex = ctx.exchange.clone().ok_or("no licence exchange on this node (needs the mesh)")?;
    if p.prepare {
        if p.reapprove_orphaned {
            // The signed envelopes: the CLI checks each signature itself.
            let orphaned = ex.approvals().orphaned_signed();
            return Ok(json!({ "mesh_id": mesh_id, "orphaned_signed": orphaned, "now": ctx.now }));
        }
        let (cog, version) = p.cog_id.as_deref().zip(p.version.as_deref()).ok_or("give <cog>@<version>")?;
        let (sha256, grant) = approval_set(ctx, cog, version, &p.sha256)?;
        // The content key does not depend on `approved_at`: the CLI's
        // `--confirm <content_key>` pins exactly this content.
        let content_key = clawft_kernel::licence::Approval {
            v: 1, mesh_id: mesh_id.clone(), cog_id: cog.into(), version: version.into(),
            sha256: sha256.clone(), approved_at: 0,
        }
        .content_key();
        return Ok(json!({ "mesh_id": mesh_id, "cog_id": cog, "version": version, "sha256": sha256,
                          "grant": grant, "now": ctx.now, "content_key": content_key }));
    }
    if p.signed.is_empty() {
        return Err("workload.cog.checkout.approve needs 'signed' (or 'prepare': true)".into());
    }
    let mut out = Vec::new();
    for s in &p.signed {
        let receipt = ex.issue_approval(s.clone()).await.map_err(|e| format!("approval refused: {e}"))?;
        let a: clawft_kernel::licence::Approval =
            serde_json::from_str(&s.payload).map_err(|e| format!("approval payload: {e}"))?;
        let id = a.content_key();
        chain(ctx.rt, "cog.checkout.approved", json!({
            "approval_id": id, "cog_id": a.cog_id, "version": a.version, "sha256": a.sha256,
            "mesh_id": a.mesh_id, "receipt": format!("{receipt:?}"),
        }));
        out.push(json!({ "approval_id": id, "cog_id": a.cog_id, "version": a.version, "receipt": format!("{receipt:?}") }));
    }
    Ok(json!({ "approved": out }))
}

fn status(ctx: &Ctx<'_>) -> Value {
    let rt = ctx.rt;
    let store = rt.store();
    let approvals = ctx.exchange.as_ref().map(|x| x.approvals().clone());
    let binding = store.active_binding();
    let steward = binding.as_ref().map(|b| b.steward_node_id.clone());
    let is_steward = steward.as_deref() == Some(rt.steward_node_id.as_str());
    let reachable = steward.as_deref().map(|s| {
        if is_steward {
            ctx.mesh.as_ref().is_some_and(|m| m.relay().is_some())
        } else {
            (ctx.reachable)(s)
        }
    });
    let grants: Vec<Value> = store
        .grant_rows()
        .into_iter()
        .map(|g| {
            let gate: Vec<Value> = g
                .artifacts
                .iter()
                .map(|a| {
                    let req = RunRequest { cog_id: &g.cog_id, version: &g.version, sha256: &a.sha256, blake3: &a.blake3 };
                    let verdict = match check_run(store, approvals.as_deref(), &req) {
                        Ok(RunVerdict::Permit(p)) => json!({ "verdict": "permit", "approval_id": p.approval_id }),
                        Ok(RunVerdict::NotSeedBound) => json!({ "verdict": "not_seed_bound" }),
                        Err(e) => json!({ "verdict": e.code(), "reason": e.to_string(),
                                          "remedy": e.remedy(&g.cog_id, &g.version) }),
                    };
                    json!({ "arch": a.arch, "sha256": a.sha256, "run_gate": verdict })
                })
                .collect();
            json!({ "grant": g, "run_gate": gate })
        })
        .collect();
    json!({
        "mesh_id": rt.local.get().map(|m| m.to_hex()),
        "binding": binding.as_ref().map(|b| json!({ "device_id": b.device_id, "seq": b.seq, "state": b.state })),
        "this_node": rt.steward_node_id,
        "steward": steward,
        "is_steward": is_steward,
        "steward_reachable": reachable,
        "relay": licence_steward::status_json(ctx.relay.as_ref()),
        "now": store.effective_now().unwrap_or(ctx.now),
        "grants": grants,
        "approvals": approvals.map(|a| a.rows()).unwrap_or_default(),
        "approval_store": ctx.exchange.is_some(),
    })
}

/// The RPC principal of an extension call: the verified project, else a
/// token (by a short hash, never the secret), else the local operator.
pub fn principal_of(ctx: &crate::rpc_ext::ExtCtx) -> String {
    if let Some(p) = &ctx.verified_project {
        return format!("project:{}", p.as_str());
    }
    match ctx.auth.as_deref() {
        Some(t) if !crate::capability::is_literal_scope(t) => {
            format!("token:{}", &clawft_kernel::licence::sha256_hex(t.as_bytes())[..12])
        }
        _ => "operator".into(),
    }
}

/// `workload.cog.checkout`, registered in `rpc_ext::ROUTES` (Admin: a
/// checkout spends the Seed's licence and transfer budget), so the handler
/// knows the caller's principal.
pub fn handle_ext(call: crate::rpc_ext::ExtCall) -> crate::rpc_ext::ExtFuture {
    Box::pin(async move {
        let principal = principal_of(&call.ctx);
        dispatch_as(&call.method, call.params, call.ctx.kernel.clone(), &principal).await
    })
}

/// Daemon entry (legacy route: approve and status; a checkout normally
/// arrives through [`handle_ext`]).
pub async fn dispatch(
    method: &str,
    params: Value,
    kernel: Arc<tokio::sync::RwLock<clawft_kernel::boot::Kernel<clawft_platform::NativePlatform>>>,
) -> Response {
    dispatch_as(method, params, kernel, "operator").await
}

async fn dispatch_as(
    method: &str,
    params: Value,
    kernel: Arc<tokio::sync::RwLock<clawft_kernel::boot::Kernel<clawft_platform::NativePlatform>>>,
    principal: &str,
) -> Response {
    let Some(rt) = licence_boot::runtime() else {
        return Response::error("the licence runtime is not initialised on this node");
    };
    // Checkout and approve need the cog mesh and the licence exchange, which
    // placement builds; status reports whatever is there.
    if !matches!(method, "workload.cog.checkout.status" | "workload.cog.checkout.list")
        && let Err(e) = crate::workload_place_rpc::ensure_started(kernel.clone()).await
    {
        return Response::error(format!("placement unavailable: {e}"));
    }
    let mesh_rt = kernel.read().await.a2a_router().mesh_runtime().cloned();
    let reachable = move |node: &str| mesh_rt.as_ref().is_some_and(|m| m.peer_licensed(node));
    let arch = clawft_kernel::workload_runtime::native::host_arch();
    let ctx = Ctx {
        rt: &rt,
        mesh: crate::cog_swarm::get(),
        exchange: crate::workload_place_rpc::licence_exchange(),
        relay: licence_steward::state(),
        reachable: &reachable,
        arch,
        now: chrono::Utc::now().timestamp().max(0) as u64,
        principal,
        renewer: licence_steward::renewer(),
    };
    route(&ctx, method, params).await
}

#[cfg(test)]
#[path = "licence_checkout_rpc_tests.rs"]
mod tests;
