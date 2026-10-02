//! Installer tiers (ADR-103 Phase 3, package H; amends
//! `docs/plans/install-update-review.md` 6.4).
//!
//! The install receipt gains `tiers: [{name, exe, sha, version, owner}]` for
//! `service`, `user` and `project`. The receipt is only a hint: the service
//! tier is always re-read from what the service reports (`service.json`,
//! `weaver mesh status`) and skew is reported against that.
//!
//! `weaver update` never runs `sudo`: for the service it PRINTS the `cp` and
//! restart lines ([`service_update_lines`]) when the packaged build differs
//! from the running service's.

use std::path::Path;

use clawft_rpc::doctor::{Component, Finding, Severity};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::service_units_system::{MESH_LAUNCHD_LABEL, MESH_SYSTEMD_UNIT, SERVICE_EXE};

/// One installed tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tier {
    /// `service`, `user` or `project`.
    pub name: String,
    pub exe: String,
    /// Build stamp (what `--version` / `service.json` report).
    pub sha: String,
    pub version: String,
    /// Account that owns the binary.
    pub owner: String,
}

/// Tiers recorded in a receipt document (tolerant: bad entries are skipped).
pub fn parse_tiers(receipt: &Value) -> Vec<Tier> {
    receipt["tiers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| serde_json::from_value(t.clone()).ok())
        .collect()
}

/// Insert or replace the tier named `t.name`, leaving every other receipt key alone.
pub fn upsert_tier(receipt: &mut Value, t: Tier) {
    if !receipt.is_object() {
        *receipt = json!({});
    }
    let tiers = receipt["tiers"].take();
    let mut list: Vec<Value> = tiers.as_array().cloned().unwrap_or_default();
    list.retain(|e| e["name"] != t.name.as_str());
    list.push(serde_json::to_value(t).unwrap_or_default());
    receipt["tiers"] = Value::Array(list);
}

/// What the running service reports about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceObserved {
    pub build_sha: String,
}

/// Skew findings (`install` component). `own_sha` is this weaver's build.
/// `observed` is `None` when no service answers or has a record.
pub fn skew_findings(receipt: &[Tier], observed: Option<&ServiceObserved>, own_sha: &str) -> Vec<Finding> {
    let c = Component::Install;
    let mut out = Vec::new();
    let Some(o) = observed else {
        if let Some(t) = receipt.iter().find(|t| t.name == "service") {
            out.push(
                Finding::new(c, "tier.service", Severity::Warn, format!("the receipt lists a service tier ({}) but no service record is present", t.exe))
                    .remedy("weaver mesh status"),
            );
        }
        return out;
    };
    if o.build_sha == own_sha {
        out.push(Finding::new(c, "tier.service", Severity::Ok, format!("service build {} matches this weaver", o.build_sha)));
    } else {
        out.push(
            Finding::new(c, "tier.service", Severity::Warn, format!("service runs build {} but this weaver is {own_sha}", o.build_sha))
                .remedy("weaver update prints the sudo cp and restart lines for the service"),
        );
    }
    if let Some(t) = receipt.iter().find(|t| t.name == "service")
        && t.sha != o.build_sha
    {
        out.push(Finding::new(
            c,
            "tier.service_receipt",
            Severity::Warn,
            format!("the receipt says the service is {} but it reports {} (the receipt is stale; the service's own report wins)", t.sha, o.build_sha),
        ));
    }
    out
}

/// Which manager the service restart lines are for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Launchd,
    Systemd,
}

impl Manager {
    pub fn host() -> Self {
        if cfg!(target_os = "macos") { Manager::Launchd } else { Manager::Systemd }
    }
}

/// Lines `weaver update` prints for the service tier. Empty when the service
/// is absent or already on `packaged_sha`. These are text for an
/// administrator; nothing here runs `sudo` or signals a process.
pub fn service_update_lines(manager: Manager, packaged_exe: &Path, packaged_sha: &str, service: Option<&ServiceObserved>) -> Vec<String> {
    let Some(s) = service else { return Vec::new() };
    if s.build_sha == packaged_sha {
        return Vec::new();
    }
    let exe = crate::service_units::sh_quote(&packaged_exe.to_string_lossy());
    let restart = match manager {
        Manager::Launchd => format!("sudo launchctl kickstart -k system/{MESH_LAUNCHD_LABEL}"),
        Manager::Systemd => format!("sudo systemctl restart {MESH_SYSTEMD_UNIT}"),
    };
    vec![
        format!("The machine mesh service runs build {} but the packaged build is {packaged_sha}.", s.build_sha),
        "It is not touched by this update. To update it, run (nothing here runs them):".to_owned(),
        format!("  sudo install -m 0755 -o root {exe} {SERVICE_EXE}"),
        format!("  {restart}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(name: &str, sha: &str) -> Tier {
        Tier { name: name.into(), exe: "/x".into(), sha: sha.into(), version: "0.8.1".into(), owner: "root".into() }
    }

    #[test]
    fn receipt_round_trip_keeps_other_keys() {
        let mut r = json!({"install_prefix": "/p", "binaries": ["weaver"]});
        upsert_tier(&mut r, tier("service", "a"));
        upsert_tier(&mut r, tier("user", "b"));
        upsert_tier(&mut r, tier("service", "c"));
        assert_eq!(r["install_prefix"], "/p");
        let t = parse_tiers(&r);
        assert_eq!(t.len(), 2);
        assert_eq!(t.iter().find(|t| t.name == "service").unwrap().sha, "c");
        assert!(parse_tiers(&json!({"tiers": [1, {"name": "x"}]})).is_empty());
    }

    #[test]
    fn observed_service_beats_the_receipt() {
        let obs = ServiceObserved { build_sha: "new".into() };
        let v = skew_findings(&[tier("service", "old")], Some(&obs), "new");
        assert_eq!(v[0].severity, Severity::Ok);
        assert_eq!(v[1].id, "tier.service_receipt");
        let skew = skew_findings(&[], Some(&obs), "other");
        assert_eq!(skew[0].severity, Severity::Warn);
        let gone = skew_findings(&[tier("service", "a")], None, "x");
        assert_eq!(gone[0].severity, Severity::Warn);
        assert!(skew_findings(&[], None, "x").is_empty());
    }

    #[test]
    fn update_prints_sudo_lines_only_when_the_sha_differs() {
        let exe = Path::new("/opt/we ftos/weaver");
        assert!(service_update_lines(Manager::Systemd, exe, "a", None).is_empty());
        let same = ServiceObserved { build_sha: "a".into() };
        assert!(service_update_lines(Manager::Systemd, exe, "a", Some(&same)).is_empty());
        let old = ServiceObserved { build_sha: "z".into() };
        let sd = service_update_lines(Manager::Systemd, exe, "a", Some(&old));
        assert!(sd.contains(&"  sudo systemctl restart weftos-mesh".to_owned()));
        assert!(sd.iter().any(|l| l.contains("'/opt/we ftos/weaver' /usr/local/libexec/weftos/weaver")));
        let ld = service_update_lines(Manager::Launchd, exe, "a", Some(&old));
        assert!(ld.contains(&"  sudo launchctl kickstart -k system/ai.weftos.mesh".to_owned()));
    }
}
