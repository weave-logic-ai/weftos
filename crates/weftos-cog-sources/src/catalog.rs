//! The cog catalog across sources (`weaver workload catalog --kind cog`).
//!
//! Joins what a source lists with what we know from two local inputs:
//! the cog's `cog.toml` (resources, secrets, hardware) and the committed
//! conformance baseline (`scripts/cogs/expectations.json`: which of the
//! 93 clean / 5 interval / 9 needs-extra-CLI groups a cog falls in). From
//! those it derives a per-cog default placement policy (ADR-099 / ADR-100
//! section 2). Both inputs are optional; a row says what it could not fill.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{CogLicence, SourceKind};
use crate::error::{Result, SourceError};
use crate::licence::entitlement;
use crate::resolve::LoadedSource;

/// One cog's committed conformance expectation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Expect {
    /// `clean` | `needs-interval` | `needs-extra-cli` | `no-build`.
    pub group: String,
    /// What the cog needs beyond the sensor feed (`seed-peers`, `seed-api`, ...).
    #[serde(default)]
    pub needs: Vec<String>,
    /// `--interval` value when the group is `needs-interval`.
    #[serde(default)]
    pub interval: Option<u32>,
    /// Free-text finding.
    #[serde(default)]
    pub notes: String,
}

/// `scripts/cogs/expectations.json`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Expectations {
    /// Per-cog expectation.
    pub cogs: BTreeMap<String, Expect>,
}

/// Group counts of a baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BaselineSummary {
    /// Run clean with `--once`.
    pub clean: usize,
    /// Persistent-listener cogs that need `--interval`.
    pub needs_interval: usize,
    /// Need seed peers, assets or other CLI.
    pub needs_extra_cli: usize,
    /// No aarch64 build published.
    pub no_build: usize,
}

impl Expectations {
    /// Parse the JSON file's bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(|e| SourceError::Parse { what: "expectations.json".into(), msg: e.to_string() })
    }

    /// Group counts (the 93 / 5 / 9 (/ 1) headline).
    pub fn summary(&self) -> BaselineSummary {
        let n = |g: &str| self.cogs.values().filter(|e| e.group == g).count();
        BaselineSummary {
            clean: n("clean"),
            needs_interval: n("needs-interval"),
            needs_extra_cli: n("needs-extra-cli"),
            no_build: n("no-build"),
        }
    }
}

/// How a cog is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    /// `--once` runs clean.
    Once,
    /// Persistent listener: run with `--interval N`.
    Interval,
    /// Needs seed peers or the Seed API (`swarm-*`).
    NeedsSeed,
    /// Needs other setup (assets, an MQTT broker, tailscale auth, its own CLI).
    NeedsExtra,
    /// No baseline entry.
    Unknown,
}

/// Resources and secrets read from `cog.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CogMeta {
    /// `[resources].ram_mb`.
    pub ram_mb: Option<u64>,
    /// `[resources].cpu_pct`.
    pub cpu_pct: Option<u64>,
    /// `[cog].hardware_requirement`.
    pub hardware_requirement: Vec<String>,
    /// `[config.*]` keys with `secret = true`.
    pub secrets: Vec<String>,
}

