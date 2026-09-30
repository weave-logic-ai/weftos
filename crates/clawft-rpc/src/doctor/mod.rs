//! Shared `doctor` engine behind `weft doctor` and `weaver doctor`.
//!
//! One command, grouped checks, each pass/warn/fail with a one-line remedy.
//! Read-only by default: [`Options::fix`] only performs safe local repairs
//! (removing a provably stale socket or pid file) and reports each change in
//! [`Finding::fixed`]. Nothing here deletes keys, prints key contents, or
//! signals a process.
//!
//! Components (`--component`):
//!
//! | Component | Module | Checks |
//! |---|---|---|
//! | `install` | [`install`] | every copy of weft/weaver/weftos, channel, PATH winner |
//! | `daemon` | [`daemon`] | running kernel processes, exe path, version skew |
//! | `runtime` | [`runtime`] | runtime dir, stale socket/pid, multiple `node.key` |
//! | `mcp` | [`mcp`] | `.mcp.json` servers resolve |
//! | `config`, `agents` | `clawft-cli` | config load and multi-agent readiness |
//!
//! All path resolution goes through [`env::DoctorEnv::detect`], the single
//! place to switch over to a new runtime-paths API.

pub mod channel;
pub mod daemon;
pub mod env;
pub mod install;
pub mod mcp;
pub mod probe;
pub mod runtime;

use std::str::FromStr;

use serde::Serialize;
use serde_json::{Map, Value};

pub use env::DoctorEnv;

/// Severity of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Severity {
    /// Healthy.
    #[serde(rename = "pass")]
    Ok,
    /// Advisory; exit 0 unless `--strict`.
    #[serde(rename = "warn")]
    Warn,
    /// Hard problem; exit non-zero.
    #[serde(rename = "fail")]
    Fail,
}

impl Severity {
    /// Fixed-width tag for text output.
    pub fn tag(self) -> &'static str {
        match self {
            Severity::Ok => "PASS",
            Severity::Warn => "WARN",
            Severity::Fail => "FAIL",
        }
    }
}

/// A check group selectable with `--component`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    /// Installed binaries.
    Install,
    /// Running kernel processes.
    Daemon,
    /// Runtime dir, sockets, keys.
    Runtime,
    /// Config file.
    Config,
    /// MCP server wiring.
    Mcp,
    /// Multi-agent readiness.
    Agents,
}

impl Component {
    /// All components in report order.
    pub const ALL: [Component; 6] = [
        Component::Install,
        Component::Daemon,
        Component::Runtime,
        Component::Config,
        Component::Mcp,
        Component::Agents,
    ];

    /// Names accepted on the command line.
    pub const NAMES: [&'static str; 6] = ["install", "daemon", "runtime", "config", "mcp", "agents"];

    /// Lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Component::Install => "install",
            Component::Daemon => "daemon",
            Component::Runtime => "runtime",
            Component::Config => "config",
            Component::Mcp => "mcp",
            Component::Agents => "agents",
        }
    }
}

impl FromStr for Component {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Component::ALL
            .into_iter()
            .find(|c| c.as_str() == s)
            .ok_or_else(|| format!("unknown component `{s}` (expected one of {:?})", Component::NAMES))
    }
}

/// One check result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// Group.
    pub component: Component,
    /// Stable check id.
    pub id: String,
    /// Result.
    pub severity: Severity,
    /// One-line summary.
    pub message: String,
    /// One-line remedy command, when there is something to do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remedy: Option<String>,
    /// What `--fix` changed for this finding, if anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed: Option<String>,
}

impl Finding {
    /// New finding without remedy.
    pub fn new(
        component: Component,
        id: impl Into<String>,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            component,
            id: id.into(),
            severity,
            message: message.into(),
            remedy: None,
            fixed: None,
        }
    }

    /// Attach a remedy.
    pub fn remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }
}

/// Run options.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Selected components; empty means all.
    pub components: Vec<Component>,
    /// Apply safe local repairs.
    pub fix: bool,
    /// Extend `fix` from the active runtime dir to every candidate dir.
    pub all_runtimes: bool,
    /// Full version stamp of the calling binary (`0.8.1 (2cd752e1 ...)`).
    pub self_version: String,
}

impl Options {
    /// Whether `c` is selected.
    pub fn wants(&self, c: Component) -> bool {
        self.components.is_empty() || self.components.contains(&c)
    }
}

/// Findings plus structured inventory data.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    /// Ordered findings.
    pub findings: Vec<Finding>,
    /// Per-component structured data (copies, daemons, runtime dirs).
    pub data: Map<String, Value>,
}

impl Report {
    /// Worst severity present.
    pub fn worst(&self) -> Severity {
        self.findings.iter().map(|f| f.severity).max().unwrap_or(Severity::Ok)
    }

    /// Process exit code: 1 on any FAIL, or on WARN with `strict`.
    pub fn exit_code(&self, strict: bool) -> i32 {
        match self.worst() {
            Severity::Fail => 1,
            Severity::Warn if strict => 1,
            _ => 0,
        }
    }

    /// Counts as `(pass, warn, fail)`.
    pub fn counts(&self) -> (usize, usize, usize) {
        let n = |s| self.findings.iter().filter(|f| f.severity == s).count();
        (n(Severity::Ok), n(Severity::Warn), n(Severity::Fail))
    }

