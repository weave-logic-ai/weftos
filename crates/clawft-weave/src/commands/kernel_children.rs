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

/// The user daemon's socket for this uid: `~/.weftos/run/kernel.sock`, or
/// `$WEFTOS_RUNTIME_DIR/kernel.sock` when this process is addressing the
/// user daemon with an overridden runtime dir (`--profile user` /
/// `WEAVER_PROFILE=user` plus `WEFTOS_RUNTIME_DIR`). A runtime dir override
/// without the user profile names some other daemon, not the user daemon.
pub fn user_socket() -> anyhow::Result<PathBuf> {
    let profile = std::env::var("WEAVER_PROFILE").ok();
    let runtime = std::env::var("WEFTOS_RUNTIME_DIR").ok();
    Ok(user_socket_with(
        home_dir().as_deref(),
        crate::user_daemon::is_active() || profile.as_deref() == Some("user"),
        runtime.as_deref(),
    )
    .ok_or_else(|| anyhow::anyhow!("cannot determine the home directory to find the user daemon"))?)
}

/// [`user_socket`] over explicit inputs.
pub fn user_socket_with(
    home: Option<&std::path::Path>,
    user_profile: bool,
    runtime_dir: Option<&str>,
) -> Option<PathBuf> {
    if user_profile
        && let Some(rt) = runtime_dir.map(str::trim).filter(|r| !r.is_empty())
    {
        return Some(clawft_types::runtime_paths::absolutize(std::path::Path::new(rt)).join(SOCKET_NAME));
    }
    home.map(|h| user_runtime_root(h).join(SOCKET_NAME))
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

/// `weaver kernel restart --project <id|name>`.
pub async fn restart_project(project: &str) -> anyhow::Result<()> {
    let v = user_call("project.restart", json!({ "id": project })).await?;
    println!(
        "project {} kernel restarted (pid {}, socket {})",
        v["project_id"].as_str().unwrap_or(project),
        v["pid"],
        v["socket"].as_str().unwrap_or("?"),
    );
    Ok(())
}

/// `weaver kernel stop --project <id|name>`.
pub async fn stop_project(project: &str) -> anyhow::Result<()> {
    let v = user_call("project.stop", json!({ "id": project })).await?;
    let name = v["project_id"].as_str().unwrap_or(project);
    if v["stopped"].as_bool().unwrap_or(false) {
        println!("project {name}: stopped");
    } else if let Some(pid) = v["unmanaged_pid"].as_u64() {
        println!(
            "project {name}: unmanaged kernel pid {pid} ({}); it is not supervised and was not signalled",
            v["unmanaged_reason"].as_str().unwrap_or("not adopted")
        );
    } else {
        println!("project {name}: was not running");
    }
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

/// The NOTE cell of the children table: the failure reason, or the stale-build
/// warning (a child outlives `weaver update`, so it can run an older binary).
fn child_note(c: &Value) -> String {
    if let Some(why) = c["failed_reason"].as_str() {
        return why.to_owned();
    }
    if c["stale_build"].as_bool().unwrap_or(false) {
        return format!(
            "stale build {}; `weaver kernel restart --project {}`",
            c["kernel_sha"].as_str().map_or("?", |s| &s[..s.len().min(12)]),
            c["project_id"].as_str().unwrap_or("<id>")
        );
    }
    String::new()
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
            child_note(c),
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
    let Ok(sock) = user_socket() else { return Ok(()) };
    let manifests = home_dir().map(|h| crate::user_daemon::manifests_dir(&h));
    legacy_guard_with(legacy_flag, &RuntimePaths::resolve(), &sock, manifests.as_deref()).await
}

/// [`legacy_guard`] over explicit paths (tests).
pub async fn legacy_guard_with(
    legacy_flag: bool,
    paths: &RuntimePaths,
    user_socket: &std::path::Path,
    manifests_dir: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let RootSource::Project(root) = paths.source() else {
        return Ok(());
    };
    // A project the owner migrated (`via = child-kernel`) runs under the
    // user daemon. A second kernel over the same tree would split its
    // history and its governance, so no flag lifts this refusal; the way
    // back is `migrate-kernel --revert`.
    if let Some(m) = manifests_dir
        .and_then(|d| clawft_types::project::find_by_root(d, root).ok().flatten())
        .filter(|m| {
            m.state == clawft_types::project::ProjectState::Active
                && m.serve.as_ref().is_some_and(|s| s.via == clawft_types::project::ServeVia::ChildKernel)
        })
    {
        anyhow::bail!(
            "this project ({id}) was migrated to a child kernel under the user daemon; a second \
             kernel over it would split its history and governance, so --legacy-project-daemon does \
             not apply.\n  run it with `weaver kernel start --project {id}`, or undo the migration first: \
             `weaver kernel stop --project {id}`, then `weaver project migrate-kernel {id} --revert`",
            id = m.id
        );
    }
    if DaemonClient::connect_path(user_socket).await.is_none() {
        return Ok(());
    }
    if legacy_flag {
        eprintln!(
            "warning: a project-rooted daemon beside the user daemon is deprecated and goes away \
             next release; use `weaver project migrate-kernel`"
        );
        return Ok(());
    }
    anyhow::bail!(
        "a user daemon is running for this user; a project-rooted daemon here would run beside it.\n  \
         run this project's kernel under the user daemon: `weaver project migrate-kernel <id>` once, then \
         `weaver kernel start --project <id>`,\n  or pass --legacy-project-daemon to start the old kind \
         anyway (deprecated, removed next release)"
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    struct W {
        _t: tempfile::TempDir,
        home: PathBuf,
        proj: PathBuf,
        sock: PathBuf,
    }

    fn world() -> W {
        let t = tempfile::Builder::new().prefix("kc").tempdir_in("/tmp").unwrap();
        let home = t.path().join("home");
        let proj = home.join("work/app");
        std::fs::create_dir_all(proj.join(".weftos/runtime")).unwrap();
        std::fs::create_dir_all(home.join(".weftos/run")).unwrap();
        let sock = home.join(".weftos/run/kernel.sock");
        W { _t: t, home, proj, sock }
    }

    fn project_paths(w: &W) -> RuntimePaths {
        let p = RuntimePaths::resolve_with(None, Some(&w.proj), Some(&w.home));
        assert!(matches!(p.source(), RootSource::Project(_)), "{:?}", p.source());
        p
    }

    #[test]
    fn the_user_socket_honours_a_runtime_dir_override_only_for_the_user_profile() {
        let home = std::path::Path::new("/h");
        assert_eq!(user_socket_with(Some(home), false, None).unwrap(), PathBuf::from("/h/.weftos/run/kernel.sock"));
        // An override without the user profile names some other daemon.
        assert_eq!(user_socket_with(Some(home), false, Some("/x")).unwrap(), PathBuf::from("/h/.weftos/run/kernel.sock"));
        assert_eq!(user_socket_with(Some(home), true, Some("/x")).unwrap(), PathBuf::from("/x/kernel.sock"));
        assert_eq!(user_socket_with(Some(home), true, Some("  ")).unwrap(), PathBuf::from("/h/.weftos/run/kernel.sock"));
        assert!(user_socket_with(None, false, None).is_none());
    }

    #[tokio::test]
    async fn a_plain_start_in_a_project_beside_a_user_daemon_is_refused_with_the_migration_hint() {
        let w = world();
        let _daemon = std::os::unix::net::UnixListener::bind(&w.sock).unwrap();
        let e = legacy_guard_with(false, &project_paths(&w), &w.sock, None).await.unwrap_err().to_string();
        assert!(e.contains("migrate-kernel") && e.contains("--legacy-project-daemon"), "{e}");
    }

    #[tokio::test]
    async fn the_legacy_flag_a_missing_user_daemon_and_other_roots_do_not_block() {
        let w = world();
        // No user daemon: a single-project user is not locked out.
        assert!(legacy_guard_with(false, &project_paths(&w), &w.sock, None).await.is_ok());
        let _daemon = std::os::unix::net::UnixListener::bind(&w.sock).unwrap();
        // The explicit flag wins.
        assert!(legacy_guard_with(true, &project_paths(&w), &w.sock, None).await.is_ok());
        // An explicit runtime dir (env) or the legacy home is not "inside a project".
        let env = RuntimePaths::resolve_with(Some("/some/runtime"), Some(&w.proj), Some(&w.home));
        assert!(legacy_guard_with(false, &env, &w.sock, None).await.is_ok());
        let outside = RuntimePaths::resolve_with(None, Some(&w.home), Some(&w.home));
        assert!(legacy_guard_with(false, &outside, &w.sock, None).await.is_ok());
    }

    fn register(w: &W, via: clawft_types::project::ServeVia) -> (PathBuf, String) {
        use clawft_types::project::{ProjectManifest, ServeSection, write_manifest};
        let dir = w.home.join(".weftos/projects");
        std::fs::create_dir_all(&dir).unwrap();
        let id = "01J0000000000000000000000A".to_owned();
        let now = chrono::Utc::now();
        let m = ProjectManifest {
            schema_version: 1,
            id: id.clone(),
            name: "app".into(),
            root: w.proj.canonicalize().unwrap(),
            state: Default::default(),
            created: now,
            last_seen: now,
            project_toml: Default::default(),
            seed: None,
            legacy: None,
            serve: Some(ServeSection { via, ..Default::default() }),
            chain: None,
            binary: None,
            extra: Default::default(),
        };
        write_manifest(&dir, &m).unwrap();
        (dir, id)
    }

    #[tokio::test]
    async fn a_migrated_project_refuses_a_second_kernel_even_with_the_flag_and_no_user_daemon() {
        use clawft_types::project::ServeVia;
        let w = world();
        let (dir, id) = register(&w, ServeVia::ChildKernel);
        // No user daemon is listening and the flag is passed: still refused.
        for flag in [false, true] {
            let e = legacy_guard_with(flag, &project_paths(&w), &w.sock, Some(&dir))
                .await
                .unwrap_err()
                .to_string();
            assert!(e.contains(&format!("kernel start --project {id}")) && e.contains("--revert"), "{e}");
        }
        // An archived entry (left by `init --fork --force`) is not a migrated project.
        let mut m = clawft_types::project::find_by_id(&dir, &id).unwrap().unwrap();
        m.state = clawft_types::project::ProjectState::Archived;
        clawft_types::project::write_manifest(&dir, &m).unwrap();
        assert!(legacy_guard_with(true, &project_paths(&w), &w.sock, Some(&dir)).await.is_ok());
        // A project served the ordinary way is not blocked by the marker check.
        let (dir, _) = register(&w, ServeVia::UserDaemon);
        assert!(legacy_guard_with(true, &project_paths(&w), &w.sock, Some(&dir)).await.is_ok());
        assert!(legacy_guard_with(false, &project_paths(&w), &w.sock, Some(&dir)).await.is_ok());
    }
}
