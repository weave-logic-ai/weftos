//! The well-known capability vocabulary (`config/capabilities.toml`).
//!
//! ADR-099 section 2, rule 2: the file lists well-known ids and their
//! attribute schemas. It drives **validation warnings**, explain output and
//! docs. It **does not gate**: an id missing from the file is still accepted
//! and matched, and `x.` ids are experimental by convention and draw no
//! warning. Nothing in [`super::requirement`] consults this module.
//!
//! Decision 6 of ADR-099: the file changes only through the governance path.
//! The digest pin, pinned loader and change record live in
//! [`super::vocabulary_pin`]; the gate check and chain event live in the
//! kernel (`clawft_kernel::placement_vocabulary`).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::capability::{
    AttrValue, Capability, CapabilityId, Provenance, valid_attr_name, valid_segment,
};
use super::requirement::{IdSelector, PredicateOp, Requirement};

/// Largest vocabulary file accepted, in bytes.
pub const MAX_VOCAB_BYTES: usize = 256 * 1024;

/// Declared type of an attribute in the vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttrType {
    /// Text.
    String,
    /// Integer (byte counts, core counts).
    Int,
    /// Integer or float.
    Number,
    /// Boolean.
    Bool,
    /// List of scalars.
    List,
}

impl AttrType {
    fn accepts(self, v: &AttrValue) -> bool {
        matches!(
            (self, v),
            (AttrType::String, AttrValue::Str(_))
                | (AttrType::Int, AttrValue::Int(_))
                | (AttrType::Number, AttrValue::Int(_) | AttrValue::Float(_))
                | (AttrType::Bool, AttrValue::Bool(_))
                | (AttrType::List, AttrValue::List(_))
        )
    }

    fn name(self) -> &'static str {
        match self {
            AttrType::String => "string",
            AttrType::Int => "int",
            AttrType::Number => "number",
            AttrType::Bool => "bool",
            AttrType::List => "list",
        }
    }
}

/// File header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VocabularyMeta {
    /// Monotonic version; every governed change bumps it.
    pub version: u32,
    /// Human title.
    pub title: String,
}

/// A family: the first id segment (`accel`, `mem`, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Family {
    /// Display title.
    pub title: String,
    /// Sort key for docs.
    pub order: u32,
}

/// One well-known id or id pattern (`accel.tsu.<vendor>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdEntry {
    /// One-line description.
    pub summary: String,
    /// Attribute schema: name to type.
    #[serde(default)]
    pub attrs: BTreeMap<String, AttrType>,
    /// Provenance this id is expected to carry (for example `measured` for `perf.*`).
    #[serde(default)]
    pub min_provenance: Option<Provenance>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct VocabularyFile {
    meta: VocabularyMeta,
    families: BTreeMap<String, Family>,
    ids: BTreeMap<String, IdEntry>,
}

/// Errors loading the vocabulary file (the file itself is malformed or unpinned).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum VocabularyError {
    /// Larger than [`MAX_VOCAB_BYTES`].
    #[error("vocabulary file is too large ({0} bytes)")]
    TooLarge(usize),
    /// Not valid TOML for this schema.
    #[error("vocabulary file does not parse: {0}")]
    Parse(String),
    /// Structurally invalid content.
    #[error("vocabulary entry {entry:?} is invalid: {reason}")]
    Invalid {
        /// Offending key.
        entry: String,
        /// Why.
        reason: String,
    },
    /// Digest is not the governance-pinned digest.
    #[error("vocabulary digest {actual} is not the pinned digest {expected}")]
    DigestMismatch {
        /// Pinned.
        expected: String,
        /// Actual.
        actual: String,
    },
    /// A proposed change does not bump the version.
    #[error("vocabulary change must increase meta.version ({from} -> {to})")]
    VersionNotIncreased {
        /// Current.
        from: u32,
        /// Proposed.
        to: u32,
    },
}

/// A non-fatal finding. Never blocks acceptance or matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VocabWarning {
    /// Id is neither well-known nor `x.`-prefixed.
    UnknownId(String),
    /// Prefix covers no well-known id and is not `x.`.
    UnknownPrefix(String),
    /// Attribute value or predicate operand does not fit the schema type.
    AttrType {
        /// Capability id.
        id: String,
        /// Attribute.
        attr: String,
        /// Declared type.
        expected: &'static str,
    },
    /// Provenance is below what the vocabulary expects for this id.
    Provenance {
        /// Capability id.
        id: String,
        /// Expected minimum.
        expected: Provenance,
    },
}

