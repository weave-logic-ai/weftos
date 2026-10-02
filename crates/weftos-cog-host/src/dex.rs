//! Hardware Dex persistence + catch rules (`<root>/hw/dex.json`). Rules (numbering, rarity, badges)
//! live in `weftos_cog_market::dex`; this module decides what counts as *caught*:
//!
//! 1. a USB scan matches a catalog module/chip through the id table (or a user override), or
//! 2. the user confirms an identification (`POST /hw/dex/catch`) - linking a device to a catalog id
//!    (stored as a vid:pid override so it auto-matches next time) or registering a wild species.
//!
//! Nothing is ever marked caught without one of those. (A running cog's guide/status reporting a
//! module present is not wired: `/status` and the guide bundle expose no module list.)
//! `times_seen` counts scans in which the item was present.

use crate::usb::{UsbDevice, now_secs};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use weftos_cog_market::dex as rules;
use weftos_cog_market::hw::HwCatalog;
use weftos_cog_market::sensor_types::SensorTypes;
use weftos_cog_market::usb::{UsbId, UsbIdTable};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CatchRec {
    pub first_caught_at: u64,
    pub node: String,
    pub times_seen: u32,
    pub last_seen: u64,
    /// "usb" | "user"
    pub via: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wild {
    pub vid: String,
    pub pid: String,
    pub name: String,
    #[serde(default)]
    pub answer: String,
    pub registered_at: u64,
    pub node: String,
    pub times_seen: u32,
    pub last_seen: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Dex {
    #[serde(default)]
    pub numbers: BTreeMap<String, u32>,
    #[serde(default)]
    pub caught: BTreeMap<String, CatchRec>,
    /// `vid:pid` (lowercase hex) -> catalog ref, from "Register species".
    #[serde(default)]
    pub overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub wild: Vec<Wild>,
}

pub fn path(root: &Path) -> PathBuf {
    root.join("hw").join("dex.json")
}

pub fn catalog() -> &'static HwCatalog {
    static C: OnceLock<HwCatalog> = OnceLock::new();
    C.get_or_init(HwCatalog::bundled)
}

pub fn id_table() -> &'static UsbIdTable {
    static T: OnceLock<UsbIdTable> = OnceLock::new();
    T.get_or_init(UsbIdTable::bundled)
}

pub fn sensor_types() -> &'static SensorTypes {
    static T: OnceLock<SensorTypes> = OnceLock::new();
    T.get_or_init(SensorTypes::bundled)
}

fn vp(vid: u16, pid: u16) -> String {
    format!("{vid:04x}:{pid:04x}")
}

/// Load-modify-save under one process-wide lock; writes (atomically) only when something changed.
pub fn with_dex<R>(root: &Path, f: impl FnOnce(&mut Dex) -> R) -> std::io::Result<R> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut dex: Dex = std::fs::read(path(root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    rules::assign_numbers(&mut dex.numbers, catalog());
    let before = serde_json::to_string(&dex).unwrap_or_default();
    let out = f(&mut dex);
    let after = serde_json::to_string(&dex).unwrap_or_default();
    if before != after || !path(root).exists() {
        let p = path(root);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, after)?;
        std::fs::rename(&tmp, &p)?;
    }
    Ok(out)
}

impl Dex {
    /// Id-table rows for user-registered devices (overrides and wild species), to prepend.
    pub fn user_rows(&self, cat: &HwCatalog) -> Vec<UsbId> {
        let parse = |k: &str| -> Option<(u16, u16)> {
            let (v, p) = k.split_once(':')?;
            Some((u16::from_str_radix(v, 16).ok()?, u16::from_str_radix(p, 16).ok()?))
        };
        let mut rows = Vec::new();
        for (k, r) in &self.overrides {
            let (Some((vid, pid)), Some((name, kind))) = (parse(k), rules::describe(cat, r)) else { continue };
            let (module, chip) = match rules::split_ref(r) {
                Some(("module", id)) => (Some(id.to_string()), None),
                Some((_, id)) => (None, Some(id.to_string())),
                None => continue,
            };
            rows.push(UsbId { vid, pid: Some(pid), name, kind, chip, module, notes: "registered by you".into(), product: None });
        }
        for w in &self.wild {
            let (Ok(vid), Ok(pid)) = (u16::from_str_radix(&w.vid, 16), u16::from_str_radix(&w.pid, 16)) else { continue };
            rows.push(UsbId { vid, pid: Some(pid), name: w.name.clone(), kind: "wild".into(), chip: None, module: None, notes: "wild species (not in the catalog)".into(), product: None });
        }
        rows
    }

