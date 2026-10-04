//! Sensor hook-up guides shipped with the catalog (ADR-104, ADR-107).
//!
//! A user has to be able to read how to wire a sensor *before* installing its cog, with no host
//! and no device. Each sensor cog's `guide/` folder is bundled into `catalog/guides/<cog id>.json`
//! by `scripts/bundle-cog-guides.py` (the same JSON a running cog serves at `GET /guide`) and
//! embedded here, so the console works offline and before install. This crate only hands out the
//! JSON text; `weftos-sensor-guide` parses and renders it.

/// `(cog id, bundle JSON)` for every bundled guide.
pub const GUIDES: &[(&str, &str)] = &[
    ("bridge", include_str!("../catalog/guides/bridge.json")),
    ("hlk-as201", include_str!("../catalog/guides/hlk-as201.json")),
    ("ld2450-radar", include_str!("../catalog/guides/ld2450-radar.json")),
    ("mentra-live", include_str!("../catalog/guides/mentra-live.json")),
    ("rd-03e", include_str!("../catalog/guides/rd-03e.json")),
    ("sen0213-ecg", include_str!("../catalog/guides/sen0213-ecg.json")),
    ("sen0628-tof", include_str!("../catalog/guides/sen0628-tof.json")),
    ("sound-detect", include_str!("../catalog/guides/sound-detect.json")),
];

/// The bundled guide JSON for a cog, if one ships with the catalog.
pub fn bundled(cog_id: &str) -> Option<&'static str> {
    GUIDES.iter().find(|(id, _)| *id == cog_id).map(|(_, j)| *j)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_the_guides_directory() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog/guides");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".json").map(str::to_string))
            .collect();
        on_disk.sort();
        let table: Vec<String> = GUIDES.iter().map(|(id, _)| id.to_string()).collect();
        assert_eq!(on_disk, table, "update GUIDES after running scripts/bundle-cog-guides.py");
    }

    #[test]
    fn every_bundle_is_json_with_toml_and_pages() {
        for (id, j) in GUIDES {
            let v: serde_json::Value = serde_json::from_str(j).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(v["toml"].is_string() && v["pages"].is_object(), "{id}");
        }
    }

    #[test]
    fn every_linked_sensor_cog_with_a_guide_resolves() {
        let cat = crate::hw::HwCatalog::bundled();
        for m in &cat.modules {
            for c in &m.cogs {
                // ld6002-radar has no guide yet; every other linked cog does
                if c != "ld6002-radar" {
                    assert!(bundled(c).is_some(), "module {} links cog {c} with no bundled guide", m.id);
                }
            }
        }
        assert!(bundled("ld2450-radar").is_some());
        assert!(bundled("nope").is_none());
    }
}
