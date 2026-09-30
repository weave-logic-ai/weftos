//! Multi-agent readiness checks for `weft doctor` (WEFT-197): `claude` on
//! PATH, auto-delegation config, agent routes. Pure functions over
//! [`Config`] so they unit-test without a machine.

use std::path::PathBuf;
use std::process::Command;

use clawft_rpc::doctor::{Component, Finding};
use clawft_types::agent_routing::AgentRoutingConfig;
use clawft_types::config::Config;
use clawft_types::delegation::DelegationConfig;

/// Severity of a doctor finding (shared with the system checks).
pub use clawft_rpc::doctor::Severity as CheckSeverity;

/// One doctor check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorFinding {
    /// Short check id (`claude_path`, `auto_delegation`, `agent_routes`).
    pub id: &'static str,
    /// Severity.
    pub severity: CheckSeverity,
    /// Human-readable summary.
    pub message: String,
    /// Optional remediation hint.
    pub hint: Option<String>,
}

/// Pure multi-agent checks used by doctor and unit tests (WEFT-197).
pub fn multi_agent_findings(
    config: &Config,
    claude_on_path: bool,
    delegate_feature_compiled: bool,
) -> Vec<DoctorFinding> {
    let mut out = Vec::new();

    // ── claude binary ──────────────────────────────────────────────────
    out.push(if claude_on_path {
        DoctorFinding {
            id: "claude_path",
            severity: CheckSeverity::Ok,
            message: "claude binary found on PATH".into(),
            hint: None,
        }
    } else {
        DoctorFinding {
            id: "claude_path",
            severity: CheckSeverity::Warn,
            message: "claude binary not found on PATH".into(),
            hint: Some(
                "Install Claude Code CLI or ensure `claude` is on PATH for MCP bridge / delegation."
                    .into(),
            ),
        }
    });

    // ── auto-delegation ────────────────────────────────────────────────
    out.push(auto_delegation_finding(
        &config.delegation,
        delegate_feature_compiled,
    ));

    // ── agent routes ───────────────────────────────────────────────────
    out.push(agent_routes_finding(&config.agent_routing));

    out
}

fn auto_delegation_finding(
    delegation: &DelegationConfig,
    delegate_feature_compiled: bool,
) -> DoctorFinding {
    if !delegate_feature_compiled {
        return DoctorFinding {
            id: "auto_delegation",
            severity: CheckSeverity::Warn,
            message: "CLI built without `delegate` feature — auto-delegation unavailable".into(),
            hint: Some(
                "Rebuild with `--features delegate` (default for release builds) to enable pre-LLM routing."
                    .into(),
            ),
        };
    }

    let rule_count = delegation.rules.len();
    if !delegation.claude_enabled {
        return DoctorFinding {
            id: "auto_delegation",
            severity: CheckSeverity::Warn,
            message: format!(
                "delegation.claude_enabled=false ({rule_count} rule(s) configured; auto-delegation short-circuit inactive for Claude targets)"
            ),
            hint: Some(
                "Set `delegation.claude_enabled = true` (and rules) to enable pre-LLM auto-delegation."
                    .into(),
            ),
        };
    }

    if rule_count == 0 {
        DoctorFinding {
            id: "auto_delegation",
            severity: CheckSeverity::Warn,
            message: "auto-delegation enabled but no explicit rules — Auto complexity heuristic only"
                .into(),
            hint: Some(
                "Add `[[delegation.rules]]` patterns for predictable routing; see docs/guides/auto-delegation-classifier.md (WEFT-201)."
                    .into(),
            ),
        }
    } else {
        DoctorFinding {
            id: "auto_delegation",
            severity: CheckSeverity::Ok,
            message: format!(
                "auto-delegation ready (claude_enabled=true, {rule_count} rule(s))"
            ),
            hint: None,
        }
    }
}

