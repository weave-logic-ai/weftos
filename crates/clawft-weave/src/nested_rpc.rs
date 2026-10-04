//! Owner/admin-only lifecycle for nested user instances. Project nesting is a
//! separate API; project tokens cannot impersonate the instance master here.
use crate::nested_supervisor::{NestedSupervisor, global, install};
use crate::rpc_ext::{ExtCall, ExtFuture, KernelRef};
use anyhow::Context;
use clawft_kernel::parent_policy::{ParentPolicy, export_rules};
use clawft_rpc::Response;
use clawft_types::config::Config;
use serde_json::json;
use std::sync::Arc;

pub async fn init(master: bool, kernel: &KernelRef) -> anyhow::Result<()> {
    if !master {
        return Ok(());
    }
    anyhow::ensure!(
        crate::user_daemon::is_active(),
        "weave.master requires a user daemon"
    );
    let k = kernel.read().await;
    let key = k
        .chain_manager()
        .and_then(|c| c.signing_key_clone())
        .context("nested master needs its user chain")?;
    let depth = crate::nested_boot::active().map_or(0, |c| c.instance.depth);
    install(Arc::new(NestedSupervisor::new(
        crate::protocol::runtime_paths().root().join("nested"),
        std::env::current_exe()?,
        key,
        master,
        depth,
    )?))
}

async fn policy(kernel: &KernelRef) -> anyhow::Result<ParentPolicy> {
    let k = kernel.read().await;
    let snapshot = k
        .governance_gate()
        .and_then(|g| g.governance_snapshot())
        .context("master has no governance snapshot")?;
    let key = k
        .chain_manager()
        .and_then(|c| c.signing_key_clone())
        .context("master has no signing key")?;
    let limits = clawft_kernel::gate::parent_limits_of(k.kernel_config());
    Ok(export_rules(
        snapshot.rules,
        snapshot.risk_threshold,
        snapshot.human_approval_required,
        &limits,
        &key,
        chrono::Utc::now().timestamp_micros().max(1) as u64,
        chrono::Utc::now(),
    )?)
}

/// Call before changing master rules, including reload/update: no inner keeps
/// an old weaker policy while the parent changes. Explicit start re-exports it.
pub async fn quiesce() -> anyhow::Result<()> {
    if let Some(sup) = global() {
        sup.stop_all().await?;
    }
    Ok(())
}

pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        // The route additionally requires Admin. Never broaden child endpoint
        // ceilings to include this prefix or infer ownership from Request.project.
        if call.ctx.verified_project.is_some() {
            return Response::error_with_kind(
                "master_required",
                "project-scoped callers cannot administer nested user instances",
            );
        }
        let Some(sup) = global() else {
            return Response::error_with_kind(
                "master_required",
                "weave.master is not enabled on this user daemon",
            );
        };
        let result: anyhow::Result<serde_json::Value> = async {
            let id = call
                .params
                .get("id")
                .and_then(|v| v.as_str())
                .context("nested method needs id")?;
            match call.method.as_str() {
                "instance.nested.register" => {
                    let config: Config = serde_json::from_value(
                        call.params
                            .get("config")
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                    )?;
                    let cap = serde_json::from_value(
                        call.params.get("cap").cloned().unwrap_or_else(|| json!({})),
                    )?;
                    let inner_key_id = sup
                        .register(id, config, policy(&call.ctx.kernel).await?, cap)
                        .await?;
                    Ok(json!({"id": id, "inner_key_id": inner_key_id, "registration": "isolated"}))
                }
                "instance.nested.start" => {
                    sup.refresh_policy(id, policy(&call.ctx.kernel).await?)
                        .await?;
                    sup.start(id).await
                }
                "instance.nested.stop" => {
                    sup.stop(id).await?;
                    Ok(json!({"id": id, "stopped": true}))
                }
                "instance.nested.grant" => {
                    let grant = serde_json::from_value(
                        call.params
                            .get("registration")
                            .cloned()
                            .context("registration required")?,
                    )?;
                    sup.grant(id, grant).await?;
                    Ok(json!({"id": id, "restart_required": true}))
                }
                "instance.nested.revoke" => {
                    sup.revoke(id).await?;
                    Ok(json!({"id": id, "revoked": true}))
                }
                _ => anyhow::bail!("unknown nested instance method"),
            }
        }
        .await;
        match result {
            Ok(value) => {
                if let Some(chain) = call.ctx.kernel.read().await.chain_manager() {
                    chain.append("governance", &call.method, Some(value.clone()));
                }
                Response::success(value)
            }
            Err(e) => Response::error_with_kind("nested_refused", e.to_string()),
        }
    })
}
