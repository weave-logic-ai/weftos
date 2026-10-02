//! `weaver kernel` pieces that address per-project child kernels through
//! the user daemon (ADR-103 A6, Phase 2 package G): `start --project`,
//! `stop --project|--all-children`, the children list in `status`, and the
//! refusal to start a legacy project-rooted daemon beside a user daemon.

use std::path::PathBuf;

use clawft_rpc::{DaemonClient, Request};
use clawft_types::runtime_paths::{RootSource, RuntimePaths, SOCKET_NAME, home_dir, user_runtime_root};
use comfy_table::{Table, presets};
use serde_json::{Value, json};

/// The hint printed when the user daemon is not running.
pub const NO_USER_DAEMON: &str =
    "user daemon not running; run `weaver kernel start --profile user`";

/// The user daemon's socket for this uid.
pub fn user_socket() -> anyhow::Result<PathBuf> {
    let home = home_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot determine the home directory to find the user daemon"))?;
    Ok(user_runtime_root(&home).join(SOCKET_NAME))
}

/// One call to the user daemon. Never starts it.
pub async fn user_call(method: &str, params: Value) -> anyhow::Result<Value> {
    let sock = user_socket()?;
    let Some(mut client) = DaemonClient::connect_path(&sock).await else {
        anyhow::bail!("{NO_USER_DAEMON}");
    };
    let resp = client.call(Request::with_params(method, params)).await?;
    if resp.ok {
        Ok(resp.result.unwrap_or(Value::Null))
    } else {
        let kind = resp.error_kind.as_deref().unwrap_or("error");
        anyhow::bail!("{kind}: {}", resp.error.unwrap_or_else(|| "unknown error".into()))
    }
}

/// `weaver kernel start --project <id|name>`.
pub async fn start_project(project: &str) -> anyhow::Result<()> {
    let v = user_call("project.start", json!({ "id": project })).await?;
    println!(
        "project {} kernel {} (pid {}, socket {})",
        v["project_id"].as_str().unwrap_or(project),
        if v["started"].as_bool().unwrap_or(false) { "started" } else { "already running" },
        v["pid"],
        v["socket"].as_str().unwrap_or("?"),
    );
    Ok(())
}

/// `weaver kernel stop --project <id|name>`.
pub async fn stop_project(project: &str) -> anyhow::Result<()> {
    let v = user_call("project.stop", json!({ "id": project })).await?;
    println!(
        "project {}: {}",
        v["project_id"].as_str().unwrap_or(project),
        if v["stopped"].as_bool().unwrap_or(false) { "stopped" } else { "was not running" }
    );
    Ok(())
}

/// `weaver kernel stop --all-children`: returns the ids stopped.
pub async fn stop_all_children() -> anyhow::Result<Vec<String>> {
    let v = user_call("project.stop_all", json!({})).await?;
    Ok(v["stopped"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default())
}

/// Stop the children before the user daemon itself goes down (the default
/// cascade). Best effort: a daemon that cannot be asked is stopped anyway.
pub async fn cascade_before_user_stop(keep_children: bool) {
    if keep_children {
        println!("--keep-children: project kernels are left running (the next user daemon adopts them)");
        return;
    }
    match stop_all_children().await {
        Ok(ids) if ids.is_empty() => {}
        Ok(ids) => println!("stopped {} project kernel(s): {}", ids.len(), ids.join(", ")),
        Err(e) => eprintln!("warning: could not stop project kernels first: {e}"),
    }
}

/// The children table for `weaver kernel status --profile user`.
pub async fn print_children() {
    let Ok(v) = user_call("project.status", json!({})).await else { return };
    let children = v["children"].as_array().cloned().unwrap_or_default();
    let leftovers = v["unverifiable"].as_array().cloned().unwrap_or_default();
    if children.is_empty() && leftovers.is_empty() {
        println!("\nproject kernels: none");
        return;
    }
    let mut t = Table::new();
    t.load_preset(presets::UTF8_FULL_CONDENSED);
    t.set_header(["PROJECT", "STATE", "PID", "RESTARTS", "NOTE"]);
    for c in &children {
        t.add_row([
            c["project_id"].as_str().unwrap_or("?").to_owned(),
            c["state"].as_str().unwrap_or("?").to_owned(),
            c["pid"].as_u64().map_or("-".into(), |p| p.to_string()),
            c["restarts"].to_string(),
            c["failed_reason"].as_str().unwrap_or("").to_owned(),
        ]);
    }
    println!("\nproject kernels:\n{t}");
    for l in &leftovers {
        println!(
            "  unverified leftover {} (pid {}): {} (not adopted, not signalled)",
            l["project_id"].as_str().unwrap_or("?"),
            l["pid"],
            l["reason"].as_str().unwrap_or("?")
        );
    }
}

/// Refuse a plain `weaver kernel start` inside a project while this uid's
/// user daemon is running, unless `--legacy-project-daemon` (one release of
/// deprecation, ADR-103 A6 decision 7).
pub async fn legacy_guard(legacy_flag: bool) -> anyhow::Result<()> {
    if legacy_flag {
        eprintln!(
            "warning: --legacy-project-daemon starts a project-rooted daemon beside the user daemon; \
             this is deprecated and goes away next release"
        );
        return Ok(());
    }
    let paths = RuntimePaths::resolve();
    if !matches!(paths.source(), RootSource::Project(_)) {
        return Ok(());
    }
    let Ok(sock) = user_socket() else { return Ok(()) };
    if DaemonClient::connect_path(&sock).await.is_none() {
        return Ok(());
    }
    anyhow::bail!(
        "a user daemon is running for this user; a project-rooted daemon here would run beside it.\n  \
         run this project's kernel under the user daemon: `weaver project migrate-kernel <id>` once, then \
         `weaver kernel start --project <id>`,\n  or pass --legacy-project-daemon to start the old kind \
         anyway (deprecated, removed next release)"
    )
}