fn agent_routes_finding(routing: &AgentRoutingConfig) -> DoctorFinding {
    let route_count = routing.routes.len();
    let has_catch_all = routing.catch_all.is_some();

    match (route_count, has_catch_all) {
        (0, false) => DoctorFinding {
            id: "agent_routes",
            severity: CheckSeverity::Warn,
            message: "no agent routes and no catch-all configured".into(),
            hint: Some(
                "Configure `agent_routing.routes` and/or `agent_routing.catch_all` for multi-agent dispatch."
                    .into(),
            ),
        },
        (0, true) => DoctorFinding {
            id: "agent_routes",
            severity: CheckSeverity::Ok,
            message: format!(
                "agent routing: catch-all only → {:?}",
                routing.catch_all.as_deref().unwrap_or("?")
            ),
            hint: Some(
                "Only catch-all is set; all unmatched traffic goes to that agent. Add explicit routes for channel/user splits."
                    .into(),
            ),
        },
        (n, true) => DoctorFinding {
            id: "agent_routes",
            severity: CheckSeverity::Ok,
            message: format!(
                "agent routing: {n} route(s) + catch-all → {:?}",
                routing.catch_all.as_deref().unwrap_or("?")
            ),
            hint: None,
        },
        (n, false) => DoctorFinding {
            id: "agent_routes",
            severity: CheckSeverity::Ok,
            message: format!(
                "agent routing: {n} route(s), no catch-all (unmatched → NoMatch)"
            ),
            hint: Some(
                "Consider `agent_routing.catch_all` so unmatched messages do not drop."
                    .into(),
            ),
        },
    }
}

/// Whether `claude` resolves on PATH (also accepts `CLAUDE_CLI` override).
pub fn claude_binary_on_path() -> bool {
    if let Ok(custom) = std::env::var("CLAUDE_CLI") {
        let p = PathBuf::from(&custom);
        if p.is_file() {
            return true;
        }
    }
    which_on_path("claude").is_some()
}

fn which_on_path(program: &str) -> Option<PathBuf> {
    // Prefer `which` when available; fall back to PATH walk.
    if let Ok(output) = Command::new("which").arg(program).output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }

    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{program}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

impl DoctorFinding {
    /// Convert into the shared report format under `component`.
    pub fn into_finding(self, component: Component) -> Finding {
        let mut f = Finding::new(component, self.id, self.severity, self.message);
        f.remedy = self.hint;
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_types::agent_routing::{AgentRoute, AgentRoutingConfig, MatchCriteria};
    use clawft_types::delegation::{DelegationConfig, DelegationRule, DelegationTarget};

    fn base_config() -> Config {
        Config::default()
    }

    #[test]
    fn claude_missing_is_warn() {
        let findings = multi_agent_findings(&base_config(), false, true);
        let c = findings.iter().find(|f| f.id == "claude_path").unwrap();
        assert_eq!(c.severity, CheckSeverity::Warn);
    }

    #[test]
    fn claude_present_is_ok() {
        let findings = multi_agent_findings(&base_config(), true, true);
        let c = findings.iter().find(|f| f.id == "claude_path").unwrap();
        assert_eq!(c.severity, CheckSeverity::Ok);
    }

    #[test]
    fn auto_delegation_without_feature() {
        let f = auto_delegation_finding(&DelegationConfig::default(), false);
        assert_eq!(f.severity, CheckSeverity::Warn);
        assert!(f.message.contains("without `delegate`"));
    }

    #[test]
    fn auto_delegation_disabled_claude() {
        let mut d = DelegationConfig::default();
        d.claude_enabled = false;
        let f = auto_delegation_finding(&d, true);
        assert_eq!(f.severity, CheckSeverity::Warn);
        assert!(f.message.contains("claude_enabled=false"));
    }

    #[test]
    fn auto_delegation_rules_ready() {
        let d = DelegationConfig {
            claude_enabled: true,
            rules: vec![DelegationRule {
                pattern: r"(?i)deploy".into(),
                target: DelegationTarget::Claude,
            }],
            ..Default::default()
        };
        let f = auto_delegation_finding(&d, true);
        assert_eq!(f.severity, CheckSeverity::Ok);
        assert!(f.message.contains("1 rule"));
    }

    #[test]
    fn agent_routes_empty_warns() {
        let f = agent_routes_finding(&AgentRoutingConfig::default());
        assert_eq!(f.severity, CheckSeverity::Warn);
        assert!(f.message.contains("no agent routes"));
    }

    #[test]
    fn agent_routes_catch_all_only() {
        let routing = AgentRoutingConfig {
            routes: vec![],
            catch_all: Some("default-agent".into()),
        };
        let f = agent_routes_finding(&routing);
        assert_eq!(f.severity, CheckSeverity::Ok);
        assert!(f.message.contains("catch-all only"));
        assert!(f.hint.is_some());
    }

    #[test]
    fn agent_routes_with_explicit_routes() {
        let routing = AgentRoutingConfig {
            routes: vec![AgentRoute {
                channel: "telegram".into(),
                match_criteria: MatchCriteria::default(),
                agent: "work".into(),
            }],
            catch_all: Some("default".into()),
        };
        let f = agent_routes_finding(&routing);
        assert_eq!(f.severity, CheckSeverity::Ok);
        assert!(f.message.contains("1 route"));
    }
}
