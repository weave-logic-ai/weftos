//! Unified cog marketplace catalog (COG-009).
//!
//! Merges two sources into one list the manager UI can render:
//!   - the **WeaveLogic** signed registry (`registry.json`, [`weftos_cog_repo::Registry`]) — ours,
//!     Ed25519-signed per arch (COG-008);
//!   - a read-only mirror of **Cognitum**'s `app-registry.json` (107 cogs) — fetched as published,
//!     single armhf binary, unsigned.
//!
//! This crate only parses and merges bytes into a [`Catalog`]; it never touches the network, so it
//! builds for both native and wasm. Callers fetch the bytes their own way (reqwest on native, ehttp
//! in the browser) and hand them in. Signature verification of a WeaveLogic artifact still goes
//! through [`weftos_cog_repo::verify_artifact`] against the pinned key.

pub mod hw;

use serde::{Deserialize, Serialize};
use weftos_cog_repo::Registry as WlRegistry;

/// Which repository a catalog item came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    WeaveLogic,
    Cognitum,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::WeaveLogic => "WeaveLogic",
            Source::Cognitum => "Cognitum",
        }
    }
}

/// One cog as the manager shows it, normalized across sources.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalogItem {
    pub id: String,
    pub name: String,
    pub version: String,
    pub category: String,
    pub description: String,
    pub source: Source,
    /// True only for WeaveLogic cogs (Ed25519-signed, verified before install). Cognitum's are not.
    pub signed: bool,
    /// Arches available, sorted (e.g. `["arm", "arm64"]` for ours, `["arm"]` for Cognitum's).
    pub arches: Vec<String>,
    pub size_kb: Option<u64>,
    /// sha256 of the `arm` binary, for display / integrity (hex).
    pub sha256_arm: Option<String>,
    /// True when a cog with this id also exists in the other source (ours is kept; see [`Catalog::build`]).
    pub also_in_other_source: bool,
}

// ---- Cognitum app-registry.json -------------------------------------------

/// Cognitum's `app-registry.json` (`storage.googleapis.com/cognitum-apps/app-registry.json`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CognitumRegistry {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub updated: String,
    #[serde(default)]
    pub binary_base_url: String,
    #[serde(default)]
    pub cogs: Vec<CognitumCog>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CognitumCog {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub size_kb: Option<u64>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub binary_size: Option<u64>,
}

impl CognitumRegistry {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("parse Cognitum app-registry.json: {e}"))
    }

    /// Where Cognitum publishes a cog's armhf binary, if `binary_base_url` is set.
    pub fn arm_binary_url(&self, id: &str) -> Option<String> {
        if self.binary_base_url.is_empty() {
            None
        } else {
            Some(format!("{}/cogs/arm/cog-{id}-arm", self.binary_base_url.trim_end_matches('/')))
        }
    }

    fn items(&self) -> Vec<CatalogItem> {
        self.cogs
            .iter()
            .map(|c| CatalogItem {
                id: c.id.clone(),
                name: if c.name.is_empty() { c.id.clone() } else { c.name.clone() },
                version: if c.version.is_empty() { "0.0.0".into() } else { c.version.clone() },
                category: c.category.clone(),
                description: c.description.clone(),
                source: Source::Cognitum,
                signed: false,
                arches: vec!["arm".to_string()],
                size_kb: c.size_kb,
                sha256_arm: c.sha256.clone(),
                also_in_other_source: false,
            })
            .collect()
    }
}

// ---- WeaveLogic registry --------------------------------------------------

fn weavelogic_items(reg: &WlRegistry) -> Vec<CatalogItem> {
    reg.cogs
        .iter()
        .map(|c| {
            let mut arches: Vec<String> = c.artifacts.keys().cloned().collect();
            arches.sort();
            let size_kb = c.artifacts.get("arm").or_else(|| c.artifacts.values().next()).map(|a| a.size / 1024);
            let sha256_arm = c.artifacts.get("arm").map(|a| a.sha256.clone());
            CatalogItem {
                id: c.id.clone(),
                name: if c.name.is_empty() { c.id.clone() } else { c.name.clone() },
                version: c.version.clone(),
                category: c.category.clone(),
                description: c.description.clone(),
                source: Source::WeaveLogic,
                signed: true,
                arches,
                size_kb,
                sha256_arm,
                also_in_other_source: false,
            }
        })
        .collect()
}

// ---- the unified catalog --------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// Sorted by (category, id). WeaveLogic wins an id collision; the duplicate is dropped and the
    /// kept item is flagged `also_in_other_source`.
    pub items: Vec<CatalogItem>,
}

impl Catalog {
    /// Build from whichever sources are available. A WeaveLogic cog shadows a Cognitum cog of the same
    /// id (ours is signed and authoritative), and the kept item records that the id exists in both.
    pub fn build(weavelogic: Option<&WlRegistry>, cognitum: Option<&CognitumRegistry>) -> Self {
        let wl = weavelogic.map(weavelogic_items).unwrap_or_default();
        let cg = cognitum.map(CognitumRegistry::items).unwrap_or_default();

        let mut items: Vec<CatalogItem> = Vec::with_capacity(wl.len() + cg.len());
        for mut w in wl {
            if cg.iter().any(|c| c.id == w.id) {
                w.also_in_other_source = true;
            }
            items.push(w);
        }
        for mut c in cg {
            if items.iter().any(|w| w.id == c.id && w.source == Source::WeaveLogic) {
                // shadowed by the signed WeaveLogic cog — drop it
                continue;
            }
            c.also_in_other_source = false;
            items.push(c);
        }
        items.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
        Catalog { items }
    }

