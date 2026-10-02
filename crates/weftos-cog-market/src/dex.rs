//! Hardware Dex rules (pure data, wasm-safe): stable numbering, rarity tiers and badges over the
//! hardware catalog. Persistence and catch detection live in the host (`weftos-cog-host::dex`).
//!
//! A *ref* names a catalog item: `module:<id>` or `chip:<id>`.
//!
//! - **Numbering** is append-only: an item keeps the number it was first given (stored by the host);
//!   new catalog items get the next free number, in catalog order (modules, then chips).
//! - **Rarity** is derived from how widely the item is used: for a module, the number of projects
//!   that use it; for a chip, the number of modules that carry it. `>=3` common, `2` uncommon,
//!   `1` rare, `0` legendary (catalogued but nothing uses it yet).
//! - For sensor *chips* and `sensor`-kind modules that have a grade (`sensor_types.json`), rarity follows
//!   the grade instead: S legendary, A rare, B uncommon, C common. Boards and other modules keep the
//!   usage rule.
//! - **Badges** come from catalog fields (module `kind`, USB id-table kinds) - no hardcoded ids.

use crate::hw::HwCatalog;
use crate::sensor_types::{Grade, SensorTypes};
use crate::usb::UsbIdTable;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Legendary,
}

impl Rarity {
    pub fn from_uses(uses: usize) -> Self {
        match uses {
            0 => Rarity::Legendary,
            1 => Rarity::Rare,
            2 => Rarity::Uncommon,
            _ => Rarity::Common,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Rarity::Common => "common",
            Rarity::Uncommon => "uncommon",
            Rarity::Rare => "rare",
            Rarity::Legendary => "legendary",
        }
    }
}

pub fn module_ref(id: &str) -> String {
    format!("module:{id}")
}
pub fn chip_ref(id: &str) -> String {
    format!("chip:{id}")
}
pub fn split_ref(r: &str) -> Option<(&str, &str)> {
    r.split_once(':').filter(|(k, _)| matches!(*k, "module" | "chip"))
}

/// Every catalog ref in catalog order: all modules, then all chips.
pub fn all_refs(cat: &HwCatalog) -> Vec<String> {
    cat.modules.iter().map(|m| module_ref(&m.id)).chain(cat.chips.iter().map(|c| chip_ref(&c.id))).collect()
}

/// Give every catalog item without a number the next free one. Existing numbers never change, so
/// catalog additions (and removals) cannot renumber anything.
pub fn assign_numbers(numbers: &mut BTreeMap<String, u32>, cat: &HwCatalog) {
    let mut next = numbers.values().max().copied().unwrap_or(0) + 1;
    for r in all_refs(cat) {
        numbers.entry(r).or_insert_with(|| {
            next += 1;
            next - 1
        });
    }
}

pub fn exists(cat: &HwCatalog, r: &str) -> bool {
    match split_ref(r) {
        Some(("module", id)) => cat.module(id).is_some(),
        Some(("chip", id)) => cat.chip(id).is_some(),
        _ => false,
    }
}

pub fn rarity(cat: &HwCatalog, r: &str) -> Option<Rarity> {
    match split_ref(r)? {
        ("module", id) => cat.module(id).map(|_| Rarity::from_uses(cat.projects_using_module(id).len())),
        (_, id) => cat.chip(id).map(|_| Rarity::from_uses(cat.modules_with_chip(id).len())),
    }
}

/// Rarity with the sensor-grade rule layered on top of [`rarity`].
pub fn rarity_with(cat: &HwCatalog, types: &SensorTypes, r: &str) -> Option<Rarity> {
    let base = rarity(cat, r)?;
    let graded = match split_ref(r) {
        Some(("module", id)) => cat.module(id).is_some_and(|m| m.kind == "sensor"),
        _ => true,
    };
    Some(match types.grade_of(r).filter(|_| graded) {
        Some(Grade::S) => Rarity::Legendary,
        Some(Grade::A) => Rarity::Rare,
        Some(Grade::B) => Rarity::Uncommon,
        Some(Grade::C) => Rarity::Common,
        None => base,
    })
}

