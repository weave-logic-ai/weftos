//! The hardware catalog: **Projects -> Modules -> Chips** (COG-009, Voltkid-style browse).
//!
//! A cog is a *project*; it uses physical *modules* (a sound board, a radar, an ADC breakout, the
//! Seed board); each module carries *chips* (the silicon). This is the typed, link-checked backbone
//! both the egui console and the web dashboard read. The data ships in `catalog/catalog.json` and is
//! embedded here via [`CATALOG_JSON`], so the console needs no network to show it.
//!
//! Honesty rules baked into the data: `spec.source` marks vendor/datasheet claims (never our bench
//! data — real measurements live in each cog's guide), `buy` is filled by the distributor pull, and
//! `photo` names an image served in the cog's `/guide` bundle.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The catalog shipped with this crate (`catalog/catalog.json`).
pub const CATALOG_JSON: &str = include_str!("../catalog/catalog.json");

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct HwCatalog {
    pub schema: u32,
    #[serde(default)]
    pub generated: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub modules: Vec<Module>,
    #[serde(default)]
    pub chips: Vec<Chip>,
}

/// A cog, as a buildable project.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub difficulty: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub store_id: Option<u32>,
    #[serde(default)]
    pub export_port: Option<u16>,
    #[serde(default)]
    pub simulate: bool,
    #[serde(default)]
    pub guide: String,
    /// Module ids this project uses (resolve via [`HwCatalog::module`]).
    #[serde(default)]
    pub modules: Vec<String>,
    /// Stable unique item hash: `"wh_" + hex(sha256("<type>:<id>"))[..16]`. Matches the Sensor
    /// Explorer's hash so a catalog item is addressable the same everywhere.
    #[serde(default)]
    pub hash: String,
}

/// One distributor offer for a module or chip (filled by the Mouser/distributor pull).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct BuyLink {
    pub vendor: String,
    #[serde(default)]
    pub price: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub ships_from: String,
}

/// A physical module: a board, sensor, display, or actuator.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Module {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    /// board | sensor | display | actuator
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub good_for: Vec<String>,
    #[serde(default)]
    pub not_for: Vec<String>,
    /// Critical application/design notes pulled from the datasheet (gotchas that affect how the part
    /// must be used), each ideally ending with its source. E.g. UWB antenna co-polarization.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Free-form spec rows; a `source` key marks it as vendor/datasheet, not our measurement.
    #[serde(default)]
    pub spec: BTreeMap<String, String>,
    #[serde(default)]
    pub pins: Vec<String>,
    /// Chip ids on this module (resolve via [`HwCatalog::chip`]).
    #[serde(default)]
    pub chips: Vec<String>,
    #[serde(default)]
    pub photo: String,
    #[serde(default)]
    pub datasheet: String,
    #[serde(default)]
    pub mouser_query: String,
    #[serde(default)]
    pub buy: Vec<BuyLink>,
    /// Source documents this entry was drawn from (repo-relative paths, possibly in sibling repos).
    #[serde(default)]
    pub seen_in: Vec<String>,
    /// Ids of the cogs that drive this module (hardware -> software link, ADR-107). The cog side
    /// is the cog's own id (`cog.toml` `[cog].id`); resolve availability / install state against
    /// the marketplace catalog and the host. Empty = no cog yet.
    #[serde(default)]
    pub cogs: Vec<String>,
    /// Facts about the module's own firmware (not the cog's), when known.
    #[serde(default)]
    pub firmware: Option<Firmware>,
    /// Curated documentation beyond the datasheet: protocol notes, ADR / COG records, guides.
    #[serde(default)]
    pub docs: Vec<DocLink>,
    /// Stable unique item hash: `"wh_" + hex(sha256("<type>:<id>"))[..16]` (type `module`).
    #[serde(default)]
    pub hash: String,
}

/// A documentation pointer: `url` is an `http(s)` link, or a repo-relative path / `repo: path`
/// reference for docs that live in a source tree rather than on the web.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct DocLink {
    pub label: String,
    pub url: String,
}

impl DocLink {
    /// True when `url` can be opened as a hyperlink.
    pub fn is_web(&self) -> bool {
        self.url.starts_with("http://") || self.url.starts_with("https://")
    }
}

/// Firmware facts for the module itself. Every field is optional: an empty string means "not
/// known", never a guess. Whether a cog can read the version is `read_with` (+ the cog config key
/// in `read_config_key`); WeftOS does not update module firmware, `update` only says how.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Firmware {
    /// Version or variant facts known from the vendor ("V1.1 protocol", "ranging firmware").
    #[serde(default)]
    pub version: String,
    /// How a cog reads the version off the module, empty when none can.
    #[serde(default)]
    pub read_with: String,
    /// The cog config key that turns the read on (e.g. `query_firmware`).
    #[serde(default)]
    pub read_config_key: String,
    /// How the module firmware is updated (vendor route). WeftOS has no updater.
    #[serde(default)]
    pub update: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub notes: String,
}

/// A chip (the silicon on a module).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Chip {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub spec: BTreeMap<String, String>,
    #[serde(default)]
    pub mouser_query: String,
    #[serde(default)]
    pub datasheet: String,
    /// Stable unique item hash: `"wh_" + hex(sha256("<type>:<id>"))[..16]` (type `chip`).
    #[serde(default)]
    pub hash: String,
}

impl HwCatalog {
    /// Parse a catalog.json byte slice.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    }

