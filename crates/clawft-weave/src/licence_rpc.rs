//! `workload.node.bind`, `workload.node.unbind` and `workload.node.binding`
//! (ADR-106 phase 1d).
//!
//! | method                  | capability | what it does                                   |
//! |-------------------------|------------|------------------------------------------------|
//! | `workload.node.binding` | Read       | status: mesh id, held binding, orphaned or not |
//! | `workload.node.bind`    | Admin      | `{seed_node_id, prepare: true}`: the Seed's identity and what the operator signs; `{seed_node_id, signed, grant_fingerprint}`: the steward bind |
//! | `workload.node.unbind`  | Admin      | `{signed}`: an operator-signed `unbound` record |
//! | `workload.node.reset-floor` | Admin  | forget the grant clock high-water mark, restart from now (chained as `floor_reset`) |
//!
//! The daemon never holds an operator key. The operator signs the v2 record
//! with their own key (`weaver workload node bind`); the daemon verifies it
//! under the steward profile ([`SeedBinder::bind_v2`]) and stores it. The
//! operator's signature is the approval, so there is no further governance
//! gate on top of the Admin capability (the capability table in
//! `capability.rs` classifies all three).

use std::sync::Arc;

use clawft_kernel::boot::Kernel;
use clawft_kernel::licence::{AdmissionPosture, SignedBinding};
use clawft_kernel::workload_runtime::{SeedApiRuntime, StewardBind, WorkloadRuntime};
use clawft_platform::NativePlatform;
use clawft_rpc::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::licence_boot::{self, LicenceRuntime};

/// Methods served here.
pub const METHODS: &[&str] = &[
    "workload.node.bind",
    "workload.node.unbind",
    "workload.node.binding",
    "workload.node.reset-floor",
];

/// True for the methods this module serves.
pub fn handles(m: &str) -> bool {
    METHODS.contains(&m)
}

/// Finds the adapter runtime of a Seed by its operator-assigned node id.
pub type SeedLookup<'a> = &'a (dyn Fn(&str) -> Result<SeedApiRuntime, String> + Send + Sync);

