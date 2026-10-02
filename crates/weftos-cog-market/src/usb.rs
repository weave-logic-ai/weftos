//! USB id table: maps `vid:pid` to a friendly name, a device kind and (where we know it) the
//! hardware-catalog module/chip it corresponds to. Pure data (embedded `catalog/usb_ids.json`), so it
//! works on native and wasm. The host's `/hw/usb` scan uses it to label what is plugged in.

use crate::hw::{Chip, HwCatalog, Module};
use serde::{Deserialize, Serialize};

/// The table shipped with this crate (`catalog/usb_ids.json`).
pub const USB_IDS_JSON: &str = include_str!("../catalog/usb_ids.json");

/// One id-table row. `pid == None` means "any product from this vendor".
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UsbId {
    pub vid: u16,
    pub pid: Option<u16>,
    pub name: String,
    /// usb-uart | mcu | sdr | tpu | debug-probe | hub | storage | hid | camera | audio
    pub kind: String,
    pub chip: Option<String>,
    pub module: Option<String>,
    pub notes: String,
}

#[derive(Deserialize)]
struct RawEntry {
    vid: String,
    pid: String,
    name: String,
    kind: String,
    #[serde(default)]
    chip: Option<String>,
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    notes: String,
}

#[derive(Deserialize)]
struct RawTable {
    #[serde(default)]
    entries: Vec<RawEntry>,
}

/// Catalog objects a [`UsbId`] points at (either may be absent).
#[derive(Debug, Default)]
pub struct CatalogRefs<'a> {
    pub module: Option<&'a Module>,
    pub chip: Option<&'a Chip>,
}

#[derive(Clone, Debug, Default)]
pub struct UsbIdTable {
    pub entries: Vec<UsbId>,
}

fn hex16(s: &str) -> Result<u16, String> {
    u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).map_err(|e| format!("bad hex '{s}': {e}"))
}

impl UsbIdTable {
    pub fn parse(json: &str) -> Result<Self, String> {
        let raw: RawTable = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut entries = Vec::with_capacity(raw.entries.len());
        for e in raw.entries {
            entries.push(UsbId {
                vid: hex16(&e.vid)?,
                pid: if e.pid == "*" { None } else { Some(hex16(&e.pid)?) },
                name: e.name,
                kind: e.kind,
                chip: e.chip,
                module: e.module,
                notes: e.notes,
            });
        }
        Ok(Self { entries })
    }

    /// The table embedded in this crate.
    pub fn bundled() -> Self {
        Self::parse(USB_IDS_JSON).expect("embedded usb_ids.json is valid")
    }

    /// Exact `vid:pid` wins over a vendor-wide (`pid: *`) row.
    pub fn lookup(&self, vid: u16, pid: u16) -> Option<&UsbId> {
        self.entries
            .iter()
            .find(|e| e.vid == vid && e.pid == Some(pid))
            .or_else(|| self.entries.iter().find(|e| e.vid == vid && e.pid.is_none()))
    }
}

/// Resolve a row's `module` / `chip` ids against the hardware catalog.
pub fn catalog_match<'a>(cat: &'a HwCatalog, id: &UsbId) -> CatalogRefs<'a> {
    CatalogRefs {
        module: id.module.as_deref().and_then(|m| cat.module(m)),
        chip: id.chip.as_deref().and_then(|c| cat.chip(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_parses_and_is_unique() {
        let t = UsbIdTable::bundled();
        assert!(t.entries.len() > 40);
        let mut seen = std::collections::BTreeSet::new();
        for e in &t.entries {
            assert!(seen.insert((e.vid, e.pid)), "duplicate row {:04x}:{:?}", e.vid, e.pid);
        }
    }

    #[test]
    fn every_catalog_ref_exists() {
        let cat = HwCatalog::bundled();
        for e in &UsbIdTable::bundled().entries {
            if let Some(m) = &e.module {
                assert!(cat.module(m).is_some(), "{}: missing module '{m}'", e.name);
            }
            if let Some(c) = &e.chip {
                assert!(cat.chip(c).is_some(), "{}: missing chip '{c}'", e.name);
            }
        }
    }

    #[test]
    fn exact_pid_beats_vendor_wildcard() {
        let t = UsbIdTable::bundled();
        assert_eq!(t.lookup(0x303a, 0x1001).unwrap().name, "Espressif USB-Serial/JTAG (built-in)");
        assert_eq!(t.lookup(0x303a, 0x7777).unwrap().name, "Espressif native USB device");
        assert_eq!(t.lookup(0x2341, 0x0043).unwrap().name, "Arduino Uno R3");
        assert_eq!(t.lookup(0x2341, 0x9999).unwrap().name, "Arduino board");
        assert!(t.lookup(0xdead, 0xbeef).is_none());
    }

    #[test]
    fn catalog_match_resolves_refs() {
        let cat = HwCatalog::bundled();
        let t = UsbIdTable::bundled();
        let r = catalog_match(&cat, t.lookup(0x0bda, 0x2838).unwrap());
        assert_eq!(r.module.unwrap().id, "rtl-sdr");
        assert_eq!(r.chip.unwrap().id, "rtl2832u");
        let none = catalog_match(&cat, t.lookup(0x0403, 0x6001).unwrap());
        assert!(none.module.is_none() && none.chip.is_none());
    }
}
