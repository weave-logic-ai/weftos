//! `weaver route serve --plan|--apply` (ADR-116 §1, §5): the Tailscale Serve
//! change that puts the router behind the machine's `:443`.
//!
//! The plan is a pure function of `tailscale serve status --json`, the route
//! table and the router's address. It refuses, with nothing changed, when
//! Funnel is on for `:443` or when an existing mapping has no route
//! equivalent (today's `/ → :18120` must first be covered by a route). It
//! never enables Funnel, and runs `tailscale` with an argument vector, no shell.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use crate::router_routes::RouteTable;

/// Longest a `tailscale` subprocess may run.
pub const TAILSCALE_TIMEOUT: Duration = Duration::from_secs(20);
/// Largest `tailscale serve status --json` read.
const MAX_OUTPUT: usize = 1024 * 1024;

/// One handler under `:443`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Mount path (`/`, `/foo`).
    pub path: String,
    /// The `Proxy` target, or a description of a non-proxy handler.
    pub target: String,
    pub is_proxy: bool,
}

/// What Serve does with `:443` today.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServeStatus {
    pub https_443: bool,
    pub funnel_443: bool,
    pub mounts: Vec<Mount>,
}

#[derive(Deserialize)]
struct Handler {
    #[serde(rename = "Proxy")]
    proxy: Option<String>,
    #[serde(rename = "Path")]
    path: Option<String>,
    #[serde(rename = "Text")]
    text: Option<String>,
}

#[derive(Deserialize)]
struct WebHost {
    #[serde(rename = "Handlers", default)]
    handlers: BTreeMap<String, Handler>,
}

#[derive(Deserialize, Default)]
struct Raw {
    #[serde(rename = "TCP", default)]
    tcp: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "Web", default)]
    web: BTreeMap<String, WebHost>,
    #[serde(rename = "AllowFunnel", default)]
    allow_funnel: BTreeMap<String, bool>,
}

/// Parse `tailscale serve status --json` (an empty document means no config).
pub fn parse_status(json: &str) -> Result<ServeStatus, String> {
    let text = json.trim();
    if text.is_empty() {
        return Ok(ServeStatus::default());
    }
    let raw: Raw = serde_json::from_str(text).map_err(|e| format!("tailscale serve status --json: {e}"))?;
    let https_443 = raw.tcp.get("443").and_then(|v| v.get("HTTPS")).and_then(|v| v.as_bool()).unwrap_or(false);
    let funnel_443 = raw.allow_funnel.iter().any(|(k, on)| *on && k.ends_with(":443"));
    let mut mounts = Vec::new();
    for (host, w) in raw.web.iter().filter(|(k, _)| k.ends_with(":443")) {
        let _ = host;
        for (path, h) in &w.handlers {
            let (target, is_proxy) = match (&h.proxy, &h.path, &h.text) {
                (Some(p), _, _) => (p.clone(), true),
                (None, Some(p), _) => (format!("path {p}"), false),
                (None, None, Some(_)) => ("text".to_owned(), false),
                _ => ("unknown handler".to_owned(), false),
            };
            mounts.push(Mount { path: path.clone(), target, is_proxy });
        }
    }
    Ok(ServeStatus { https_443, funnel_443, mounts })
}

/// The port of a loopback proxy target (`http://127.0.0.1:18120`,
/// `localhost:3000`, `http://[::1]:8080/`), else `None`.
pub fn loopback_port(target: &str) -> Option<u16> {
    let rest = target.split_once("://").map_or(target, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or("");
    let (host, port) = authority.rsplit_once(':')?;
    matches!(host, "127.0.0.1" | "localhost" | "[::1]").then(|| port.parse().ok()).flatten()
}

/// The change to make: `tailscale` argument vectors (without the binary),
/// in order, and what each existing mapping is covered by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServePlan {
    pub steps: Vec<Vec<String>>,
    pub notes: Vec<String>,
}