/// What one call needs besides its params.
pub struct Ctx<'a> {
    /// The daemon's licence runtime.
    pub rt: &'a LicenceRuntime,
    /// The node's admission state.
    pub posture: AdmissionPosture,
    /// Now, unix seconds.
    pub now: u64,
    /// Where Seeds come from (`workload-seeds.json` in the daemon; a stub in tests).
    pub seed: SeedLookup<'a>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindParams {
    seed_node_id: String,
    #[serde(default)]
    prepare: bool,
    #[serde(default)]
    signed: Option<SignedBinding>,
    #[serde(default)]
    grant_fingerprint: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetParams {
    /// Without it the call only previews.
    #[serde(default)]
    confirm: bool,
    /// With `confirm`: the floor the operator was shown; the reset is refused
    /// if it has changed since.
    #[serde(default)]
    floor: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnbindParams {
    signed: SignedBinding,
}

fn parse<T: serde::de::DeserializeOwned>(m: &str, params: Value) -> Result<T, String> {
    serde_json::from_value(params).map_err(|e| format!("invalid {m} params: {e}"))
}

fn err(e: impl std::fmt::Display) -> Response {
    Response::error(e.to_string())
}

/// Serve one method.
pub async fn route(ctx: &Ctx<'_>, method: &str, params: Value) -> Response {
    match method {
        "workload.node.binding" => Response::success(licence_boot::status(ctx.rt)),
        "workload.node.bind" => match parse::<BindParams>(method, params) {
            Ok(p) => bind(ctx, p).await,
            Err(e) => Response::error(e),
        },
        "workload.node.reset-floor" => match parse::<ResetParams>(method, params) {
            Ok(p) => reset_floor(ctx, p.confirm, p.floor),
            Err(e) => Response::error(e),
        },
        "workload.node.unbind" => match parse::<UnbindParams>(method, params) {
            Ok(p) => unbind(ctx, &p.signed),
            Err(e) => Response::error(e),
        },
        other => Response::error(format!("{other} is not a licence method")),
    }
}

async fn bind(ctx: &Ctx<'_>, p: BindParams) -> Response {
    let rt = ctx.rt;
    let Some(local) = rt.local.get() else {
        return Response::error(
            "no local mesh id: set kernel.mesh.genesis_hash and kernel.mesh.mesh_nonce \
             (weaver mesh nonce generate) and restart the daemon",
        );
    };
    let seed = match (ctx.seed)(&p.seed_node_id) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    if p.prepare {
        // The identity read proves nothing over an unauthenticated link.
        if let Err(e) = seed.link_security().require_pinned(seed.node_id()) {
            return Response::error(e);
        }
        return match seed.identity().await {
            Ok((device_id, device_pubkey)) => {
                let st = licence_boot::status(rt);
                Response::success(json!({
                    "device_id": device_id, "device_pubkey": device_pubkey,
                    "mesh_id": local.to_hex(),
                    "steward_node_id": rt.steward_node_id, "steward_pubkey": rt.steward_pubkey,
                    "next_seq": st["next_seq"], "now": ctx.now,
                }))
            }
            Err(e) => Response::error(format!("could not read the Seed identity: {e}")),
        };
    }
    let (Some(signed), Some(fp)) = (p.signed, p.grant_fingerprint) else {
        return Response::error(
            "workload.node.bind needs 'signed' and 'grant_fingerprint' (or 'prepare': true)",
        );
    };
    let binder = match &rt.binder {
        Ok(b) => b,
        Err(why) => return Response::error(format!("bind state unreadable: {why}")),
    };
    let r = binder
        .bind_v2(&StewardBind {
            signed: &signed,
            rt: &seed,
            store: rt.policy.store(),
            posture: ctx.posture,
            confirmed_fingerprint: &fp,
            steward_node_id: &rt.steward_node_id,
            steward_pubkey: &rt.steward_pubkey,
            now: ctx.now,
        })
        .await;
    match r {
        Ok(rec) => Response::success(json!({ "bound": rec, "grant_fingerprint": fp })),
        Err(e) => Response::error(format!("{e} [{}]", e.code())),
    }
}

fn unbind(ctx: &Ctx<'_>, signed: &SignedBinding) -> Response {
    let binder = match &ctx.rt.binder {
        Ok(b) => b,
        Err(why) => return Response::error(format!("bind state unreadable: {why}")),
    };
    match binder.unbind_v2(signed, ctx.rt.policy.store()) {
        Ok(out) => {
            let mut v = json!({ "unbound": out.record, "save_pending": out.save_pending });
            if out.save_pending {
                v["warning"] = json!(
                    "the unbind is in effect but could not be saved to disk yet; it is retried every minute"
                );
            }
            Response::success(v)
        }
        Err(e) => Response::error(format!("{e} [{}]", e.code())),
    }
}

/// Admin `reset-floor`: forget the clock high-water mark of the bound grant
/// key and restart it from now (the only way to undo a forward clock jump).
/// Without `confirm` it returns the preview (the floor, the floor after, the
/// grants it would revive) and changes nothing. With it, the request is
/// chained with that preview (`licence.floor_reset_requested`), then the store
/// resets and chains `licence.floor_reset`.
fn reset_floor(ctx: &Ctx<'_>, confirm: bool, expected_floor: Option<u64>) -> Response {
    let store = ctx.rt.policy.store();
    if !confirm {
        return match store.floor_preview() {
            Ok(p) => Response::success(json!({ "applied": false, "preview": p })),
            Err(e) => err(e),
        };
    }
    // One critical section: the preview chained is the state that was reset.
    match store.reset_floor_checked(expected_floor) {
        Ok(preview) => {
            ctx.rt.chain.append(
                licence_boot::LICENCE_CHAIN_SOURCE,
                &licence_boot::licence_kind("floor_reset_requested"),
                Some(json!({ "preview": &preview })),
            );
            Response::success(json!({ "applied": true, "preview": preview }))
        }
        Err(e) => err(e),
    }
}

/// Daemon entry: pulls the posture from the kernel and Seeds from the runtime dir.
pub async fn dispatch(
    method: &str,
    params: Value,
    kernel: Arc<RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let Some(rt) = licence_boot::runtime() else {
        if method == "workload.node.binding"
            && let Some(why) = licence_boot::not_installed()
        {
            // The doctor needs to see this, not an error.
            return Response::success(json!({ "installed": false, "nonce_configured": true, "reason": why }));
        }
        return Response::error("the licence runtime is not initialised on this node");
    };
    let posture = {
        let k = kernel.read().await;
        licence_boot::posture(k.kernel_config().mesh.as_ref(), k.governance_gate().is_some())
    };
    let dir = rt.dir.clone();
    let seed = move |id: &str| crate::workload_place_policy::load_seed_runtime(&dir, id);
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    route(&Ctx { rt: &rt, posture, now, seed: &seed }, method, params).await
}

#[cfg(test)]
#[path = "licence_rpc_tests.rs"]
mod tests;
