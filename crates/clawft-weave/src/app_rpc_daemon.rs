//! Kernel-facing half of `app_rpc`: dispatch plus `app.start` /
//! `app.stop`, which need the supervisor to run and stop agent loops.

use std::sync::Arc;

use clawft_kernel::boot::Kernel;
use clawft_kernel::{AppManager, AppState, Pid};
use clawft_platform::NativePlatform;
use clawft_rpc::Response;
use serde_json::{Value, json};
use tokio::sync::RwLock;

use super::{
    APP_START, APP_STOP, AppGate, gate_check, handle_inspect, handle_install, handle_list,
    handle_remove, name_param,
};
use crate::rpc_gate::Audit;

type KernelRef = Arc<RwLock<Kernel<NativePlatform>>>;

/// Route one `app.*` call against the live kernel.
pub async fn dispatch(method: &str, params: Value, kernel: KernelRef) -> Response {
    let k = kernel.read().await;
    let mgr = Arc::clone(k.app_manager());
    #[cfg(feature = "exochain")]
    let (gate, chain) = (k.governance_gate().cloned(), k.chain_manager().cloned());
    #[cfg(feature = "exochain")]
    let gate_ref: AppGate<'_> = gate.as_deref();
    #[cfg(not(feature = "exochain"))]
    let gate_ref: AppGate<'_> = None;
    let audit = |kind: &str, payload: Value| {
        #[cfg(feature = "exochain")]
        if let Some(cm) = &chain {
            cm.append("app", kind, Some(payload));
        }
        #[cfg(not(feature = "exochain"))]
        let _ = (kind, payload);
    };
    match method {
        "app.install" => handle_install(&mgr, &params, gate_ref, &audit),
        "app.list" => handle_list(&mgr),
        "app.inspect" => handle_inspect(&mgr, &params),
        "app.remove" => handle_remove(&mgr, &params, gate_ref, &audit),
        "app.start" => start(&k, &mgr, &params, gate_ref, &audit),
        "app.stop" => stop(&k, &mgr, &params, gate_ref, &audit),
        other => Response::error(format!("unknown method: {other}")),
    }
}

/// Spawn one app agent as a real kernel agent loop (same loop
/// `agent.spawn` runs), gated by the kernel gate.
fn spawn_agent(k: &Kernel<NativePlatform>, req: clawft_kernel::SpawnRequest) -> Result<Pid, String> {
    let a2a = k.a2a_router().clone();
    let cron = k.cron_service().clone();
    let pt = k.process_table().clone();
    #[cfg(feature = "exochain")]
    let chain = k.chain_manager().cloned();
    #[cfg(feature = "exochain")]
    let gate = k.governance_gate().cloned();
    k.supervisor()
        .spawn_and_run(req, move |pid, cancel| {
            let inbox = a2a.create_inbox(pid);
            async move {
                clawft_kernel::agent_loop::kernel_agent_loop(
                    pid,
                    cancel,
                    inbox,
                    a2a,
                    cron,
                    pt,
                    None,
                    #[cfg(feature = "exochain")]
                    chain,
                    #[cfg(feature = "exochain")]
                    gate,
                )
                .await
            }
        })
        .map(|r| r.pid)
        .map_err(|e| e.to_string())
}

fn start(
    k: &Kernel<NativePlatform>,
    mgr: &AppManager,
    params: &Value,
    gate: AppGate<'_>,
    audit: Audit<'_>,
) -> Response {
    let name = match name_param(params, "app.start") {
        Ok(n) => n,
        Err(e) => return Response::error(e),
    };
    let ctx = json!({ "app_name": &name, "effect": { "risk": 0.3, "security": 0.2 } });
    if let Err(e) = gate_check(gate, APP_START, &ctx) {
        return Response::error(e);
    }
    let requests = match mgr.start(&name) {
        Ok(r) => r,
        Err(e) => return Response::error(e.to_string()),
    };
    let mut pids = Vec::with_capacity(requests.len());
    for req in requests {
        let agent_id = req.agent_id.clone();
        match spawn_agent(k, req) {
            Ok(pid) => {
                let _ = mgr.add_agent_pid(&name, pid);
                pids.push(pid);
            }
            Err(e) => {
                // Roll back: stop what started, then Running -> Stopping -> Failed.
                for pid in &pids {
                    let _ = k.supervisor().stop(*pid, true);
                }
                let reason = format!("agent {agent_id} failed to spawn: {e}");
                let _ = mgr.transition_to(&name, AppState::Stopping);
                let _ = mgr.transition_to(&name, AppState::Failed(reason.clone()));
                return Response::error(format!("app.start failed: {reason}"));
            }
        }
    }
    let services = mgr.inspect(&name).map(|a| a.manifest.services.len()).unwrap_or(0);
    audit(APP_START, json!({ "app_name": &name, "agent_pids": &pids }));
    Response::success(json!({
        "name": name,
        "state": "running",
        "agent_pids": pids,
        // Sidecar services are not launched by this path yet (ADR-099 card 09).
        "services_not_started": services,
    }))
}

fn stop(
    k: &Kernel<NativePlatform>,
    mgr: &AppManager,
    params: &Value,
    gate: AppGate<'_>,
    audit: Audit<'_>,
) -> Response {
    let name = match name_param(params, "app.stop") {
        Ok(n) => n,
        Err(e) => return Response::error(e),
    };
    let app = match mgr.inspect(&name) {
        Ok(a) => a,
        Err(e) => return Response::error(e.to_string()),
    };
    if app.state != AppState::Running {
        return Response::error(format!("app '{name}' is {}, not running", app.state));
    }
    let ctx = json!({ "app_name": &name, "effect": { "risk": 0.2, "security": 0.1 } });
    if let Err(e) = gate_check(gate, APP_STOP, &ctx) {
        return Response::error(e);
    }
    for pid in &app.agent_pids {
        let _ = k.supervisor().stop(*pid, true);
    }
    match mgr.stop(&name) {
        Ok(()) => {
            audit(APP_STOP, json!({ "app_name": &name, "agent_pids": &app.agent_pids }));
            Response::success(json!({ "name": name, "state": "stopped", "stopped_pids": app.agent_pids }))
        }
        Err(e) => Response::error(e.to_string()),
    }
}