impl ServePlan {
    /// Nothing to do: `:443` already proxies to the router and nothing else is mounted.
    pub fn is_noop(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Compute the plan, or the reason it is refused.
pub fn plan(status: &ServeStatus, table: &RouteTable, router: SocketAddr) -> Result<ServePlan, String> {
    if status.funnel_443 {
        return Err("Funnel is on for :443; the router is tailnet-only. Turn it off first (tailscale funnel --https=443 off). Nothing changed.".into());
    }
    let router_target = format!("http://{router}");
    let mut out = ServePlan::default();
    let mut root_is_router = false;
    for m in &status.mounts {
        if m.path == "/" && m.is_proxy && loopback_port(&m.target) == Some(router.port()) {
            root_is_router = true;
            out.notes.push(format!("/ → {} is already the router", m.target));
            continue;
        }
        if !m.is_proxy {
            return Err(format!("existing mapping {} ({}) is not a loopback proxy; the router cannot cover it. Nothing changed.", m.path, m.target));
        }
        let Some(port) = loopback_port(&m.target) else {
            return Err(format!("existing mapping {} → {} is not a loopback proxy; the router cannot cover it. Nothing changed.", m.path, m.target));
        };
        let covered = table.routes.iter().find(|r| r.port == port && if m.path == "/" { r.default } else { r.prefix == m.path });
        match covered {
            None => {
                let hint = if m.path == "/" { " (default: true keeps it serving at /)" } else { "" };
                return Err(format!(
                    "existing mapping {} → {} has no route equivalent; declare a route for :{port} in that project's compose/ports.yaml{hint}, reload, then apply. Nothing changed.",
                    m.path, m.target
                ));
            }
            Some(r) => {
                out.notes.push(format!("{} → {} is covered by project {} ({}{})", m.path, m.target, r.project, r.prefix, if r.default { ", default" } else { "" }));
                if m.path != "/" {
                    out.steps.push(vec!["serve".into(), "--bg".into(), "--https=443".into(), format!("--set-path={}", m.path), "off".into()]);
                }
            }
        }
    }
    if !root_is_router {
        out.steps.push(vec!["serve".into(), "--bg".into(), "--https=443".into(), router_target]);
    }
    Ok(out)
}

/// Run `bin args…` without a shell, bounded in time and output; stdout on success.
pub fn run(bin: &str, args: &[String]) -> Result<String, String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{bin}: {e}"))?;
    let deadline = std::time::Instant::now() + TAILSCALE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{bin} {} did not finish within {}s", args.join(" "), TAILSCALE_TIMEOUT.as_secs()));
            }
        }
    }
    let out = child.wait_with_output().map_err(|e| format!("{bin}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout[..out.stdout.len().min(MAX_OUTPUT)]).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(format!("{bin} {} failed: {}", args.join(" "), if err.is_empty() { stdout.trim().to_owned() } else { err }))
    }
}