/// Parse the parts of `cog.toml` the catalog needs.
pub fn parse_cog_toml(text: &str) -> Result<CogMeta> {
    let v: toml::Value = toml::from_str(text).map_err(|e| SourceError::Parse { what: "cog.toml".into(), msg: e.to_string() })?;
    let num = |p: &str| v.get("resources").and_then(|r| r.get(p)).and_then(|x| x.as_integer()).map(|n| n.max(0) as u64);
    let hardware_requirement = v
        .get("cog")
        .and_then(|c| c.get("hardware_requirement"))
        .and_then(|h| h.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let mut secrets: Vec<String> = v
        .get("config")
        .and_then(|c| c.as_table())
        .map(|t| {
            t.iter()
                .filter(|(_, e)| e.get("secret").and_then(|s| s.as_bool()).unwrap_or(false))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    secrets.sort();
    Ok(CogMeta { ram_mb: num("ram_mb"), cpu_pct: num("cpu_pct"), hardware_requirement, secrets })
}

/// Run mode and its inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunInfo {
    /// Derived mode.
    pub mode: RunMode,
    /// `--interval` seconds for [`RunMode::Interval`].
    pub interval: Option<u32>,
    /// Baseline group, when the baseline has the cog.
    pub group: Option<String>,
    /// Extra needs from the baseline.
    pub needs: Vec<String>,
    /// Baseline note.
    pub notes: String,
}

impl RunInfo {
    fn from_expect(e: Option<&Expect>) -> Self {
        let Some(e) = e else {
            return Self { mode: RunMode::Unknown, interval: None, group: None, needs: vec![], notes: String::new() };
        };
        let mode = match e.group.as_str() {
            "clean" | "no-build" => RunMode::Once,
            "needs-interval" => RunMode::Interval,
            _ if e.needs.iter().any(|n| n.starts_with("seed-")) => RunMode::NeedsSeed,
            _ => RunMode::NeedsExtra,
        };
        Self { mode, interval: e.interval, group: Some(e.group.clone()), needs: e.needs.clone(), notes: e.notes.clone() }
    }
}

/// Default placement policy for a cog (an operator can override per instance).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlacementPolicy {
    /// ADR-100 section 3: native placement needs a paired node.
    pub min_trust_tier: String,
    /// Governed placement needs a `cogpkg` signed by a pinned signer
    /// (`weaver workload pack`), whatever source the binary came from.
    pub signed_package_required: bool,
    /// Emulation is operator opt-in only (ADR-099).
    pub allow_emulated: bool,
    /// Runtime adapters in preference order.
    pub runtime_preference: Vec<String>,
    /// Capability ids a node must advertise (ADR-100 section 2).
    pub requires: Vec<String>,
    /// Needs the ESP32 UDP sensor feed on the node's LAN (data locality).
    pub sensor_feed: bool,
    /// In the v1 scope (the clean `--once` group, ADR-100 decision 3).
    pub v1_scope: bool,
}

/// Whether this project can install the cog from its source today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    /// Signed source: install verifies the Ed25519 signature.
    Signed,
    /// Cognitum cog with a covering, unexpired licence.
    Licensed,
    /// Cognitum cog with no covering licence (listable, not installable).
    NeedsLicence,
    /// Cognitum cog whose licence expired.
    LicenceExpired,
}

/// One catalog row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CatalogRow {
    /// Source name.
    pub source: String,
    /// Source kind label.
    pub kind: String,
    /// Cog id.
    pub id: String,
    /// `source:id`.
    pub reference: String,
    /// Version.
    pub version: String,
    /// Category.
    pub category: String,
    /// Description.
    pub description: String,
    /// Arches with a binary.
    pub arches: Vec<String>,
    /// Hardware requirement (registry, else `cog.toml`).
    pub hardware_requirement: Vec<String>,
    /// `[resources]` from `cog.toml`, when it was available.
    pub ram_mb: Option<u64>,
    /// `[resources].cpu_pct`.
    pub cpu_pct: Option<u64>,
    /// Secret config keys.
    pub secrets: Vec<String>,
    /// Run mode.
    pub run: RunInfo,
    /// Default placement policy.
    pub policy: PlacementPolicy,
    /// Install access in this project.
    pub access: Access,
    /// True when a Cognitum-kind source lists this id (the Seed store lists 107 cogs, not
    /// `anomaly-detect`, so a cog with `seed_store = false` can only reach a Seed through our
    /// own signed-package path, ADR-100 section 5).
    pub seed_store: bool,
    /// Other enabled sources that also list this id (namespaced ids disambiguate them).
    pub also_in: Vec<String>,
}

fn policy_for(arches: &[String], hw: &[String], meta: &CogMeta, run: &RunInfo) -> PlacementPolicy {
    let mut requires: Vec<String> = Vec::new();
    for a in arches {
        match a.as_str() {
            "arm" => requires.push("cpu.arch.armv7".into()),
            "arm64" => requires.push("cpu.arch.aarch64".into()),
            other => requires.push(format!("cpu.arch.{other}")),
        }
    }
    requires.sort();
    requires.dedup();
    requires.push("runtime.native".into());
    if let Some(ram) = meta.ram_mb {
        requires.push(format!("mem.system>={ram}mb"));
    }
    for h in hw {
        requires.push(format!("x.hw.{h}"));
    }
    let sensor_feed = matches!(run.mode, RunMode::Once | RunMode::Interval | RunMode::Unknown);
    if sensor_feed {
        requires.push("feed.esp32-csi-udp".into());
    }
    // Apple `container` runs aarch64 only; armv7 needs native or docker.
    let mut runtime_preference = vec!["native".to_string()];
    if arches.iter().any(|a| a == "arm64") {
        runtime_preference.push("container.apple".into());
    }
    runtime_preference.push("container.docker".into());
    PlacementPolicy {
        min_trust_tier: "paired".into(),
        signed_package_required: true,
        allow_emulated: false,
        runtime_preference,
        requires,
        sensor_feed,
        v1_scope: run.group.as_deref() == Some("clean"),
    }
}

