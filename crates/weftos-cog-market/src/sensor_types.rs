//! Sensor types and grades (pure data, wasm-safe). `catalog/sensor_types.json` rolls the sensor
//! modules and chips of the hardware catalog up into *types* (imu, radar, sdr, ...) and grades each
//! one **C** (hobby), **B** (prosumer), **A** (pro) or **S** (lab/reference). Grades are judgement
//! from the product class plus the datasheet figures already in the catalog spec fields, which the
//! `why` line cites; `confidence` is `low` when the catalog holds no figure for the part.

use crate::hw::HwCatalog;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SENSOR_TYPES_JSON: &str = include_str!("../catalog/sensor_types.json");

/// Ordered worst to best, so `Grade::S > Grade::C`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Grade {
    C,
    B,
    A,
    S,
}

impl Grade {
    pub fn label(self) -> &'static str {
        match self {
            Grade::C => "C",
            Grade::B => "B",
            Grade::A => "A",
            Grade::S => "S",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct SensorType {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub desc: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Assignment {
    /// `module:<id>` | `chip:<id>`
    #[serde(rename = "ref")]
    pub r: String,
    #[serde(rename = "type")]
    pub type_id: String,
    pub grade: Grade,
    pub why: String,
    #[serde(default)]
    pub confidence: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SensorTypes {
    #[serde(default)]
    pub types: Vec<SensorType>,
    #[serde(default)]
    pub assign: Vec<Assignment>,
}

impl SensorTypes {
    pub fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    pub fn bundled() -> Self {
        Self::parse(SENSOR_TYPES_JSON).expect("embedded sensor_types.json is valid")
    }

    pub fn assignment(&self, r: &str) -> Option<&Assignment> {
        self.assign.iter().find(|a| a.r == r)
    }

    pub fn grade_of(&self, r: &str) -> Option<Grade> {
        self.assignment(r).map(|a| a.grade)
    }

    pub fn members(&self, type_id: &str) -> Vec<&Assignment> {
        self.assign.iter().filter(|a| a.type_id == type_id).collect()
    }

    /// ref -> grade, for quick lookups.
    pub fn grade_map(&self) -> BTreeMap<&str, Grade> {
        self.assign.iter().map(|a| (a.r.as_str(), a.grade)).collect()
    }

    /// Every assigned ref exists in the catalog, refs are unique, and every type has a member.
    pub fn validate(&self, cat: &HwCatalog) -> Vec<String> {
        let mut errs = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for a in &self.assign {
            if !crate::dex::exists(cat, &a.r) {
                errs.push(format!("{}: not in the catalog", a.r));
            }
            if !seen.insert(a.r.as_str()) {
                errs.push(format!("{}: assigned twice", a.r));
            }
            if !self.types.iter().any(|t| t.id == a.type_id) {
                errs.push(format!("{}: unknown type '{}'", a.r, a.type_id));
            }
        }
        for t in &self.types {
            if self.members(&t.id).is_empty() {
                errs.push(format!("type '{}' has no members", t.id));
            }
        }
        errs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_validates_against_the_catalog() {
        let t = SensorTypes::bundled();
        let errs = t.validate(&HwCatalog::bundled());
        assert!(errs.is_empty(), "{errs:?}");
        assert!(t.types.len() >= 10);
    }

    #[test]
    fn grades_order_worst_to_best() {
        assert!(Grade::C < Grade::B && Grade::B < Grade::A && Grade::A < Grade::S);
        assert_eq!(Grade::S.label(), "S");
    }

    #[test]
    fn every_sensor_module_is_graded() {
        let cat = HwCatalog::bundled();
        let t = SensorTypes::bundled();
        let missing: Vec<_> = cat.modules.iter().filter(|m| m.kind == "sensor" && t.assignment(&format!("module:{}", m.id)).is_none()).map(|m| m.id.as_str()).collect();
        assert!(missing.is_empty(), "ungraded sensor modules: {missing:?}");
    }

    #[test]
    fn why_cites_something_and_low_confidence_is_marked() {
        let t = SensorTypes::bundled();
        assert!(t.assign.iter().all(|a| a.why.len() > 10 && matches!(a.confidence.as_str(), "low" | "medium")));
        let bme = t.assignment("chip:bme280").unwrap();
        assert!(bme.why.contains("temp="), "{}", bme.why);
        assert_eq!(bme.confidence, "medium");
    }
}