/// `tailscale serve status --json`, parsed.
pub fn tailscale_status(bin: &str) -> Result<ServeStatus, String> {
    let out = run(bin, &["serve".into(), "status".into(), "--json".into()])?;
    parse_status(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router_routes::Route;

    const SHASTA_ROOT: &str = r#"{"TCP":{"443":{"HTTPS":true}},"Web":{"machine.example.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:18120"}}}}}"#;
    const FUNNEL_ON: &str = r#"{"TCP":{"443":{"HTTPS":true}},"Web":{"machine.example.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:18120"}}}},"AllowFunnel":{"machine.example.ts.net:443":true}}"#;
    const ROUTER_ALREADY: &str = r#"{"TCP":{"443":{"HTTPS":true}},"Web":{"machine.example.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:18100"}}}}}"#;

    fn router() -> SocketAddr {
        "127.0.0.1:18100".parse().unwrap()
    }
    fn table(routes: Vec<Route>) -> RouteTable {
        RouteTable { routes, ..Default::default() }
    }
    fn route(prefix: &str, port: u16, default: bool) -> Route {
        Route { project: "shastaos".into(), prefix: prefix.into(), port, health: None, default }
    }

    #[test]
    fn status_parses_root_mapping_funnel_and_empty() {
        let s = parse_status(SHASTA_ROOT).unwrap();
        assert!(s.https_443 && !s.funnel_443);
        assert_eq!(s.mounts, vec![Mount { path: "/".into(), target: "http://127.0.0.1:18120".into(), is_proxy: true }]);
        assert!(parse_status(FUNNEL_ON).unwrap().funnel_443);
        assert_eq!(parse_status("").unwrap(), ServeStatus::default());
        assert_eq!(parse_status("{}").unwrap(), ServeStatus::default());
    }

    #[test]
    fn loopback_targets_only() {
        assert_eq!(loopback_port("http://127.0.0.1:18120"), Some(18120));
        assert_eq!(loopback_port("localhost:3000"), Some(3000));
        assert_eq!(loopback_port("http://[::1]:8080/"), Some(8080));
        assert_eq!(loopback_port("http://10.0.0.5:80"), None);
        assert_eq!(loopback_port("https://example.com"), None);
    }

    #[test]
    fn refuses_to_drop_the_root_mapping_without_a_default_route() {
        let s = parse_status(SHASTA_ROOT).unwrap();
        let e = plan(&s, &table(vec![route("/shastaos", 18120, false)]), router()).unwrap_err();
        assert!(e.contains("/ → http://127.0.0.1:18120") && e.contains("no route equivalent") && e.contains("Nothing changed"), "{e}");
        let e = plan(&s, &table(vec![]), router()).unwrap_err();
        assert!(e.contains("default: true"), "{e}");
    }

    #[test]
    fn a_default_route_covers_the_root_and_the_plan_is_one_serve_command() {
        let s = parse_status(SHASTA_ROOT).unwrap();
        let p = plan(&s, &table(vec![route("/shastaos", 18120, true)]), router()).unwrap();
        assert_eq!(p.steps, vec![vec!["serve", "--bg", "--https=443", "http://127.0.0.1:18100"]]);
        assert!(p.notes[0].contains("covered by project shastaos"), "{:?}", p.notes);
    }

    #[test]
    fn refuses_when_funnel_is_on_even_with_coverage() {
        let s = parse_status(FUNNEL_ON).unwrap();
        let e = plan(&s, &table(vec![route("/shastaos", 18120, true)]), router()).unwrap_err();
        assert!(e.contains("Funnel") && e.contains("Nothing changed"), "{e}");
        assert!(!e.contains("funnel on"), "the plan must never suggest enabling Funnel: {e}");
    }

    #[test]
    fn already_configured_is_a_noop_and_fresh_machine_is_one_step() {
        let p = plan(&parse_status(ROUTER_ALREADY).unwrap(), &table(vec![]), router()).unwrap();
        assert!(p.is_noop() && p.notes[0].contains("already the router"));
        let p = plan(&ServeStatus::default(), &table(vec![]), router()).unwrap();
        assert_eq!(p.steps.len(), 1);
    }

    #[test]
    fn covered_sub_paths_are_cleared_first_and_non_proxy_handlers_refuse() {
        let json = r#"{"Web":{"m.example.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:18120"},"/docs":{"Proxy":"http://127.0.0.1:4000"}}}}}"#;
        let s = parse_status(json).unwrap();
        let t = table(vec![route("/shastaos", 18120, true), Route { project: "weftos".into(), prefix: "/docs".into(), port: 4000, health: None, default: false }]);
        let p = plan(&s, &t, router()).unwrap();
        assert_eq!(p.steps[0], vec!["serve", "--bg", "--https=443", "--set-path=/docs", "off"]);
        assert_eq!(p.steps[1][3], "http://127.0.0.1:18100");
        let json = r#"{"Web":{"m.example.ts.net:443":{"Handlers":{"/":{"Path":"/srv/www"}}}}}"#;
        let e = plan(&parse_status(json).unwrap(), &t, router()).unwrap_err();
        assert!(e.contains("not a loopback proxy"), "{e}");
    }
}