/// Build rows for every cog every loaded source lists, sorted by (category, source, id).
pub fn build_rows(
    loaded: &[LoadedSource],
    cog_toml_dir: Option<&Path>,
    expectations: Option<&Expectations>,
    licences: &[CogLicence],
    now: DateTime<Utc>,
) -> Vec<CatalogRow> {
    let mut rows = Vec::new();
    let mut listed: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
    for l in loaded.iter().filter(|l| l.source.enabled) {
        for c in l.cogs() {
            listed.entry(c.id).or_default().push((l.source.name.clone(), l.source.kind == SourceKind::Cognitum));
        }
    }
    for l in loaded.iter().filter(|l| l.source.enabled) {
        for c in l.cogs() {
            let meta = cog_toml_dir
                .and_then(|d| std::fs::read_to_string(d.join(&c.id).join("cog.toml")).ok())
                .and_then(|t| parse_cog_toml(&t).ok())
                .unwrap_or_default();
            let hw = if c.hardware_requirement.is_empty() { meta.hardware_requirement.clone() } else { c.hardware_requirement.clone() };
            let run = RunInfo::from_expect(expectations.and_then(|e| e.cogs.get(&c.id)));
            let policy = policy_for(&c.arches, &hw, &meta, &run);
            let access = if c.signed {
                Access::Signed
            } else {
                match entitlement(licences, &l.source.name, &c.id, now) {
                    Ok(_) => Access::Licensed,
                    Err(SourceError::LicenceExpired { .. }) => Access::LicenceExpired,
                    Err(_) => Access::NeedsLicence,
                }
            };
            let all = listed.get(&c.id).map(Vec::as_slice).unwrap_or(&[]);
            let seed_store = all.iter().any(|(_, cognitum)| *cognitum);
            let also_in: Vec<String> = all.iter().filter(|(n, _)| n != &l.source.name).map(|(n, _)| n.clone()).collect();
            rows.push(CatalogRow {
                source: l.source.name.clone(),
                kind: l.source.kind.label().into(),
                reference: format!("{}:{}", l.source.name, c.id),
                id: c.id,
                version: c.version,
                category: c.category,
                description: c.description,
                arches: c.arches,
                hardware_requirement: hw,
                ram_mb: meta.ram_mb,
                cpu_pct: meta.cpu_pct,
                secrets: meta.secrets,
                run,
                policy,
                access,
                seed_store,
                also_in,
            });
        }
    }
    rows.sort_by(|a, b| a.category.cmp(&b.category).then(a.source.cmp(&b.source)).then(a.id.cmp(&b.id)));
    rows
}

/// Fixed-width table for a terminal.
pub fn render_table(rows: &[CatalogRow]) -> String {
    if rows.is_empty() {
        return "No cogs: add a source with `weaver cog source add`\n".into();
    }
    let mut out = format!(
        "{:<34} {:<9} {:<10} {:<10} {:<16} {:<22} {:<8} {}\n",
        "COG", "VERSION", "ARCHES", "RUN", "ACCESS", "RESOURCES", "SECRETS", "HARDWARE"
    );
    for r in rows {
        let run = match (r.run.mode, r.run.interval) {
            (RunMode::Once, _) => "once".to_string(),
            (RunMode::Interval, Some(n)) => format!("interval {n}"),
            (RunMode::Interval, None) => "interval".into(),
            (RunMode::NeedsSeed, _) => "needs-seed".into(),
            (RunMode::NeedsExtra, _) => "needs-extra".into(),
            (RunMode::Unknown, _) => "?".into(),
        };
        let res = match (r.ram_mb, r.cpu_pct) {
            (Some(m), Some(c)) => format!("{m} MB / {c}% cpu"),
            (Some(m), None) => format!("{m} MB"),
            (None, Some(c)) => format!("{c}% cpu"),
            (None, None) => "-".into(),
        };
        let access = serde_json::to_value(r.access).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        out.push_str(&format!(
            "{:<34} {:<9} {:<10} {:<10} {:<16} {:<22} {:<8} {}\n",
            r.reference,
            r.version,
            r.arches.join(","),
            run,
            access,
            res,
            r.secrets.len(),
            if r.hardware_requirement.is_empty() { "-".into() } else { r.hardware_requirement.join(",") },
        ));
    }
    out
}