    pub fn find(&self, id: &str) -> Option<&CatalogItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn by_source(&self, s: Source) -> impl Iterator<Item = &CatalogItem> {
        self.items.iter().filter(move |i| i.source == s)
    }

    pub fn categories(&self) -> Vec<String> {
        let mut c: Vec<String> = self.items.iter().map(|i| i.category.clone()).filter(|s| !s.is_empty()).collect();
        c.sort();
        c.dedup();
        c
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use weftos_cog_repo::{Artifact, CogEntry, Registry, SCHEMA};

    fn wl_registry() -> Registry {
        let art = |size: u64| Artifact {
            path: "cogs/arm/cog-x-arm".into(),
            size,
            sha256: "ab".repeat(32),
            sig: "cd".repeat(64),
            manifest_path: "cogs/arm/manifest.json".into(),
        };
        Registry {
            schema: SCHEMA,
            repo: "weavelogic".into(),
            updated: "2026-10-02".into(),
            cogs: vec![
                CogEntry {
                    id: "bridge".into(),
                    name: "Sensor Bridge".into(),
                    version: "0.1.0".into(),
                    category: "network".into(),
                    description: "links nodes".into(),
                    hardware_requirement: vec![],
                    artifacts: BTreeMap::from([("arm".into(), art(420956)), ("arm64".into(), art(463560))]),
                },
                // same id as a Cognitum cog below -> ours must win
                CogEntry {
                    id: "fall-detect".into(),
                    name: "Fall Detect (WL)".into(),
                    version: "2.0.0".into(),
                    category: "health".into(),
                    description: "ours".into(),
                    hardware_requirement: vec![],
                    artifacts: BTreeMap::from([("arm".into(), art(100000))]),
                },
            ],
        }
    }

    fn cognitum_json() -> Vec<u8> {
        br#"{
          "version":"2.3.2","updated":"2026-10-01",
          "binary_base_url":"https://storage.googleapis.com/cognitum-apps",
          "cogs":[
            {"id":"sleep-apnea","name":"Sleep Apnea Detector","category":"health","version":"1.2.2","size_kb":394,"sha256":"25","binary_size":403000},
            {"id":"fall-detect","name":"Fall Detect","category":"health","version":"1.0.0","size_kb":200,"sha256":"99","binary_size":205000}
          ]
        }"#
        .to_vec()
    }

    #[test]
    fn cognitum_registry_parses_and_builds_urls() {
        let reg = CognitumRegistry::parse(&cognitum_json()).unwrap();
        assert_eq!(reg.cogs.len(), 2);
        assert_eq!(
            reg.arm_binary_url("sleep-apnea").unwrap(),
            "https://storage.googleapis.com/cognitum-apps/cogs/arm/cog-sleep-apnea-arm"
        );
    }

    #[test]
    fn weavelogic_shadows_cognitum_on_id_collision_and_flags_it() {
        let wl = wl_registry();
        let cg = CognitumRegistry::parse(&cognitum_json()).unwrap();
        let cat = Catalog::build(Some(&wl), Some(&cg));

        // fall-detect exists in both -> exactly one, and it's ours
        let fd: Vec<_> = cat.items.iter().filter(|i| i.id == "fall-detect").collect();
        assert_eq!(fd.len(), 1);
        assert_eq!(fd[0].source, Source::WeaveLogic);
        assert!(fd[0].signed);
        assert!(fd[0].also_in_other_source);
        assert_eq!(fd[0].version, "2.0.0");

        // sleep-apnea only in Cognitum
        let sa = cat.find("sleep-apnea").unwrap();
        assert_eq!(sa.source, Source::Cognitum);
        assert!(!sa.signed);
        assert_eq!(sa.arches, vec!["arm"]);

        // bridge only in WeaveLogic, two arches, size from arm
        let br = cat.find("bridge").unwrap();
        assert_eq!(br.arches, vec!["arm", "arm64"]);
        assert_eq!(br.size_kb, Some(420956 / 1024));
        assert!(!br.also_in_other_source);

        // total = 3 (bridge, fall-detect[ours], sleep-apnea); sorted by (category,id)
        assert_eq!(cat.len(), 3);
        assert_eq!(cat.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["fall-detect", "sleep-apnea", "bridge"]);
        assert_eq!(cat.by_source(Source::WeaveLogic).count(), 2);
        assert_eq!(cat.by_source(Source::Cognitum).count(), 1);
    }

    #[test]
    fn builds_from_a_single_source() {
        let cg = CognitumRegistry::parse(&cognitum_json()).unwrap();
        let cat = Catalog::build(None, Some(&cg));
        assert_eq!(cat.len(), 2);
        assert!(cat.items.iter().all(|i| i.source == Source::Cognitum && !i.signed));
    }
}