    /// The catalog embedded in this crate.
    pub fn bundled() -> Self {
        Self::parse(CATALOG_JSON.as_bytes()).expect("embedded catalog.json is valid")
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }
    pub fn module(&self, id: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.id == id)
    }
    pub fn chip(&self, id: &str) -> Option<&Chip> {
        self.chips.iter().find(|c| c.id == id)
    }

    /// Projects that use a given module (reverse link, for a module's "used in" list).
    pub fn projects_using_module(&self, module_id: &str) -> Vec<&Project> {
        self.projects.iter().filter(|p| p.modules.iter().any(|m| m == module_id)).collect()
    }
    /// Modules a cog drives (reverse of [`Module::cogs`]): the hardware a cog needs.
    pub fn modules_for_cog(&self, cog_id: &str) -> Vec<&Module> {
        self.modules.iter().filter(|m| m.cogs.iter().any(|c| c == cog_id)).collect()
    }
    /// Modules that carry a given chip (reverse link).
    pub fn modules_with_chip(&self, chip_id: &str) -> Vec<&Module> {
        self.modules.iter().filter(|m| m.chips.iter().any(|c| c == chip_id)).collect()
    }

    /// Cross-link integrity: every `project.modules` resolves to a module, every `module.chips` to a
    /// chip, and ids are unique. Empty result = consistent.
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let dup = |kind: &str, ids: Vec<&str>, errs: &mut Vec<String>| {
            let mut seen = std::collections::BTreeSet::new();
            for id in ids {
                if !seen.insert(id) {
                    errs.push(format!("duplicate {kind} id '{id}'"));
                }
            }
        };
        dup("project", self.projects.iter().map(|p| p.id.as_str()).collect(), &mut errs);
        dup("module", self.modules.iter().map(|m| m.id.as_str()).collect(), &mut errs);
        dup("chip", self.chips.iter().map(|c| c.id.as_str()).collect(), &mut errs);
        for p in &self.projects {
            for m in &p.modules {
                if self.module(m).is_none() {
                    errs.push(format!("project '{}' -> missing module '{m}'", p.id));
                }
            }
        }
        for m in &self.modules {
            for c in &m.chips {
                if self.chip(c).is_none() {
                    errs.push(format!("module '{}' -> missing chip '{c}'", m.id));
                }
            }
            let mut seen = std::collections::BTreeSet::new();
            for c in &m.cogs {
                if c.trim().is_empty() || c.contains(char::is_whitespace) {
                    errs.push(format!("module '{}' has a malformed cog id '{c}'", m.id));
                } else if !seen.insert(c.as_str()) {
                    errs.push(format!("module '{}' lists cog '{c}' twice", m.id));
                }
            }
            for d in &m.docs {
                if d.label.trim().is_empty() || d.url.trim().is_empty() {
                    errs.push(format!("module '{}' has a doc link with an empty label or url", m.id));
                }
            }
            if m.firmware.as_ref().is_some_and(|f| !f.read_config_key.is_empty() && f.read_with.is_empty()) {
                errs.push(format!("module '{}' firmware names a read_config_key but no read_with", m.id));
            }
        }
        errs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_catalog_parses_and_links_resolve() {
        let c = HwCatalog::bundled();
        assert_eq!(c.schema, 1);
        assert!(!c.projects.is_empty() && !c.modules.is_empty() && !c.chips.is_empty());
        let errs = c.validate();
        assert!(errs.is_empty(), "catalog link integrity: {errs:?}");
    }

    #[test]
    fn links_and_reverse_links_work() {
        let c = HwCatalog::bundled();
        let sd = c.project("sound-detect").expect("sound-detect project");
        assert!(sd.modules.iter().any(|m| m == "ky-038"));
        assert_eq!(c.module("ads1115-board").unwrap().chips, vec!["ads1115".to_string()]);
        // the Seed board is shared across every sensor project
        assert!(c.projects_using_module("pi-zero-2w").len() >= 3);
        // the ADS1115 chip is on its breakout
        assert!(c.modules_with_chip("ads1115").iter().any(|m| m.id == "ads1115-board"));
    }

    #[test]
    fn hardware_to_software_link_resolves_both_ways() {
        let c = HwCatalog::bundled();
        let ld = c.module("hlk-ld2450").expect("hlk-ld2450");
        assert_eq!(ld.cogs, vec!["ld2450-radar".to_string()]);
        let fw = ld.firmware.as_ref().expect("ld2450 firmware facts");
        assert_eq!(fw.read_config_key, "query_firmware");
        assert!(c.modules_for_cog("ld2450-radar").iter().any(|m| m.id == "hlk-ld2450"));
        // a supporting board is shared by two cogs
        let ads = c.module("ads1115-board").unwrap();
        assert!(ads.cogs.iter().any(|x| x == "sen0213-ecg") && ads.cogs.iter().any(|x| x == "sound-detect"));
        assert!(c.modules_for_cog("no-such-cog").is_empty());
    }

    #[test]
    fn validate_flags_bad_cog_links_and_docs() {
        let mut c = HwCatalog::default();
        c.modules.push(Module {
            id: "m".into(),
            cogs: vec!["a".into(), "a".into(), "bad id".into()],
            docs: vec![DocLink { label: "x".into(), url: String::new() }],
            firmware: Some(Firmware { read_config_key: "k".into(), ..Default::default() }),
            ..Default::default()
        });
        let errs = c.validate();
        assert_eq!(errs.len(), 4, "{errs:?}");
    }

    #[test]
    fn doc_link_web_detection() {
        assert!(DocLink { label: "a".into(), url: "https://x.y".into() }.is_web());
        assert!(!DocLink { label: "a".into(), url: "docs/hardware/radar-protocols.md".into() }.is_web());
    }
}
