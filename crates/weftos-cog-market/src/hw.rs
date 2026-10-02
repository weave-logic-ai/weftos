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
}