    fn mark(&mut self, r: &str, via: &str, node: &str, now: u64) -> bool {
        match self.caught.get_mut(r) {
            Some(c) => {
                c.times_seen += 1;
                c.last_seen = now;
                false
            }
            None => {
                self.caught.insert(r.to_string(), CatchRec { first_caught_at: now, node: node.into(), times_seen: 1, last_seen: now, via: via.into() });
                true
            }
        }
    }

    fn catch_json(&self, cat: &HwCatalog, r: &str) -> Value {
        let (name, kind) = rules::describe(cat, r).unwrap_or_default();
        json!({
            "ref": r,
            "number": self.numbers.get(r),
            "name": name,
            "kind": kind,
            "rarity": rules::rarity_with(cat, sensor_types(), r).map(|x| x.label()),
            "grade": sensor_types().grade_of(r).map(|g| g.label()),
        })
    }

    /// Record one scan: every catalog item matched by an attached device is caught (first time) or
    /// seen again. `table` must already include the user rows. Returns the *new* catches.
    pub fn record_scan(&mut self, devices: &[UsbDevice], table: &UsbIdTable, cat: &HwCatalog, node: &str, now: u64) -> Vec<Value> {
        let mut refs = BTreeSet::new();
        let mut wild_present = BTreeSet::new();
        for d in devices {
            if let Some(id) = table.lookup_device(d.vid, d.pid, &d.product) {
                for r in [id.module.as_deref().map(rules::module_ref), id.chip.as_deref().map(rules::chip_ref)].into_iter().flatten() {
                    if rules::exists(cat, &r) {
                        refs.insert(r);
                    }
                }
            }
            wild_present.insert(vp(d.vid, d.pid));
        }
        let mut fresh = Vec::new();
        for r in refs {
            if self.mark(&r, "usb", node, now) {
                fresh.push(self.catch_json(cat, &r));
            }
        }
        for w in &mut self.wild {
            if wild_present.contains(&format!("{}:{}", w.vid, w.pid)) {
                w.times_seen += 1;
                w.last_seen = now;
            }
        }
        fresh
    }

    /// User confirmation (`POST /hw/dex/catch`): link `dev` to `catalog_id` (a ref) or register it
    /// as a wild species (`catalog_id == "wild"`, needs a name).
    #[allow(clippy::too_many_arguments)]
    pub fn register(&mut self, dev: &UsbDevice, catalog_id: &str, name: &str, answer: &str, cat: &HwCatalog, node: &str, now: u64) -> Result<Value, String> {
        let key = vp(dev.vid, dev.pid);
        if catalog_id == "wild" {
            let name = if name.trim().is_empty() { dev.product.clone() } else { name.trim().to_string() };
            if name.is_empty() {
                return Err("a wild species needs a name".into());
            }
            self.wild.retain(|w| format!("{}:{}", w.vid, w.pid) != key);
            self.wild.push(Wild {
                vid: format!("{:04x}", dev.vid),
                pid: format!("{:04x}", dev.pid),
                name: name.clone(),
                answer: answer.chars().take(4000).collect(),
                registered_at: now,
                node: node.into(),
                times_seen: 1,
                last_seen: now,
            });
            return Ok(json!({ "wild": true, "name": name }));
        }
        if !rules::exists(cat, catalog_id) {
            return Err(format!("unknown catalog id '{catalog_id}'"));
        }
        self.overrides.insert(key, catalog_id.to_string());
        let fresh = self.mark(catalog_id, "user", node, now);
        let mut v = self.catch_json(cat, catalog_id);
        v["new"] = json!(fresh);
        Ok(v)
    }

