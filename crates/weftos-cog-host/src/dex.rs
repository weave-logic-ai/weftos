//! Hardware Dex persistence + catch rules (`<root>/hw/dex.json`). Rules (numbering, rarity, badges)
//! live in `weftos_cog_market::dex`; this module decides what counts as *caught*:
//!
//! 1. a USB scan matches a catalog module/chip through the id table (or a user override), or
//! 2. the user confirms an identification (`POST /hw/dex/catch`) - linking a device to a catalog id
//!    (stored as a vid:pid override so it auto-matches next time) or registering a wild species.
//!
//! Nothing is ever marked caught without one of those. (A running cog's guide/status reporting a
//! module present is not wired: `/status` and the guide bundle expose no module list.)
//! `times_seen` counts scans in which the item was present, at most once per minute. Dex numbers are
//! per host (each host numbers the catalog independently, append-only).

use crate::usb::UsbDevice;
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

/// Most characters kept from any user-supplied string (species names, agent answers are capped
/// separately).
pub const NAME_CAP: usize = 80;
/// Sightings of the same item closer together than this count once.
pub const SEEN_WINDOW_S: u64 = 60;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wild {
    pub vid: String,
    pub pid: String,
    /// Only set for a `force`d, product-scoped row.
    #[serde(default)]
    pub product: Option<String>,
    pub name: String,
    #[serde(default)]
    pub answer: String,
    pub registered_at: u64,
    pub node: String,
    pub times_seen: u32,
    pub last_seen: u64,
}

/// A user "Register species" link from a device to a catalog ref. Reads the older plain-string form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Override {
    Ref(String),
    Rec { r: String, #[serde(default)] product: Option<String> },
}

