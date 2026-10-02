//! The guide format (ADR-104, schema 1): `guide.toml` declares the pages and the diagram data;
//! each page is a Markdown file whose first `# ` line is its title and first `> ` line its
//! summary. A cog serves the bundle as JSON at `/guide`: `{"toml": "...", "pages": {id: md}}`.

use serde::Deserialize;
use std::collections::BTreeMap;

pub const SCHEMA: u32 = 1;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct GuideDoc {
    pub schema: u32,
    pub title: String,
    #[serde(default)]
    pub sensor: String,
    #[serde(default)]
    pub cog: String,
    #[serde(default)]
    pub cog_version: String,
    #[serde(default)]
    pub medical: bool,
    pub pages: Vec<String>,
    /// Checklist step id -> page id, for companion apps.
    #[serde(default)]
    pub links: BTreeMap<String, String>,
    #[serde(default)]
    pub header: Option<Header>,
    #[serde(default)]
    pub parts: Vec<Part>,
    #[serde(default)]
    pub wires: Vec<Wire>,
    #[serde(default)]
    pub placements: Vec<Placement>,
    #[serde(default)]
    pub flow: Vec<FlowStep>,
    /// Zone map for matrix sensors (ToF arrays, thermal arrays).
    #[serde(default)]
    pub grid: Option<Grid>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Grid {
    pub rows: u32,
    pub cols: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub top: String,
    #[serde(default)]
    pub bottom: String,
    #[serde(default)]
    pub left: String,
    #[serde(default)]
    pub right: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Header {
    #[serde(default)]
    pub board: String,
    #[serde(default)]
    pub orientation: String,
    #[serde(rename = "use", default)]
    pub used: Vec<HeaderPin>,
    #[serde(default)]
    pub avoid: Vec<AvoidPin>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct HeaderPin {
    pub pin: u8,
    pub name: String,
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub color: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AvoidPin {
    pub pin: u8,
    pub why: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Part {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub detail: String,
    pub pins: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Wire {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Placement {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub when: String,
    pub pads: Vec<Pad>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Pad {
    pub label: String,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FlowStep {
    pub name: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GuidePage {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub markdown: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GuideBundle {
    pub doc: GuideDoc,
    /// In `doc.pages` order.
    pub pages: Vec<GuidePage>,
}

/// One piece of a page: Markdown text, or a diagram fence.
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    Text(String),
    Diagram(String),
}

fn title_and_summary(id: &str, md: &str) -> (String, String) {
    let title = md
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(str::trim)
        .unwrap_or(id)
        .to_string();
    let summary = md
        .lines()
        .find_map(|l| l.strip_prefix("> "))
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    (title, summary)
}

impl GuideBundle {
    /// Builds a bundle from `guide.toml` text and `(page id, markdown)` pairs.
    pub fn from_parts(toml_text: &str, pages: &BTreeMap<String, String>) -> Result<Self, String> {
        let doc: GuideDoc = toml::from_str(toml_text).map_err(|e| format!("guide.toml: {e}"))?;
        if doc.schema != SCHEMA {
            return Err(format!(
                "guide.toml schema {} not supported (want {SCHEMA})",
                doc.schema
            ));
        }
        let mut out = Vec::new();
        for id in &doc.pages {
            let md = pages
                .get(id)
                .ok_or_else(|| format!("page '{id}' is listed but missing"))?;
            let (title, summary) = title_and_summary(id, md);
            out.push(GuidePage {
                id: id.clone(),
                title,
                summary,
                markdown: md.clone(),
            });
        }
        Ok(Self { doc, pages: out })
    }

    /// Parses the `/guide` JSON a cog serves: `{"toml": "...", "pages": {id: markdown}}`.
    pub fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        let toml_text = v["toml"].as_str().ok_or("guide JSON has no 'toml'")?;
        let pages = v["pages"]
            .as_object()
            .ok_or("guide JSON has no 'pages'")?
            .iter()
            .filter_map(|(k, md)| md.as_str().map(|s| (k.clone(), s.to_string())))
            .collect();
        Self::from_parts(toml_text, &pages)
    }

    /// Loads `<dir>/guide.toml` and `<dir>/<page>.md` (native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_dir(dir: &std::path::Path) -> Result<Self, String> {
        let toml_text = std::fs::read_to_string(dir.join("guide.toml"))
            .map_err(|e| format!("guide.toml: {e}"))?;
        let doc: GuideDoc = toml::from_str(&toml_text).map_err(|e| format!("guide.toml: {e}"))?;
        let mut pages = BTreeMap::new();
        for id in &doc.pages {
            let md = std::fs::read_to_string(dir.join(format!("{id}.md")))
                .map_err(|e| format!("{id}.md: {e}"))?;
            pages.insert(id.clone(), md);
        }
        Self::from_parts(&toml_text, &pages)
    }

    pub fn page(&self, id: &str) -> Option<&GuidePage> {
        self.pages.iter().find(|p| p.id == id)
    }

    /// Problems that would make the guide wrong or broken. Empty means it is consistent.
    pub fn validate(&self) -> Vec<String> {
        let d = &self.doc;
        let mut errs = Vec::new();
        let page_ids: Vec<&str> = d.pages.iter().map(String::as_str).collect();
        for (step, page) in &d.links {
            if !page_ids.contains(&page.as_str()) {
                errs.push(format!("link '{step}' points at missing page '{page}'"));
            }
        }
        if let Some(h) = &d.header {
            for p in h
                .used
                .iter()
                .map(|p| p.pin)
                .chain(h.avoid.iter().map(|a| a.pin))
            {
                if !(1..=40).contains(&p) {
                    errs.push(format!("header pin {p} is outside 1-40"));
                }
            }
            for u in &h.used {
                if h.avoid.iter().any(|a| a.pin == u.pin) {
                    errs.push(format!("header pin {} is both used and to avoid", u.pin));
                }
            }
        }
        let has_pin = |ep: &str| {
            ep.split_once('.').is_some_and(|(part, pin)| {
                d.parts
                    .iter()
                    .any(|p| p.id == part && p.pins.iter().any(|x| x == pin))
            })
        };
        for w in &d.wires {
            for ep in [&w.from, &w.to] {
                if !has_pin(ep) {
                    errs.push(format!("wire endpoint '{ep}' is not a declared part pin"));
                }
            }
        }
        for pl in &d.placements {
            for pad in &pl.pads {
                if !(0.0..=1.0).contains(&pad.x) || !(0.0..=1.0).contains(&pad.y) {
                    errs.push(format!(
                        "placement '{}' pad {} is outside 0..1",
                        pl.id, pad.label
                    ));
                }
            }
        }
        for p in &self.pages {
            for seg in segments(&p.markdown) {
                if let Segment::Diagram(kind) = seg {
                    let ok = match kind.as_str() {
                        "header" => d.header.is_some(),
                        "wiring" => !d.parts.is_empty(),
                        "placements" => !d.placements.is_empty(),
                        "flow" => !d.flow.is_empty(),
                        "grid" => d.grid.as_ref().is_some_and(|g| {
                            (1..=64).contains(&g.rows) && (1..=64).contains(&g.cols)
                        }),
                        _ => false,
                    };
                    if !ok {
                        errs.push(format!(
                            "page '{}' embeds diagram '{kind}' with no data for it",
                            p.id
                        ));
                    }
                }
            }
        }
        errs
    }
}

/// Splits Markdown into text and ```diagram fences.
pub fn segments(md: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut lines = md.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("```diagram") {
            let mut kind = String::new();
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    break;
                }
                if kind.is_empty() {
                    kind = inner.trim().to_string();
                }
            }
            if !text.trim().is_empty() {
                out.push(Segment::Text(std::mem::take(&mut text)));
            }
            text.clear();
            out.push(Segment::Diagram(kind));
        } else {
            text.push_str(line);
            text.push('\n');
        }
    }
    if !text.trim().is_empty() {
        out.push(Segment::Text(text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str = r#"
schema = 1
title = "Demo"
pages = ["start", "wiring"]
[links]
adc = "wiring"
[header]
use = [{ pin = 1, name = "3V3", color = "red" }]
avoid = [{ pin = 2, why = "5 V" }]
[[parts]]
id = "adc"
name = "ADS1115"
pins = ["VDD"]
[[parts]]
id = "seed"
name = "Seed"
pins = ["1 3V3"]
[[wires]]
from = "adc.VDD"
to = "seed.1 3V3"
"#;

    fn pages() -> BTreeMap<String, String> {
        [
            (
                "start".to_string(),
                "# Start here\n> The short path.\n\nHello.\n".to_string(),
            ),
            (
                "wiring".to_string(),
                "# Wiring\n> Pins.\n\nBefore\n```diagram\nheader\n```\nAfter\n".to_string(),
            ),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn builds_in_page_order_with_titles_and_summaries() {
        let b = GuideBundle::from_parts(TOML, &pages()).unwrap();
        assert_eq!(b.pages.len(), 2);
        assert_eq!(b.pages[0].title, "Start here");
        assert_eq!(b.pages[0].summary, "The short path.");
        assert!(b.validate().is_empty(), "{:?}", b.validate());
    }

    #[test]
    fn missing_pages_and_bad_schema_are_errors() {
        let mut p = pages();
        p.remove("wiring");
        assert!(
            GuideBundle::from_parts(TOML, &p)
                .unwrap_err()
                .contains("missing")
        );
        assert!(
            GuideBundle::from_parts(&TOML.replace("schema = 1", "schema = 9"), &pages()).is_err()
        );
    }

    #[test]
    fn validation_catches_broken_links_wires_pins_and_diagrams() {
        let bad = TOML
            .replace("adc = \"wiring\"", "adc = \"nowhere\"")
            .replace("to = \"seed.1 3V3\"", "to = \"seed.99\"")
            .replace("{ pin = 2, why", "{ pin = 1, why");
        let mut p = pages();
        p.insert(
            "start".into(),
            "# S\n> s\n```diagram\nplacements\n```\n".into(),
        );
        let errs = GuideBundle::from_parts(&bad, &p).unwrap().validate();
        assert_eq!(errs.len(), 4, "{errs:?}");
    }

    #[test]
    fn segments_split_text_and_diagram_fences() {
        let s = segments("A\n```diagram\nwiring\n```\nB\n```rust\nlet x = 1;\n```\n");
        assert_eq!(s.len(), 3);
        assert_eq!(s[1], Segment::Diagram("wiring".into()));
        assert!(matches!(&s[2], Segment::Text(t) if t.contains("```rust")));
    }

    #[test]
    fn display_maps_glyphs_the_default_font_lacks() {
        assert_eq!(crate::displayable("→ see ≥ 0.8 ✓"), "-> see >= 0.8 [ok]");
    }

    #[test]
    fn json_bundle_round_trip() {
        let v = serde_json::json!({ "toml": TOML, "pages": pages() });
        let b = GuideBundle::from_json(&v).unwrap();
        assert_eq!(b.doc.title, "Demo");
        assert!(GuideBundle::from_json(&serde_json::json!({"pages": {}})).is_err());
    }
}