    /// The `GET /hw/dex` body.
    pub fn report(&self, cat: &HwCatalog, table: &UsbIdTable, node: &str) -> Value {
        let caught: Vec<Value> = self
            .caught
            .iter()
            .filter(|(r, _)| rules::exists(cat, r))
            .map(|(r, c)| {
                let mut v = self.catch_json(cat, r);
                v["first_caught_at"] = json!(c.first_caught_at);
                v["caught_on"] = json!(c.node);
                v["times_seen"] = json!(c.times_seen);
                v["last_seen"] = json!(c.last_seen);
                v["via"] = json!(c.via);
                v
            })
            .collect();
        let have = |kind: &str| self.caught.keys().filter(|r| rules::exists(cat, r) && r.starts_with(kind)).count();
        let caught_at: BTreeMap<String, u64> =
            self.caught.iter().filter(|(r, _)| rules::exists(cat, r)).map(|(r, c)| (r.clone(), c.first_caught_at)).collect();
        json!({
            "ok": true,
            "node": node,
            "caught": caught,
            "wild": self.wild,
            "totals": {
                "modules": { "caught": have("module:"), "total": cat.modules.len() },
                "chips": { "caught": have("chip:"), "total": cat.chips.len() },
            },
            "badges": rules::badges(cat, table, sensor_types(), &caught_at, self.wild.len()),
            "types": rules::type_rows(sensor_types(), &caught_at),
            "numbers": self.numbers,
        })
    }
}

