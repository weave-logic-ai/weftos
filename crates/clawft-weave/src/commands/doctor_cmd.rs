//! `weaver doctor`: install, daemon, runtime and MCP checks.
//!
//! Delegates to [`clawft_rpc::doctor`], the same engine as `weft doctor`.
//! The `config` and `agents` components need the agent config and stay in
//! `weft doctor`; naming them here is an error that says so.

use clap::Args;
use clawft_rpc::doctor::{self, Component, DoctorEnv, Options};

/// Arguments for `weaver doctor`.
#[derive(Args, Debug, Default)]
pub struct DoctorArgs {
    /// Limit to these components (also accepted as `--component`).
    #[arg(value_name = "COMPONENT", value_parser = clap::builder::PossibleValuesParser::new(["install", "daemon", "runtime", "mcp"]))]
    pub only: Vec<String>,

    /// Component(s) to check: install, daemon, runtime, mcp.
    #[arg(long, value_delimiter = ',', value_parser = clap::builder::PossibleValuesParser::new(["install", "daemon", "runtime", "mcp"]))]
    pub component: Vec<String>,

    /// Treat warnings as failures (non-zero exit).
    #[arg(long, default_value_t = false)]
    pub strict: bool,

    /// Machine-readable JSON output.
    #[arg(long, default_value_t = false)]
    pub json: bool,

    /// Apply safe local repairs (remove provably stale socket/pid files in the
    /// ACTIVE runtime dir) and report exactly what changed. Off by default.
    #[arg(long, default_value_t = false)]
    pub fix: bool,

    /// With --fix, also repair every runtime dir doctor can see (~/.clawft,
    /// ~/.weftos/runtime, ancestor .weftos/runtime), not just the active one.
    /// Setting WEFTOS_RUNTIME_DIR restricts doctor to that single dir.
    #[arg(long, default_value_t = false)]
    pub all_runtimes: bool,
}

impl DoctorArgs {
    /// Selected components. Empty means every component `weaver` can run.
    pub fn components(&self) -> anyhow::Result<Vec<Component>> {
        let names: Vec<String> = self.only.iter().chain(&self.component).cloned().collect();
        let mut picked = doctor::parse_components(&names).map_err(anyhow::Error::msg)?;
        if picked.is_empty() {
            picked = vec![Component::Install, Component::Daemon, Component::Runtime, Component::Mcp];
        }
        Ok(picked)
    }
}

/// Run `weaver doctor`.
pub async fn run(args: DoctorArgs) -> anyhow::Result<()> {
    let opts = Options {
        components: args.components()?,
        fix: args.fix,
        all_runtimes: args.all_runtimes,
        self_version: env!("BUILD_VERSION").to_string(),
    };
    let env = DoctorEnv::detect();
    let runtime_wanted = opts.components.contains(&Component::Runtime);
    let home = env.home.clone();
    let install_wanted = opts.components.contains(&Component::Install);
    let config_dir = env.config_dir.clone();
    let mut report = tokio::task::spawn_blocking(move || doctor::run_system(&env, &opts)).await?;
    if runtime_wanted {
        // user.key / chain.key during the D-5 transition (compares pubkeys).
        report.findings.extend(crate::user_key::doctor_findings(&home));
    }
    #[cfg(all(unix, feature = "mesh"))]
    {
        if runtime_wanted {
            let probe = crate::mesh_doctor::gather(&home).await;
            report.findings.extend(crate::mesh_doctor::findings(&probe, &home));
            let (profile, mesh) = daemon_mesh().await.unwrap_or_default();
            report.findings.extend(crate::mesh_doctor::mode_findings(profile.as_deref(), mesh.as_ref(), &probe));
        }
        if install_wanted {
            report.findings.extend(service_tier_findings(&config_dir));
        }
    }
    #[cfg(not(all(unix, feature = "mesh")))]
    let _ = (install_wanted, &config_dir);
    // ADR-106: mesh_nonce, binding orphaned, mesh id unset with a binding.
    #[cfg(all(unix, feature = "placement"))]
    if runtime_wanted && let Some(st) = crate::licence_doctor::gather().await {
        report.findings.extend(crate::licence_doctor::findings(&st));
    }
    // ADR-106 phase 3: missing approvals, expiring grants, no steward reachable.
    #[cfg(all(unix, feature = "placement"))]
    if runtime_wanted && let Some(st) = crate::licence_doctor::gather_checkout().await {
        report.findings.extend(crate::licence_doctor::checkout_findings(&st));
    }
    let code = doctor::print_report(&report, "weaver doctor", args.json, args.strict);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// The answering daemon's profile and mesh mode, from `kernel.status` (None:
/// no daemon answered within 3 s). The daemon is whichever this directory
/// resolves to, which may be a project daemon.
#[cfg(all(unix, feature = "mesh"))]
async fn daemon_mesh() -> Option<(Option<String>, Option<clawft_rpc::handshake::MeshHandshake>)> {
    let ask = async {
        let mut client = crate::client::DaemonClient::connect().await?;
        let resp = client.simple_call("kernel.status").await.ok()?;
        let status: crate::protocol::KernelStatusResult = serde_json::from_value(resp.result?).ok()?;
        let h = status.handshake?;
        Some((h.profile, h.mesh))
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), ask).await.ok().flatten()
}

/// Service tier skew: the service's own `service.json` wins over the receipt.
#[cfg(all(unix, feature = "mesh"))]
fn service_tier_findings(config_dir: &std::path::Path) -> Vec<clawft_rpc::doctor::Finding> {
    use crate::install_tiers::{parse_tiers, skew_findings, ServiceObserved};
    let receipt = std::fs::read_to_string(config_dir.join("weftos/weftos-receipt.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null);
    // The same record the mesh checks and the daemon read: beside the socket
    // ($WEFTOS_MESH_SOCKET honoured).
    let sock = crate::commands::mesh_cmd::socket_of(&crate::commands::mesh_cmd::ConnArgs::default());
    let record = sock
        .parent()
        .unwrap_or_else(|| std::path::Path::new(crate::service_units_system::RUN_DIR))
        .join("service.json");
    let observed = clawft_mesh_local::proto::ServiceRecord::load(&record)
        .ok()
        .map(|r| ServiceObserved { build_sha: r.build_sha });
    skew_findings(&parse_tiers(&receipt), observed.as_ref(), env!("BUILD_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_runs_system_components_only() {
        let c = DoctorArgs::default().components().unwrap();
        assert_eq!(c, vec![Component::Install, Component::Daemon, Component::Runtime, Component::Mcp]);
    }

    #[test]
    fn positional_and_flag_merge() {
        let a = DoctorArgs { only: vec!["install".into()], component: vec!["daemon".into()], ..Default::default() };
        assert_eq!(a.components().unwrap(), vec![Component::Install, Component::Daemon]);
    }
}
