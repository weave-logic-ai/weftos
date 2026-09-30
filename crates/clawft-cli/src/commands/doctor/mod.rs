//! `weft doctor`: install, daemon, runtime, config, MCP and multi-agent checks.
//!
//! The install/daemon/runtime/mcp checks live in [`clawft_rpc::doctor`] and
//! are shared verbatim with `weaver doctor`. This module adds the checks that
//! need the loaded [`Config`] (`config`, `agents`; see [`agents`]).
//!
//! Read-only unless `--fix`, which only removes provably stale socket and pid
//! files and reports each change. Exit code is 1 on any FAIL; `--strict` also
//! fails on WARN.

pub mod agents;

use clap::Args;
use clawft_platform::NativePlatform;
use clawft_rpc::doctor::{
    self, Component, DoctorEnv, Finding, Options, Report, Severity,
};

use super::{discover_config_path, load_config};

pub use agents::{claude_binary_on_path, multi_agent_findings};

/// Arguments for the `weft doctor` subcommand.
#[derive(Args, Debug, Default)]
pub struct DoctorArgs {
    /// Limit to these components (also accepted as `--component`).
    #[arg(value_name = "COMPONENT", value_parser = clap::builder::PossibleValuesParser::new(Component::NAMES))]
    pub only: Vec<String>,

    /// Component(s) to check: install, daemon, runtime, config, mcp, agents.
    #[arg(long, value_delimiter = ',', value_parser = clap::builder::PossibleValuesParser::new(Component::NAMES))]
    pub component: Vec<String>,

    /// Config file path (overrides auto-discovery).
    #[arg(short, long)]
    pub config: Option<String>,

    /// Treat warnings as failures (non-zero exit).
    #[arg(long, default_value_t = false)]
    pub strict: bool,

    /// Only run multi-agent related checks (same as `--component agents`).
    #[arg(long, default_value_t = false)]
    pub multi_agent: bool,

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
    /// Selected components; empty means all.
    pub fn components(&self) -> anyhow::Result<Vec<Component>> {
        let mut names: Vec<String> = self.only.iter().chain(&self.component).cloned().collect();
        if self.multi_agent {
            names.push("agents".into());
        }
        doctor::parse_components(&names).map_err(anyhow::Error::msg)
    }
}

/// `config` + `agents` findings for the selected components.
async fn config_findings(args: &DoctorArgs, opts: &Options) -> Vec<Finding> {
    let mut out = Vec::new();
    if !opts.wants(Component::Config) && !opts.wants(Component::Agents) {
        return out;
    }
    let platform = NativePlatform::new();
    let config = match load_config(&platform, args.config.as_deref()).await {
        Ok(c) => c,
        Err(e) => {
            out.push(
                Finding::new(Component::Config, "config_loaded", Severity::Fail, format!("config failed to load: {e}"))
                    .remedy("fix the config file or pass --config <path>"),
            );
            return out;
        }
    };
    if opts.wants(Component::Config) {
        let path = discover_config_path(&platform)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found, using defaults".into());
        out.push(Finding::new(
            Component::Config,
            "config_loaded",
            Severity::Ok,
            format!("config loaded ({path}; model={}, brand={})", config.agents.defaults.model, config.brand()),
        ));
    }
    if opts.wants(Component::Agents) {
        let found = multi_agent_findings(&config, claude_binary_on_path(), cfg!(feature = "delegate"));
        out.extend(found.into_iter().map(|f| f.into_finding(Component::Agents)));
    }
    out
}

/// Run the doctor command.
pub async fn run(args: DoctorArgs) -> anyhow::Result<()> {
    let opts = Options {
        components: args.components()?,
        fix: args.fix,
        all_runtimes: args.all_runtimes,
        self_version: env!("BUILD_VERSION").to_string(),
    };
    let env = DoctorEnv::detect();
    let o2 = opts.clone();
    let mut report: Report = tokio::task::spawn_blocking(move || doctor::run_system(&env, &o2)).await?;
    report.findings.extend(config_findings(&args, &opts).await);

    let code = doctor::print_report(&report, "weft doctor", args.json, args.strict);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_args_defaults() {
        let args = DoctorArgs::default();
        assert!(args.config.is_none());
        assert!(!args.strict && !args.multi_agent && !args.json && !args.fix && !args.all_runtimes);
        assert!(args.components().unwrap().is_empty());
    }

    #[test]
    fn multi_agent_flag_and_positional_merge() {
        let args = DoctorArgs {
            only: vec!["install".into()],
            component: vec!["runtime".into()],
            multi_agent: true,
            ..Default::default()
        };
        assert_eq!(
            args.components().unwrap(),
            vec![Component::Install, Component::Runtime, Component::Agents]
        );
    }

    #[test]
    fn unknown_component_is_an_error() {
        let args = DoctorArgs { component: vec!["nope".into()], ..Default::default() };
        assert!(args.components().is_err());
    }
}