pub fn node_now() -> (String, u64) {
    (crate::network::node_name(), now_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(vid: u16, pid: u16, product: &str) -> UsbDevice {
        UsbDevice { key: format!("{vid:04x}:{pid:04x}:p-x"), vid, pid, product: product.into(), ..Default::default() }
    }

    fn scan(root: &Path, devs: &[UsbDevice], now: u64) -> Vec<Value> {
        with_dex(root, |d| {
            let t = id_table().with_user_rows(d.user_rows(catalog()));
            d.record_scan(devs, &t, catalog(), "n1", now)
        })
        .unwrap()
    }

    #[test]
    fn usb_match_catches_module_and_chip_once_then_counts_sightings() {
        let t = tempfile::tempdir().unwrap();
        let rtl = [dev(0x0bda, 0x2838, "RTL2838UHIDIR")];
        let fresh = scan(t.path(), &rtl, 100);
        let refs: BTreeSet<_> = fresh.iter().map(|v| v["ref"].as_str().unwrap().to_string()).collect();
        assert_eq!(refs, BTreeSet::from(["module:rtl-sdr".to_string(), "chip:rtl2832u".to_string()]));
        assert!(fresh.iter().all(|v| v["number"].as_u64().unwrap() > 0 && v["rarity"].is_string()));
        // second scan: nothing new, but persisted and counted
        assert!(scan(t.path(), &rtl, 200).is_empty());
        let rec = with_dex(t.path(), |d| d.caught["module:rtl-sdr"].clone()).unwrap();
        assert_eq!((rec.first_caught_at, rec.last_seen, rec.times_seen, rec.via.as_str(), rec.node.as_str()), (100, 200, 2, "usb", "n1"));
        assert!(path(t.path()).exists());
    }

    #[test]
    fn unmatched_and_hub_devices_catch_nothing() {
        let t = tempfile::tempdir().unwrap();
        assert!(scan(t.path(), &[dev(0xdead, 0xbeef, "x"), dev(0x0bda, 0x5411, "hub"), dev(0x0403, 0x6001, "ftdi")], 1).is_empty());
        assert!(with_dex(t.path(), |d| d.caught.is_empty()).unwrap());
    }

    #[test]
    fn register_species_overrides_then_auto_matches() {
        let t = tempfile::tempdir().unwrap();
        let mystery = dev(0xdead, 0xbeef, "Mystery board");
        assert!(scan(t.path(), std::slice::from_ref(&mystery), 1).is_empty());
        let r = with_dex(t.path(), |d| d.register(&mystery, "module:ky-038", "", "looks like a KY-038", catalog(), "n1", 5)).unwrap().unwrap();
        assert_eq!(r["new"], true);
        // next scan: the override identifies it (no new catch, but seen again)
        assert!(scan(t.path(), &[mystery], 9).is_empty());
        let rec = with_dex(t.path(), |d| d.caught["module:ky-038"].clone()).unwrap();
        assert_eq!((rec.times_seen, rec.via.as_str()), (2, "user"));
        // bad ids are refused and never caught
        let bad = with_dex(t.path(), |d| d.register(&dev(1, 2, "p"), "module:nope", "", "", catalog(), "n", 1)).unwrap();
        assert!(bad.is_err());
    }

    #[test]
    fn wild_species_are_separate_from_catalog_totals() {
        let t = tempfile::tempdir().unwrap();
        let d0 = dev(0xdead, 0xbeef, "Thing");
        with_dex(t.path(), |d| d.register(&d0, "wild", "My Thing", "answer", catalog(), "n1", 5)).unwrap().unwrap();
        scan(t.path(), std::slice::from_ref(&d0), 8);
        let rep = with_dex(t.path(), |d| d.report(catalog(), id_table(), "n1")).unwrap();
        assert_eq!(rep["wild"][0]["name"], "My Thing");
        assert_eq!(rep["wild"][0]["times_seen"], 2);
        assert_eq!(rep["totals"]["modules"]["caught"], 0);
        assert_eq!(rep["totals"]["modules"]["total"], catalog().modules.len());
        assert!(rep["badges"].as_array().unwrap().iter().any(|b| b["id"] == "naturalist" && b["earned"] == true));
        // wild needs a name when the device has none
        assert!(with_dex(t.path(), |d| d.register(&dev(1, 2, ""), "wild", "", "", catalog(), "n", 1)).unwrap().is_err());
    }

    #[test]
    fn report_has_types_with_best_caught_vs_available() {
        let t = tempfile::tempdir().unwrap();
        // RTL-SDR (grade C sdr module) is caught; bladeRF/USRP (A) are available.
        scan(t.path(), &[dev(0x0bda, 0x2838, "RTL2838UHIDIR")], 10);
        let rep = with_dex(t.path(), |d| d.report(catalog(), id_table(), "n1")).unwrap();
        let sdr = rep["types"].as_array().unwrap().iter().find(|r| r["type"] == "sdr").unwrap();
        assert_eq!(sdr["caught"], true);
        assert_eq!((sdr["best_caught_grade"].as_str(), sdr["best_available_grade"].as_str()), (Some("C"), Some("A")));
        assert_eq!(sdr["upgrade_available"], true);
        assert!(sdr["members"].as_array().unwrap().iter().any(|m| m["id"] == "module:rtl-sdr" && m["caught"] == true));
        let rtl = rep["caught"].as_array().unwrap().iter().find(|c| c["ref"] == "module:rtl-sdr").unwrap();
        assert_eq!((rtl["grade"].as_str(), rtl["rarity"].as_str()), (Some("C"), Some("common")));
        let other = rep["types"].as_array().unwrap().iter().find(|r| r["type"] == "imu").unwrap();
        assert_eq!(other["caught"], false);
    }

    #[test]
    fn numbering_persists_and_totals_count_catalog() {
        let t = tempfile::tempdir().unwrap();
        let a = with_dex(t.path(), |d| d.numbers.clone()).unwrap();
        let b = with_dex(t.path(), |d| d.numbers.clone()).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), catalog().modules.len() + catalog().chips.len());
        let first = &catalog().modules[0].id;
        assert_eq!(a[&format!("module:{first}")], 1);
        scan(t.path(), &[dev(0x1d6b, 0x0104, "Cognitum Seed")], 3);
        let rep = with_dex(t.path(), |d| d.report(catalog(), id_table(), "n1")).unwrap();
        assert_eq!(rep["totals"]["modules"]["caught"], 1);
        assert_eq!(rep["caught"][0]["ref"], "module:pi-zero-2w");
        assert!(rep["badges"].as_array().unwrap().iter().any(|b| b["id"] == "first-catch" && b["earned"] == true));
    }
}