/// `(name, kind)`; the kind of a chip is `"chip"`.
pub fn describe(cat: &HwCatalog, r: &str) -> Option<(String, String)> {
    match split_ref(r)? {
        ("module", id) => cat.module(id).map(|m| (m.name.clone(), m.kind.clone())),
        (_, id) => cat.chip(id).map(|c| (c.name.clone(), "chip".to_string())),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Badge {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub earned: bool,
    pub have: usize,
    pub need: usize,
}

fn badge(id: String, name: String, desc: String, have: usize, need: usize) -> Badge {
    Badge { id, name, desc, earned: have >= need, have: have.min(need), need }
}

/// One row of the "By type" dex view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeRow {
    #[serde(rename = "type")]
    pub type_id: String,
    pub name: String,
    pub caught: bool,
    pub best_caught_grade: Option<Grade>,
    pub best_available_grade: Option<Grade>,
    /// A higher grade exists in the catalog than the best one caught (only when something is caught).
    pub upgrade_available: bool,
    pub members: Vec<TypeMember>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeMember {
    pub id: String,
    pub grade: Grade,
    pub caught: bool,
}

/// Per-type roll-up over the caught refs (`caught` maps ref -> first_caught_at).
pub fn type_rows(types: &SensorTypes, caught: &BTreeMap<String, u64>) -> Vec<TypeRow> {
    types
        .types
        .iter()
        .map(|t| {
            let mut members: Vec<TypeMember> =
                types.members(&t.id).into_iter().map(|a| TypeMember { id: a.r.clone(), grade: a.grade, caught: caught.contains_key(&a.r) }).collect();
            members.sort_by(|a, b| b.grade.cmp(&a.grade).then(a.id.cmp(&b.id)));
            let best_caught = members.iter().filter(|m| m.caught).map(|m| m.grade).max();
            let best_avail = members.iter().map(|m| m.grade).max();
            TypeRow {
                type_id: t.id.clone(),
                name: t.name.clone(),
                caught: best_caught.is_some(),
                upgrade_available: best_caught.is_some_and(|c| best_avail.is_some_and(|a| a > c)),
                best_caught_grade: best_caught,
                best_available_grade: best_avail,
                members,
            }
        })
        .collect()
}

/// Types where a higher-grade member was caught *after* a lower-grade one ("grade up").
pub fn upgraded_types(types: &SensorTypes, caught: &BTreeMap<String, u64>) -> usize {
    types
        .types
        .iter()
        .filter(|t| {
            let mut got: Vec<(u64, Grade)> = types.members(&t.id).into_iter().filter_map(|a| caught.get(&a.r).map(|at| (*at, a.grade))).collect();
            got.sort();
            let mut best: Option<Grade> = None;
            got.iter().any(|(_, g)| {
                let up = best.is_some_and(|b| *g > b);
                best = Some(best.map_or(*g, |b| b.max(*g)));
                up
            })
        })
        .count()
}

/// Badges over the caught refs (`caught` maps ref -> first_caught_at). Fixed ones: first catch,
/// collector (10), legendary find, naturalist (a wild species), first S-grade, 5 types caught, full
/// house (every sensor type caught at any grade), grade up (a type upgraded). Per module `kind` with
/// at least 3 modules: "Hat Trick" (3 caught). Per USB id kind that links at least 2 catalog
/// modules: collect them all.
pub fn badges(cat: &HwCatalog, table: &UsbIdTable, types: &SensorTypes, caught: &BTreeMap<String, u64>, wild: usize) -> Vec<Badge> {
    let rows = type_rows(types, caught);
    let types_caught = rows.iter().filter(|r| r.caught).count();
    let mut out = vec![
        badge("first-catch".into(), "First Catch".into(), "catch any catalog item".into(), caught.len(), 1),
        badge("collector".into(), "Collector".into(), "catch 10 catalog items".into(), caught.len(), 10),
        badge(
            "legendary".into(),
            "Legendary Find".into(),
            "catch a legendary-tier item".into(),
            caught.keys().filter(|r| rarity_with(cat, types, r) == Some(Rarity::Legendary)).count(),
            1,
        ),
        badge("naturalist".into(), "Naturalist".into(), "register a wild species".into(), wild, 1),
        badge("first-s-grade".into(), "First S-Grade".into(), "catch an S-grade (lab/reference) sensor".into(), caught.keys().filter(|r| types.grade_of(r) == Some(Grade::S)).count(), 1),
        badge("five-types".into(), "Five Types".into(), "catch sensors of 5 different types".into(), types_caught, 5),
        badge("full-house".into(), "Full House".into(), "catch every sensor type at any grade".into(), types_caught, rows.len().max(1)),
        badge("grade-up".into(), "Grade Up".into(), "catch a higher-grade sensor of a type you already had".into(), upgraded_types(types, caught), 1),
    ];
    let kinds: BTreeSet<&str> = cat.modules.iter().map(|m| m.kind.as_str()).filter(|k| !k.is_empty()).collect();
    for k in kinds {
        let total = cat.modules.iter().filter(|m| m.kind == k).count();
        if total >= 3 {
            let have = cat.modules.iter().filter(|m| m.kind == k && caught.contains_key(&module_ref(&m.id))).count();
            out.push(badge(format!("hat-trick:{k}"), format!("Hat Trick: {k}"), format!("catch 3 {k} modules"), have, 3));
        }
    }
    let mut by_usb_kind: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in &table.entries {
        if let Some(m) = e.module.as_deref().filter(|m| cat.module(m).is_some()) {
            by_usb_kind.entry(e.kind.as_str()).or_default().insert(m);
        }
    }
    for (k, mods) in by_usb_kind.into_iter().filter(|(_, m)| m.len() >= 2) {
        let have = mods.iter().filter(|m| caught.contains_key(&module_ref(m))).count();
        out.push(badge(format!("usb-set:{k}"), format!("Full Set: {k}"), format!("catch every {k} module the USB table can identify"), have, mods.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mini(extra: bool) -> HwCatalog {
        let mut mods = vec![
            r#"{"id":"a","name":"A","kind":"sensor","chips":["c1"]}"#,
            r#"{"id":"b","name":"B","kind":"sensor","chips":["c1"]}"#,
            r#"{"id":"c","name":"C","kind":"sensor"}"#,
        ];
        if extra {
            mods.insert(1, r#"{"id":"new","name":"New","kind":"board"}"#);
        }
        let j = format!(
            r#"{{"schema":1,"projects":[{{"id":"p1","name":"P1","modules":["a","b"]}},{{"id":"p2","name":"P2","modules":["a"]}}],
               "modules":[{}],"chips":[{{"id":"c1","name":"C1"}}]}}"#,
            mods.join(",")
        );
        HwCatalog::parse(j.as_bytes()).unwrap()
    }

    #[test]
    fn numbers_are_stable_across_catalog_additions() {
        let mut n = BTreeMap::new();
        assign_numbers(&mut n, &mini(false));
        assert_eq!(n["module:a"], 1);
        assert_eq!(n["module:c"], 3);
        assert_eq!(n["chip:c1"], 4);
        let before = n.clone();
        // a new item lands in the middle of the catalog: it gets the next number, nothing moves
        assign_numbers(&mut n, &mini(true));
        for (k, v) in &before {
            assert_eq!(n[k], *v, "{k} was renumbered");
        }
        assert_eq!(n["module:new"], 5);
        // idempotent
        let again = n.clone();
        assign_numbers(&mut n, &mini(true));
        assert_eq!(n, again);
    }

    #[test]
    fn rarity_follows_usage() {
        let c = mini(false);
        assert_eq!(rarity(&c, "module:a"), Some(Rarity::Uncommon)); // 2 projects
        assert_eq!(rarity(&c, "module:b"), Some(Rarity::Rare)); // 1 project
        assert_eq!(rarity(&c, "module:c"), Some(Rarity::Legendary)); // unused
        assert_eq!(rarity(&c, "chip:c1"), Some(Rarity::Uncommon)); // carried by 2 modules
        assert_eq!(rarity(&c, "module:zzz"), None);
        assert_eq!(rarity(&c, "bogus"), None);
    }

    #[test]
    fn badges_are_data_driven() {
        let c = mini(false);
        let t = UsbIdTable::default();
        let st = SensorTypes::default();
        let mut caught: BTreeMap<String, u64> = BTreeMap::new();
        let b = badges(&c, &t, &st, &caught, 0);
        assert!(b.iter().all(|x| !x.earned));
        assert!(b.iter().any(|x| x.id == "hat-trick:sensor" && x.need == 3));
        caught.insert("module:c".to_string(), 1); // legendary + first catch
        let b = badges(&c, &t, &st, &caught, 1);
        let earned: Vec<&str> = b.iter().filter(|x| x.earned).map(|x| x.id.as_str()).collect();
        assert!(earned.contains(&"first-catch") && earned.contains(&"legendary") && earned.contains(&"naturalist"));
        assert!(!earned.contains(&"collector") && !earned.contains(&"hat-trick:sensor"));
        caught.extend([("module:a".to_string(), 2), ("module:b".to_string(), 3)]);
        let b = badges(&c, &t, &st, &caught, 0);
        assert!(b.iter().find(|x| x.id == "hat-trick:sensor").unwrap().earned);
    }

    #[test]
    fn bundled_usb_set_badge_exists_for_sdr() {
        let b = badges(&HwCatalog::bundled(), &UsbIdTable::bundled(), &SensorTypes::bundled(), &BTreeMap::new(), 0);
        assert!(b.iter().any(|x| x.id == "usb-set:sdr" && x.need >= 2));
        assert!(b.len() >= 5 && b.len() <= 20, "{} badges", b.len());
    }

    fn two_types() -> SensorTypes {
        SensorTypes::parse(
            r#"{"types":[{"id":"t1","name":"T1"},{"id":"t2","name":"T2"}],"assign":[
              {"ref":"module:a","type":"t1","grade":"C","why":"x"},
              {"ref":"module:b","type":"t1","grade":"A","why":"x"},
              {"ref":"module:c","type":"t1","grade":"S","why":"x"},
              {"ref":"chip:c1","type":"t2","grade":"B","why":"x"}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn best_caught_vs_best_available_and_upgrade_hint() {
        let st = two_types();
        let mut caught = BTreeMap::new();
        let rows = type_rows(&st, &caught);
        assert!(rows.iter().all(|r| !r.caught && !r.upgrade_available));
        assert_eq!(rows[0].best_available_grade, Some(Grade::S));
        assert_eq!(rows[0].members[0].id, "module:c"); // best grade first
        caught.insert("module:a".to_string(), 10);
        let rows = type_rows(&st, &caught);
        assert!(rows[0].caught && rows[0].upgrade_available);
        assert_eq!((rows[0].best_caught_grade, rows[0].best_available_grade), (Some(Grade::C), Some(Grade::S)));
        caught.insert("module:c".to_string(), 20);
        let rows = type_rows(&st, &caught);
        assert!(!rows[0].upgrade_available && rows[0].best_caught_grade == Some(Grade::S));
        // t2: its only member is caught, nothing better exists
        caught.insert("chip:c1".to_string(), 30);
        assert!(!type_rows(&st, &caught)[1].upgrade_available);
    }

    #[test]
    fn grade_up_needs_a_later_higher_grade() {
        let st = two_types();
        let mut caught = BTreeMap::from([("module:b".to_string(), 5), ("module:a".to_string(), 9)]);
        assert_eq!(upgraded_types(&st, &caught), 0); // lower grade caught later is not an upgrade
        caught.insert("module:c".to_string(), 12);
        assert_eq!(upgraded_types(&st, &caught), 1);
        let b = badges(&mini(false), &UsbIdTable::default(), &st, &caught, 0);
        assert!(b.iter().find(|x| x.id == "grade-up").unwrap().earned);
        assert!(b.iter().find(|x| x.id == "first-s-grade").unwrap().earned);
        assert!(!b.iter().find(|x| x.id == "full-house").unwrap().earned); // t2 not caught
        caught.insert("chip:c1".to_string(), 13);
        let b = badges(&mini(false), &UsbIdTable::default(), &st, &caught, 0);
        assert!(b.iter().find(|x| x.id == "full-house").unwrap().earned);
    }

    #[test]
    fn rarity_leans_on_grade_for_sensors_only() {
        let c = mini(true); // module:new is a "board"
        let st = SensorTypes::parse(
            r#"{"types":[{"id":"t","name":"T"}],"assign":[
              {"ref":"module:a","type":"t","grade":"S","why":"x"},
              {"ref":"module:b","type":"t","grade":"C","why":"x"},
              {"ref":"module:new","type":"t","grade":"C","why":"x"},
              {"ref":"chip:c1","type":"t","grade":"A","why":"x"}]}"#,
        )
        .unwrap();
        assert_eq!(rarity_with(&c, &st, "module:a"), Some(Rarity::Legendary));
        assert_eq!(rarity_with(&c, &st, "module:b"), Some(Rarity::Common));
        assert_eq!(rarity_with(&c, &st, "chip:c1"), Some(Rarity::Rare));
        // a board keeps the usage rule even when graded C (unused -> legendary)
        assert_eq!(rarity_with(&c, &st, "module:new"), Some(Rarity::Legendary));
        assert_eq!(rarity_with(&c, &st, "module:c"), rarity(&c, "module:c")); // ungraded sensor
    }
}