    /// Human-readable, grouped by component.
    pub fn render_text(&self, title: &str) -> String {
        let mut out = format!("{title}\n{}\n", "=".repeat(title.len()));
        for comp in Component::ALL {
            let group: Vec<_> = self.findings.iter().filter(|f| f.component == comp).collect();
            if group.is_empty() {
                continue;
            }
            out.push_str(&format!("\n[{}]\n", comp.as_str()));
            for f in group {
                out.push_str(&format!("  [{}] {}: {}\n", f.severity.tag(), f.id, f.message));
                if let Some(r) = &f.remedy {
                    out.push_str(&format!("         fix: {r}\n"));
                }
                if let Some(x) = &f.fixed {
                    out.push_str(&format!("         fixed: {x}\n"));
                }
            }
        }
        let (p, w, f) = self.counts();
        out.push_str(&format!("\nSummary: {p} pass, {w} warn, {f} fail\n"));
        out
    }

    /// Machine-readable output.
    pub fn to_json(&self, tool: &str, strict: bool) -> Value {
        let (p, w, f) = self.counts();
        serde_json::json!({
            "tool": tool,
            "summary": { "pass": p, "warn": w, "fail": f },
            "exit_code": self.exit_code(strict),
            "findings": self.findings,
            "data": self.data,
        })
    }
}

/// Print `report` (text or JSON) and return the process exit code.
pub fn print_report(report: &Report, title: &str, json: bool, strict: bool) -> i32 {
    if json {
        println!("{}", serde_json::to_string_pretty(&report.to_json(title, strict)).unwrap_or_default());
    } else {
        print!("{}", report.render_text(title));
        if report.exit_code(strict) == 0 && report.worst() == Severity::Warn {
            println!("(warnings exit 0; use --strict to fail on them)");
        }
    }
    report.exit_code(strict)
}

/// Parse component names given on the command line (empty means all).
pub fn parse_components(names: &[String]) -> Result<Vec<Component>, String> {
    names.iter().map(|n| n.parse()).collect()
}

/// Run every selected system component (install, daemon, runtime, mcp).
///
/// `config` and `agents` need `clawft-types::Config` and are added by the
/// caller (`weft doctor`).
pub fn run_system(env: &DoctorEnv, opts: &Options) -> Report {
    let mut report = Report::default();
    let need_inventory = opts.wants(Component::Install) || opts.wants(Component::Daemon);
    let copies = if need_inventory { install::scan(env) } else { Vec::new() };

    if opts.wants(Component::Install) {
        report.findings.extend(install::findings(&copies));
        report.data.insert("install".into(), serde_json::to_value(&copies).unwrap_or_default());
    }
    let procs = if opts.wants(Component::Daemon) || opts.wants(Component::Runtime) {
        daemon::ProcTable::load(env)
    } else {
        daemon::ProcTable::default()
    };
    if opts.wants(Component::Daemon) {
        if !procs.ok {
            report.findings.push(
                Finding::new(Component::Daemon, "ps", Severity::Warn, "could not list processes (ps failed or sandboxed); daemon state is unknown")
                    .remedy("run doctor outside the sandbox"),
            );
        }
        let daemons = daemon::discover(env, &procs);
        report.findings.extend(daemon::findings(env, &copies, &daemons, &opts.self_version));
        report.data.insert("daemon".into(), serde_json::to_value(&daemons).unwrap_or_default());
    }
    if opts.wants(Component::Runtime) {
        let (f, data) = runtime::check(env, &procs, opts.fix, opts.all_runtimes);
        report.findings.extend(f);
        report.data.insert("runtime".into(), data);
    }
    if opts.wants(Component::Mcp) {
        report.findings.extend(mcp::findings(env));
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_roundtrip() {
        for c in Component::ALL {
            assert_eq!(c.as_str().parse::<Component>().unwrap(), c);
        }
        assert!("bogus".parse::<Component>().is_err());
    }

    #[test]
    fn exit_codes() {
        let mut r = Report::default();
        assert_eq!(r.exit_code(true), 0);
        r.findings.push(Finding::new(Component::Install, "a", Severity::Warn, "w"));
        assert_eq!(r.exit_code(false), 0);
        assert_eq!(r.exit_code(true), 1);
        r.findings.push(Finding::new(Component::Install, "b", Severity::Fail, "f"));
        assert_eq!(r.exit_code(false), 1);
    }

    #[test]
    fn json_uses_pass_warn_fail() {
        let mut r = Report::default();
        r.findings.push(Finding::new(Component::Runtime, "x", Severity::Ok, "ok"));
        let j = r.to_json("weft doctor", false);
        assert_eq!(j["findings"][0]["severity"], "pass");
        assert_eq!(j["findings"][0]["component"], "runtime");
        assert_eq!(j["summary"]["pass"], 1);
    }

    #[test]
    fn empty_components_means_all() {
        let o = Options::default();
        assert!(o.wants(Component::Mcp));
        let o = Options { components: vec![Component::Install], ..Default::default() };
        assert!(!o.wants(Component::Mcp));
    }
}
