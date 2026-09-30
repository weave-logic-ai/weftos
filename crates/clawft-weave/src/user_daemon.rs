//! The per-user daemon profile (ADR-103 Phase 1, package D).
//!
//! `weaver kernel start --profile user` runs the collapsed `machine` + `user`
//! roles with runtime root `~/.weftos/run` ([`RootSource::User`]), one per
//! uid (the `kernel.lock` in that root). Configuration comes from
//! `~/.weftos/weave.toml` layered over the legacy `~/.clawft/config.json`
//! (plan D-2). The default `kernel start` behaviour is unchanged.
//!
//! The profile is process state, set once by the CLI ([`enter`]) so the
//! socket, pid, boot and chain choice all resolve the same root.
//!
//! [`RootSource::User`]: clawft_types::runtime_paths::RootSource::User

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use clawft_platform::{Platform, config_loader};
use clawft_types::config::Config;
use clawft_types::runtime_paths::{home_dir, set_user_profile, user_weftos_dir};

use crate::commands::LoadedConfig;

/// Profile name accepted by `--profile` / `WEAVER_PROFILE`.
pub const PROFILE_USER: &str = "user";

/// Roles the user daemon runs (collapsed machine + user, ADR-103 roles table).
pub const USER_ROLES: [&str; 2] = ["machine", "user"];

/// Parse a `--profile` value. `None` and `default` mean the existing
/// project/legacy behaviour.
pub fn parse_profile(value: Option<&str>) -> Result<Option<&'static str>, String> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None | Some("default") => Ok(None),
        Some(PROFILE_USER) => Ok(Some(PROFILE_USER)),
        Some(other) => Err(format!(
            "unknown profile {other:?}: expected `user` (or omit for the default)"
        )),
    }
}

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Activate the user profile for this process: every later
/// `RuntimePaths::resolve()` returns the user root.
pub fn enter() {
    set_user_profile(true);
    ACTIVE.store(true, Ordering::SeqCst);
}

/// True when this process runs or addresses the user daemon.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// Profile and roles to report in the handshake (`None` for the default daemon).
pub fn handshake_profile() -> Option<(String, Vec<String>)> {
    is_active().then(|| {
        (
            PROFILE_USER.to_owned(),
            USER_ROLES.iter().map(|r| (*r).to_owned()).collect(),
        )
    })
}

/// `<home>/.weftos/weave.toml`.
pub fn user_weave_toml(home: &Path) -> PathBuf {
    user_weftos_dir(home).join("weave.toml")
}

/// `<home>/.weftos/projects`, the manifest store.
pub fn manifests_dir(home: &Path) -> PathBuf {
    clawft_rpc::resolve::manifests_dir(home)
}

/// The local uid as a string (Unix), reported unverified in the handshake.
pub fn local_uid() -> Option<String> {
    #[cfg(unix)]
    {
        Some(nix::unistd::getuid().as_raw().to_string())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Create `~/.weftos` and the runtime root, and make `~/.weftos` the working
/// directory so no project-local `weave.toml` or `.clawft/` overlay leaks into
/// the daemon's config.
pub fn prepare_home(home: &Path) -> anyhow::Result<()> {
    let weftos = user_weftos_dir(home);
    std::fs::create_dir_all(weftos.join("run"))?;
    std::env::set_current_dir(&weftos)?;
    Ok(())
}

/// Layer `weave` (from `~/.weftos/weave.toml`) over the legacy layers.
///
/// Pure so it can be tested without a filesystem: the JSON layers are the
/// base, `weave` wins on conflict, the workspace overlay (if any) stays
/// split for the permission ceiling exactly as in `load_config_layered`.
pub fn layer_user_config(
    layers: config_loader::ConfigLayers,
    weave: &serde_json::Value,
) -> anyhow::Result<LoadedConfig> {
    let mut global_value = layers.global.clone();
    config_loader::deep_merge(&mut global_value, weave);
    let global: Config = serde_json::from_value(global_value.clone())?;
    let workspace_routing = match layers.workspace.clone() {
        Some(ws) => Some(serde_json::from_value::<Config>(ws)?.routing),
        None => None,
    };
    let merged = config_loader::ConfigLayers {
        global: global_value,
        workspace: layers.workspace,
    }
    .merged();
    Ok(LoadedConfig {
        config: serde_json::from_value(merged)?,
        global_routing: global.routing,
        workspace_routing,
    })
}

/// Load the user daemon's config. An explicit `--config` file is used as-is
/// (the same as every other profile); otherwise the legacy layers with
/// `~/.weftos/weave.toml` on top (D-2).
pub async fn load_user_config<P: Platform>(
    platform: &P,
    config_override: Option<&str>,
    home: &Path,
) -> anyhow::Result<LoadedConfig> {
    if config_override.is_some() {
        return crate::commands::load_config_layered(platform, config_override).await;
    }
    let layers = config_loader::load_config_layers(platform.fs(), platform.env())
        .await
        .map_err(|e| anyhow::anyhow!("failed to load config: {e}"))?;
    let path = user_weave_toml(home);
    let weave = config_loader::load_weave_toml_file(platform.fs(), &path)
        .await
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    layer_user_config(layers, &weave)
}

/// The home directory, or an error naming what is missing.
pub fn require_home() -> anyhow::Result<PathBuf> {
    home_dir().ok_or_else(|| {
        anyhow::anyhow!("cannot determine the home directory for --profile user")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profile_flag_parsing() {
        assert_eq!(parse_profile(None), Ok(None));
        assert_eq!(parse_profile(Some("")), Ok(None));
        assert_eq!(parse_profile(Some("default")), Ok(None));
        assert_eq!(parse_profile(Some("user")), Ok(Some("user")));
        assert!(parse_profile(Some("root")).unwrap_err().contains("unknown profile"));
    }

    #[test]
    fn weave_toml_wins_over_legacy_json_and_keeps_the_rest() {
        let layers = config_loader::ConfigLayers {
            global: json!({"kernel": {"max_processes": 7, "mesh": {"enabled": false}}}),
            workspace: None,
        };
        let weave = json!({"kernel": {"mesh": {"enabled": true, "listen_addr": "0.0.0.0:9470"}}});
        let loaded = layer_user_config(layers, &weave).unwrap();
        assert_eq!(loaded.config.kernel.max_processes, 7);
        let mesh = loaded.config.kernel.mesh.expect("mesh from weave.toml");
        assert!(mesh.enabled);
        assert_eq!(mesh.listen_addr, "0.0.0.0:9470");
    }

    #[test]
    fn empty_weave_toml_is_the_legacy_config() {
        let layers = config_loader::ConfigLayers {
            global: json!({"kernel": {"max_processes": 9}}),
            workspace: None,
        };
        let loaded = layer_user_config(layers, &json!({})).unwrap();
        assert_eq!(loaded.config.kernel.max_processes, 9);
        assert!(loaded.workspace_routing.is_none());
    }

    #[test]
    fn user_paths_hang_off_home_weftos() {
        let home = Path::new("/h");
        assert_eq!(user_weave_toml(home), Path::new("/h/.weftos/weave.toml"));
        assert_eq!(manifests_dir(home), Path::new("/h/.weftos/projects"));
    }
}