impl Override {
    pub fn r(&self) -> &str {
        match self {
            Override::Ref(r) | Override::Rec { r, .. } => r,
        }
    }
    pub fn product(&self) -> Option<&str> {
        match self {
            Override::Ref(_) => None,
            Override::Rec { product, .. } => product.as_deref(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Dex {
    #[serde(default)]
    pub numbers: BTreeMap<String, u32>,
    #[serde(default)]
    pub caught: BTreeMap<String, CatchRec>,
    /// `vid:pid` (lowercase hex) -> catalog ref, from "Register species".
    #[serde(default)]
    pub overrides: BTreeMap<String, Override>,
    #[serde(default)]
    pub wild: Vec<Wild>,
    /// Catches the console has not acknowledged yet ("NEW CATCH!"), persisted so a stale response
    /// or a closed window cannot lose them. Cleared by `POST /hw/dex/ack`.
    #[serde(default)]
    pub unseen: Vec<String>,
}

pub struct RegisterReq<'a> {
    pub catalog_id: &'a str,
    pub name: &'a str,
    pub answer: &'a str,
    /// Add a product-scoped row even though the device already matches a bundled id row.
    pub force: bool,
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

/// Trim, drop control characters, cap at [`NAME_CAP`] chars.
pub fn clean_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(NAME_CAP).collect()
}

fn load_file(root: &Path) -> Dex {
    let p = path(root);
    let Ok(bytes) = std::fs::read(&p) else { return Dex::default() };
    match serde_json::from_slice(&bytes) {
        Ok(d) => d,
        Err(e) => {
            // never silently overwrite a dex we cannot read: keep it aside
            let bad = p.with_extension(format!("json.bad-{}", crate::usb::now_secs()));
            eprintln!("[cog-host] dex.json unreadable ({e}); moved to {}", bad.display());
            let _ = std::fs::rename(&p, bad);
            Dex::default()
        }
    }
}

/// Read-only view (numbers assigned in memory, nothing written).
pub fn read_dex(root: &Path) -> Dex {
    let mut d = load_file(root);
    rules::assign_numbers(&mut d.numbers, catalog());
    d
}

/// Load-modify-save under one process-wide lock; writes (atomically, 0600) only when something
/// changed or the file does not exist yet.
pub fn with_dex<R>(root: &Path, f: impl FnOnce(&mut Dex) -> R) -> std::io::Result<R> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut dex = load_file(root);
    rules::assign_numbers(&mut dex.numbers, catalog());
    let before = serde_json::to_string(&dex).unwrap_or_default();
    let out = f(&mut dex);
    let after = serde_json::to_string(&dex).unwrap_or_default();
    if before != after || !path(root).exists() {
        crate::auth::write_private(&path(root), after.as_bytes())?;
    }
    Ok(out)
}

impl Dex {
    /// Id-table rows for user-registered devices (overrides and wild species), to append.
    pub fn user_rows(&self, cat: &HwCatalog) -> Vec<UsbId> {
        let parse = |k: &str| -> Option<(u16, u16)> {
            let (v, p) = k.split_once(':')?;
            Some((u16::from_str_radix(v, 16).ok()?, u16::from_str_radix(p, 16).ok()?))
        };
        let mut rows = Vec::new();
        for (k, o) in &self.overrides {
            let r = o.r();
            let (Some((vid, pid)), Some((name, kind))) = (parse(k), rules::describe(cat, r)) else { continue };
            let (module, chip) = match rules::split_ref(r) {
                Some(("module", id)) => (Some(id.to_string()), None),
                Some((_, id)) => (None, Some(id.to_string())),
                None => continue,
            };
            rows.push(UsbId { vid, pid: Some(pid), name, kind, chip, module, notes: "registered by you".into(), product: o.product().map(String::from) });
        }
        for w in &self.wild {
            let (Ok(vid), Ok(pid)) = (u16::from_str_radix(&w.vid, 16), u16::from_str_radix(&w.pid, 16)) else { continue };
            rows.push(UsbId { vid, pid: Some(pid), name: w.name.clone(), kind: "wild".into(), chip: None, module: None, notes: "wild species (not in the catalog)".into(), product: w.product.clone() });
        }
        rows
    }

    /// First sighting catches (and queues a NEW CATCH); later ones count at most once per
    /// [`SEEN_WINDOW_S`]. Returns true on a first catch.
    fn mark(&mut self, r: &str, via: &str, node: &str, now: u64) -> bool {
        match self.caught.get_mut(r) {
            Some(c) => {
                if now.saturating_sub(c.last_seen) >= SEEN_WINDOW_S {
                    c.times_seen += 1;
                    c.last_seen = now;
                }
                false
            }
            None => {
                self.caught.insert(r.to_string(), CatchRec { first_caught_at: now, node: node.into(), times_seen: 1, last_seen: now, via: via.into() });
                if !self.unseen.iter().any(|u| u == r) {
                    self.unseen.push(r.to_string());
                }
                true
            }
        }
    }

    pub fn catch_json(&self, cat: &HwCatalog, r: &str) -> Value {
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

    /// Pending NEW CATCH banners.
    pub fn unseen_json(&self, cat: &HwCatalog) -> Vec<Value> {
        self.unseen.iter().filter(|r| rules::exists(cat, r)).map(|r| self.catch_json(cat, r)).collect()
    }

    /// Acknowledge banners: the given refs, or all of them.
    pub fn ack(&mut self, refs: Option<&[String]>) {
        match refs {
            Some(rs) => self.unseen.retain(|u| !rs.contains(u)),
            None => self.unseen.clear(),
        }
    }

    /// Record one scan: every catalog item matched by an attached device is caught (first time) or
    /// seen again. `table` must already include the user rows.
    pub fn record_scan(&mut self, devices: &[UsbDevice], table: &UsbIdTable, cat: &HwCatalog, node: &str, now: u64) {
        let mut refs = BTreeSet::new();
        let mut present = BTreeSet::new();
        for d in devices {
            if let Some(id) = table.lookup_device(d.vid, d.pid, &d.product) {
                for r in [id.module.as_deref().map(rules::module_ref), id.chip.as_deref().map(rules::chip_ref)].into_iter().flatten() {
                    if rules::exists(cat, &r) {
                        refs.insert(r);
                    }
                }
            }
            present.insert(vp(d.vid, d.pid));
        }
        for r in refs {
            self.mark(&r, "usb", node, now);
        }
        for w in &mut self.wild {
            if present.contains(&format!("{}:{}", w.vid, w.pid)) && now.saturating_sub(w.last_seen) >= SEEN_WINDOW_S {
                w.times_seen += 1;
                w.last_seen = now;
            }
        }
    }

    /// User confirmation (`POST /hw/dex/catch`): link `dev` to `catalog_id` (a ref) or register it
    /// as a wild species (`"wild"`, needs a name).
    ///
    /// A device that already matches a *bundled* id row is refused (a user row must never shadow
    /// or relabel a known part). `force` is the escape hatch for a spoofed or ambiguous match: it
    /// adds a row scoped to this device's exact product string, which is more specific than the
    /// bundled one without replacing it.
    pub fn register(&mut self, dev: &UsbDevice, req: &RegisterReq, bundled: &UsbIdTable, cat: &HwCatalog, node: &str, now: u64) -> Result<Value, String> {
        let key = vp(dev.vid, dev.pid);
        let mut product = None;
        if let Some(row) = bundled.lookup_device(dev.vid, dev.pid, &dev.product) {
            if !req.force {
                return Err(format!("device already identified as '{}'; pass force=true to add a product-specific row", row.name));
            }
            if dev.product.trim().is_empty() {
                return Err("force needs the device's product string to scope the row".into());
            }
            product = Some(dev.product.clone());
        }
        if req.catalog_id == "wild" {
            let name = clean_name(if req.name.trim().is_empty() { &dev.product } else { req.name });
            if name.is_empty() {
                return Err("a wild species needs a name".into());
            }
            let answer: String = req.answer.chars().filter(|c| !c.is_control() || *c == '\n').take(4000).collect();
            match self.wild.iter_mut().find(|w| format!("{}:{}", w.vid, w.pid) == key && w.product == product) {
                // re-registering renames and refreshes the answer; history is kept
                Some(w) => {
                    w.name = name.clone();
                    w.answer = answer;
                }
                None => self.wild.push(Wild {
                    vid: format!("{:04x}", dev.vid),
                    pid: format!("{:04x}", dev.pid),
                    product,
                    name: name.clone(),
                    answer,
                    registered_at: now,
                    node: node.into(),
                    times_seen: 1,
                    last_seen: now,
                }),
            }
            return Ok(json!({ "wild": true, "name": name }));
        }
        if req.catalog_id.len() > 120 || !rules::exists(cat, req.catalog_id) {
            return Err("unknown catalog id".into());
        }
        self.overrides.insert(key, Override::Rec { r: req.catalog_id.to_string(), product });
        let fresh = self.mark(req.catalog_id, "user", node, now);
        let mut v = self.catch_json(cat, req.catalog_id);
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
            "unseen_catches": self.unseen_json(cat),
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
    (crate::network::node_name(), crate::usb::now_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(vid: u16, pid: u16, product: &str) -> UsbDevice {
        UsbDevice { key: format!("{vid:04x}:{pid:04x}:p-x"), vid, pid, product: product.into(), ..Default::default() }
    }

    fn scan(root: &Path, devs: &[UsbDevice], now: u64) {
        with_dex(root, |d| {
            let t = id_table().with_user_rows(d.user_rows(catalog()));
            d.record_scan(devs, &t, catalog(), "n1", now);
        })
        .unwrap();
    }

    fn reg(root: &Path, d: &UsbDevice, id: &str, name: &str, force: bool, now: u64) -> Result<Value, String> {
        with_dex(root, |x| x.register(d, &RegisterReq { catalog_id: id, name, answer: "ans", force }, id_table(), catalog(), "n1", now)).unwrap()
    }

    fn unseen(root: &Path) -> Vec<String> {
        read_dex(root).unseen
    }

    #[test]
    fn usb_match_catches_module_and_chip_once_then_counts_sightings() {
        let t = tempfile::tempdir().unwrap();
        let rtl = [dev(0x0bda, 0x2838, "RTL2838UHIDIR")];
        scan(t.path(), &rtl, 100);
        let got: BTreeSet<String> = unseen(t.path()).into_iter().collect();
        assert_eq!(got, BTreeSet::from(["module:rtl-sdr".to_string(), "chip:rtl2832u".to_string()]));
        // a rescan within the dedupe window changes nothing; a later one counts
        scan(t.path(), &rtl, 130);
        assert_eq!(read_dex(t.path()).caught["module:rtl-sdr"].times_seen, 1);
        scan(t.path(), &rtl, 200);
        let rec = read_dex(t.path()).caught["module:rtl-sdr"].clone();
        assert_eq!((rec.first_caught_at, rec.last_seen, rec.times_seen, rec.via.as_str(), rec.node.as_str()), (100, 200, 2, "usb", "n1"));
        assert!(path(t.path()).exists());
    }

    #[test]
    fn unseen_catches_persist_until_acked() {
        let t = tempfile::tempdir().unwrap();
        scan(t.path(), &[dev(0x0bda, 0x2838, "x")], 1);
        // a later scan does not re-add or drop them; they survive a "restart" (re-read from disk)
        scan(t.path(), &[dev(0x0bda, 0x2838, "x")], 500);
        assert_eq!(unseen(t.path()).len(), 2);
        let rep = read_dex(t.path()).report(catalog(), id_table(), "n");
        assert_eq!(rep["unseen_catches"].as_array().unwrap().len(), 2);
        with_dex(t.path(), |d| d.ack(Some(&["chip:rtl2832u".to_string()]))).unwrap();
        assert_eq!(unseen(t.path()), vec!["module:rtl-sdr"]);
        with_dex(t.path(), |d| d.ack(None)).unwrap();
        assert!(unseen(t.path()).is_empty());
        assert!(read_dex(t.path()).caught.contains_key("module:rtl-sdr")); // acking never un-catches
    }

    #[test]
    fn read_dex_never_writes() {
        let t = tempfile::tempdir().unwrap();
        let d = read_dex(t.path());
        assert_eq!(d.numbers.len(), catalog().modules.len() + catalog().chips.len());
        assert!(!path(t.path()).exists());
    }

    #[test]
    fn unmatched_and_hub_devices_catch_nothing() {
        let t = tempfile::tempdir().unwrap();
        scan(t.path(), &[dev(0xdead, 0xbeef, "x"), dev(0x0bda, 0x5411, "hub"), dev(0x0403, 0x6001, "ftdi")], 1);
        assert!(read_dex(t.path()).caught.is_empty());
    }

    #[test]
    fn register_species_overrides_then_auto_matches() {
        let t = tempfile::tempdir().unwrap();
        let mystery = dev(0xdead, 0xbeef, "Mystery board");
        scan(t.path(), std::slice::from_ref(&mystery), 1);
        assert!(read_dex(t.path()).caught.is_empty());
        let r = reg(t.path(), &mystery, "module:ky-038", "", false, 5).unwrap();
        assert_eq!(r["new"], true);
        assert_eq!(unseen(t.path()), vec!["module:ky-038"]);
        scan(t.path(), std::slice::from_ref(&mystery), 100); // the override identifies it, seen again
        let rec = read_dex(t.path()).caught["module:ky-038"].clone();
        assert_eq!((rec.times_seen, rec.via.as_str()), (2, "user"));
        assert!(reg(t.path(), &dev(1, 2, "p"), "module:nope", "", false, 1).is_err()); // bad ids refused
    }

    #[test]
    fn user_rows_cannot_relabel_a_bundled_device() {
        let t = tempfile::tempdir().unwrap();
        let ftdi = dev(0x0403, 0x6001, "FT232R USB UART");
        let e = reg(t.path(), &ftdi, "wild", "Totally Not FTDI", false, 1).unwrap_err();
        assert!(e.contains("already identified"), "{e}");
        assert!(reg(t.path(), &ftdi, "module:ky-038", "", false, 1).is_err());
        // vendor-wildcard bundled rows count too (Arduino 2341:*)
        assert!(reg(t.path(), &dev(0x2341, 0x9999, "Weird"), "wild", "x", false, 1).is_err());
        assert!(read_dex(t.path()).wild.is_empty() && read_dex(t.path()).overrides.is_empty());
        // force adds only a product-scoped row, and needs a product string
        assert!(reg(t.path(), &dev(0x0403, 0x6001, ""), "wild", "x", true, 1).is_err());
        reg(t.path(), &ftdi, "wild", "Custom FT232R rig", true, 2).unwrap();
        let rows = read_dex(t.path()).user_rows(catalog());
        let ext = id_table().with_user_rows(rows);
        assert_eq!(ext.lookup_device(0x0403, 0x6001, "FT232R USB UART").unwrap().name, "Custom FT232R rig"); // more specific
        assert_eq!(ext.lookup_device(0x0403, 0x6001, "Some other product").unwrap().name, "FTDI FT232R USB-UART"); // bundled intact
        assert_eq!(ext.lookup(0x0403, 0x6001).unwrap().name, "FTDI FT232R USB-UART");
    }

    #[test]
    fn wild_species_are_separate_from_catalog_totals_and_keep_history() {
        let t = tempfile::tempdir().unwrap();
        let d0 = dev(0xdead, 0xbeef, "Thing");
        reg(t.path(), &d0, "wild", "My Thing", false, 5).unwrap();
        scan(t.path(), std::slice::from_ref(&d0), 100);
        // re-register: new name/answer, but registered_at and times_seen are kept
        reg(t.path(), &d0, "wild", "Renamed", false, 900).unwrap();
        let w = read_dex(t.path()).wild[0].clone();
        assert_eq!((w.name.as_str(), w.registered_at, w.times_seen), ("Renamed", 5, 2));
        assert_eq!(read_dex(t.path()).wild.len(), 1);
        let rep = read_dex(t.path()).report(catalog(), id_table(), "n1");
        assert_eq!(rep["totals"]["modules"]["caught"], 0);
        assert_eq!(rep["totals"]["modules"]["total"], catalog().modules.len());
        assert!(rep["badges"].as_array().unwrap().iter().any(|b| b["id"] == "naturalist" && b["earned"] == true));
        assert!(reg(t.path(), &dev(0xcafe, 2, ""), "wild", "", false, 1).is_err()); // needs a name
    }

    #[test]
    fn user_strings_are_capped_and_cleaned() {
        let t = tempfile::tempdir().unwrap();
        let d0 = dev(0xdead, 0xbeef, "T");
        reg(t.path(), &d0, "wild", &format!("{}\n\x1b[31m", "n".repeat(500)), false, 1).unwrap();
        let name = read_dex(t.path()).wild[0].name.clone();
        assert_eq!(name.chars().count(), NAME_CAP);
        assert!(!name.chars().any(|c| c.is_control()));
        assert!(reg(t.path(), &d0, &"x".repeat(500), "", false, 1).is_err()); // oversized catalog id
    }

    #[test]
    fn unreadable_dex_is_kept_aside_not_overwritten() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("hw")).unwrap();
        std::fs::write(path(t.path()), b"{not json").unwrap();
        with_dex(t.path(), |_| ()).unwrap();
        let kept = std::fs::read_dir(t.path().join("hw")).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains("json.bad-"));
        assert!(kept);
    }

    #[test]
    fn old_plain_string_overrides_still_load() {
        let d: Dex = serde_json::from_str(r#"{"overrides":{"dead:beef":"module:ky-038"}}"#).unwrap();
        assert_eq!(d.overrides["dead:beef"].r(), "module:ky-038");
        assert_eq!(d.user_rows(catalog()).len(), 1);
    }

    #[test]
    fn report_has_types_with_best_caught_vs_available() {
        let t = tempfile::tempdir().unwrap();
        scan(t.path(), &[dev(0x0bda, 0x2838, "RTL2838UHIDIR")], 10);
        let rep = read_dex(t.path()).report(catalog(), id_table(), "n1");
        let sdr = rep["types"].as_array().unwrap().iter().find(|r| r["type"] == "sdr").unwrap();
        assert_eq!(sdr["caught"], true);
        assert_eq!((sdr["best_caught_grade"].as_str(), sdr["best_available_grade"].as_str()), (Some("C"), Some("A")));
        assert_eq!(sdr["upgrade_available"], true);
        assert!(sdr["members"].as_array().unwrap().iter().any(|m| m["id"] == "module:rtl-sdr" && m["caught"] == true));
        let rtl = rep["caught"].as_array().unwrap().iter().find(|c| c["ref"] == "module:rtl-sdr").unwrap();
        assert_eq!((rtl["grade"].as_str(), rtl["rarity"].as_str()), (Some("C"), Some("common")));
        assert_eq!(rep["types"].as_array().unwrap().iter().find(|r| r["type"] == "imu").unwrap()["caught"], false);
    }

    #[test]
    fn numbering_persists_and_totals_count_catalog() {
        let t = tempfile::tempdir().unwrap();
        let a = with_dex(t.path(), |d| d.numbers.clone()).unwrap();
        let b = with_dex(t.path(), |d| d.numbers.clone()).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), catalog().modules.len() + catalog().chips.len());
        assert_eq!(a[&format!("module:{}", catalog().modules[0].id)], 1);
        scan(t.path(), &[dev(0x1d6b, 0x0104, "Cognitum Seed")], 3);
        let rep = read_dex(t.path()).report(catalog(), id_table(), "n1");
        assert_eq!(rep["totals"]["modules"]["caught"], 1);
        assert_eq!(rep["caught"][0]["ref"], "module:pi-zero-2w");
        assert!(rep["badges"].as_array().unwrap().iter().any(|b| b["id"] == "first-catch" && b["earned"] == true));
    }
}