impl fmt::Display for VocabWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VocabWarning::UnknownId(id) => write!(
                f,
                "{id}: not in the well-known vocabulary (accepted; use `x.` for experimental ids)"
            ),
            VocabWarning::UnknownPrefix(p) => {
                write!(f, "{p}: prefix covers no well-known id (accepted)")
            }
            VocabWarning::AttrType { id, attr, expected } => {
                write!(f, "{id}.{attr}: expected {expected}")
            }
            VocabWarning::Provenance { id, expected } => {
                write!(f, "{id}: expected provenance {expected:?} or better")
            }
        }
    }
}

/// A parsed, structurally valid vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct Vocabulary {
    meta: VocabularyMeta,
    families: BTreeMap<String, Family>,
    ids: BTreeMap<String, IdEntry>,
    digest: String,
}

fn is_placeholder(seg: &str) -> bool {
    seg.len() > 2
        && seg.starts_with('<')
        && seg.ends_with('>')
        && valid_attr_name(&seg[1..seg.len() - 1])
}

/// SHA-256 hex digest of the exact file bytes.
pub fn digest_of(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Vocabulary {
    /// Parse and structurally validate a vocabulary file.
    pub fn from_toml_str(text: &str) -> Result<Self, VocabularyError> {
        if text.len() > MAX_VOCAB_BYTES {
            return Err(VocabularyError::TooLarge(text.len()));
        }
        let file: VocabularyFile =
            toml::from_str(text).map_err(|e| VocabularyError::Parse(e.to_string()))?;
        let invalid = |entry: &str, reason: &str| VocabularyError::Invalid {
            entry: entry.to_string(),
            reason: reason.to_string(),
        };
        for name in file.families.keys() {
            if !valid_segment(name) || name == "x" {
                return Err(invalid(name, "family must be one id segment and not `x`"));
            }
        }
        for (id, entry) in &file.ids {
            let segs: Vec<&str> = id.split('.').collect();
            if segs.len() < 2 || segs.len() > super::capability::MAX_ID_SEGMENTS {
                return Err(invalid(id, "id needs 2..=8 segments"));
            }
            if !segs.iter().all(|s| valid_segment(s) || is_placeholder(s)) {
                return Err(invalid(
                    id,
                    "segments must be [a-z0-9][a-z0-9_-]* or <name>",
                ));
            }
            if !file.families.contains_key(segs[0]) {
                return Err(invalid(id, "first segment is not a declared family"));
            }
            if let Some(a) = entry.attrs.keys().find(|a| !valid_attr_name(a)) {
                return Err(invalid(
                    id,
                    &format!("attribute {a:?} must match [a-z][a-z0-9_]*"),
                ));
            }
        }
        Ok(Self {
            meta: file.meta,
            families: file.families,
            ids: file.ids,
            digest: digest_of(text),
        })
    }

    /// SHA-256 hex digest of the source text.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Header.
    pub fn meta(&self) -> &VocabularyMeta {
        &self.meta
    }

    /// Families by key.
    pub fn families(&self) -> &BTreeMap<String, Family> {
        &self.families
    }

    /// Entries by id or pattern.
    pub fn ids(&self) -> &BTreeMap<String, IdEntry> {
        &self.ids
    }

    /// Find the entry for an id: exact, else a pattern with `<name>` segments.
    pub fn lookup(&self, id: &str) -> Option<&IdEntry> {
        if let Some(e) = self.ids.get(id) {
            return Some(e);
        }
        let segs: Vec<&str> = id.split('.').collect();
        self.ids.iter().find_map(|(pat, e)| {
            let p: Vec<&str> = pat.split('.').collect();
            let hit = p.len() == segs.len()
                && p.iter()
                    .zip(&segs)
                    .all(|(a, b)| a == b || is_placeholder(a));
            hit.then_some(e)
        })
    }

    fn unknown_id(&self, id: &CapabilityId) -> Option<VocabWarning> {
        (!id.is_experimental() && self.lookup(id.as_str()).is_none())
            .then(|| VocabWarning::UnknownId(id.to_string()))
    }

    /// Warnings for one advertised capability. Never an error.
    pub fn validate_capability(&self, cap: &Capability) -> Vec<VocabWarning> {
        let mut out: Vec<VocabWarning> = self.unknown_id(&cap.id).into_iter().collect();
        if let Some(entry) = self.lookup(cap.id.as_str()) {
            for (attr, value) in &cap.attrs {
                match entry.attrs.get(attr) {
                    Some(t) if !t.accepts(value) => out.push(VocabWarning::AttrType {
                        id: cap.id.to_string(),
                        attr: attr.clone(),
                        expected: t.name(),
                    }),
                    _ => {}
                }
            }
            if let Some(min) = entry.min_provenance.filter(|&min| cap.provenance < min) {
                out.push(VocabWarning::Provenance {
                    id: cap.id.to_string(),
                    expected: min,
                });
            }
        }
        out
    }

    /// Warnings for one requirement. Never an error.
    pub fn validate_requirement(&self, req: &Requirement) -> Vec<VocabWarning> {
        match &req.selector {
            IdSelector::Exact(id) => {
                let mut out: Vec<VocabWarning> = self.unknown_id(id).into_iter().collect();
                if let Some(entry) = self.lookup(id.as_str()) {
                    for p in &req.where_ {
                        let Some(t) = entry.attrs.get(&p.attr) else {
                            continue;
                        };
                        let fits = match p.op {
                            PredicateOp::Gte | PredicateOp::Lte => {
                                matches!(t, AttrType::Int | AttrType::Number)
                            }
                            PredicateOp::Has => *t == AttrType::List,
                            PredicateOp::Eq => t.accepts(&p.value),
                            PredicateOp::In => *t != AttrType::List,
                        };
                        if !fits {
                            out.push(VocabWarning::AttrType {
                                id: id.to_string(),
                                attr: p.attr.clone(),
                                expected: t.name(),
                            });
                        }
                    }
                }
                out
            }
            IdSelector::Prefix(p) => {
                // Segment-wise, with `<name>` placeholders matching any one
                // segment, so `accel.tsu.extropic` is known as a prefix just
                // as it is as an exact id (via `accel.tsu.<vendor>`).
                let want: Vec<&str> = p.split('.').collect();
                let known = want[0] == super::capability::EXPERIMENTAL_PREFIX
                    || self.ids.keys().any(|k| {
                        let have: Vec<&str> = k.split('.').collect();
                        have.len() >= want.len()
                            && want
                                .iter()
                                .zip(&have)
                                .all(|(w, h)| w == h || is_placeholder(h))
                    });
                if known {
                    vec![]
                } else {
                    vec![VocabWarning::UnknownPrefix(p.clone())]
                }
            }
        }
    }

    /// Markdown reference table, grouped by family in `order`, ids sorted.
    /// Deterministic: the committed doc is compared against this output.
    pub fn to_markdown(&self) -> String {
        let mut fams: Vec<(&String, &Family)> = self.families.iter().collect();
        fams.sort_by_key(|(k, f)| (f.order, (*k).clone()));
        let mut s = format!(
            "# Capability vocabulary\n\n<!-- Generated from config/capabilities.toml by the clawft-types test \
             `vocabulary_doc_is_current`. Do not edit by hand; regenerate with \
             WEFTOS_REGEN_DOCS=1 scripts/build.sh test clawft-types -->\n\n\
             {} (version {}). Advisory only: ids not listed here are accepted and matched; \
             experimental ids use the `x.` prefix. Source: ADR-099 section 2.\n\n\
             Vocabulary digest (SHA-256): `{}`\n",
            self.meta.title, self.meta.version, self.digest
        );
        for (key, fam) in fams {
            s.push_str(&format!("\n## {} (`{key}`)\n\n| Id | Summary | Attributes | Expected provenance |\n|---|---|---|---|\n", fam.title));
            for (id, e) in self
                .ids
                .iter()
                .filter(|(id, _)| id.split('.').next() == Some(key.as_str()))
            {
                let attrs = if e.attrs.is_empty() {
                    "-".to_string()
                } else {
                    e.attrs
                        .iter()
                        .map(|(a, t)| format!("`{a}`: {}", t.name()))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let prov = e
                    .min_provenance
                    .map(|p| format!("{p:?}").to_lowercase())
                    .unwrap_or_else(|| "-".into());
                s.push_str(&format!(
                    "| `{id}` | {} | {attrs} | {prov} |\n",
                    e.summary.replace('|', "\\|")
                ));
            }
        }
        s
    }
}
