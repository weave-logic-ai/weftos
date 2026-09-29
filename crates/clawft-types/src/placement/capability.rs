//! Capability records: what a node advertises (ADR-099 section 2).
//!
//! A capability is a dotted, lowercase id plus typed attributes, an honest
//! provenance, a live state, and an `exclusive` flag. Ids are **open**: any
//! syntactically valid id is accepted, stored and matched. The well-known
//! vocabulary (`config/capabilities.toml`, see [`super::vocabulary`]) only
//! produces warnings; it never gates.
//!
//! Every type here validates at the serde boundary, so a record read from a
//! peer, a config file or an RPC is either well formed or rejected.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::PlacementTypeError;

/// Longest accepted capability id, in bytes.
pub const MAX_ID_LEN: usize = 128;
/// Most dot-separated segments in an id.
pub const MAX_ID_SEGMENTS: usize = 8;
/// Longest attribute name, in bytes.
pub const MAX_ATTR_NAME_LEN: usize = 64;
/// Most attributes on one capability.
pub const MAX_ATTRS: usize = 64;
/// Most elements in one list attribute.
pub const MAX_LIST_LEN: usize = 256;
/// Longest string attribute value, in bytes.
pub const MAX_STR_LEN: usize = 1024;

/// Prefix reserved for experimental ids (`x.acme.widget`).
pub const EXPERIMENTAL_PREFIX: &str = "x";

/// Check one id segment: `[a-z0-9][a-z0-9_-]*`.
pub(crate) fn valid_segment(seg: &str) -> bool {
    let mut chars = seg.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Validate a dotted id with at least `min_segments` segments.
pub(crate) fn validate_dotted(s: &str, min_segments: usize) -> Result<(), PlacementTypeError> {
    let bad = |why: &str| PlacementTypeError::InvalidId {
        id: truncate(s),
        reason: why.to_string(),
    };
    if s.is_empty() {
        return Err(bad("empty"));
    }
    if s.len() > MAX_ID_LEN {
        return Err(bad("too long"));
    }
    let segs: Vec<&str> = s.split('.').collect();
    if segs.len() < min_segments {
        return Err(bad("too few dot-separated segments"));
    }
    if segs.len() > MAX_ID_SEGMENTS {
        return Err(bad("too many dot-separated segments"));
    }
    if let Some(seg) = segs.iter().find(|seg| !valid_segment(seg)) {
        return Err(bad(&format!(
            "segment {seg:?} must match [a-z0-9][a-z0-9_-]*"
        )));
    }
    Ok(())
}

fn truncate(s: &str) -> String {
    s.chars().take(MAX_ID_LEN).collect()
}

/// A validated, dotted, lowercase capability id (`accel.gpu.metal`).
///
/// At least two segments, so every id lives under a family namespace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Parse and validate an id.
    pub fn new(s: impl Into<String>) -> Result<Self, PlacementTypeError> {
        let s = s.into();
        validate_dotted(&s, 2)?;
        Ok(Self(s))
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// First segment (`accel` for `accel.gpu.metal`).
    pub fn family(&self) -> &str {
        self.0.split('.').next().unwrap_or_default()
    }

    /// True for `x.`-prefixed experimental ids.
    pub fn is_experimental(&self) -> bool {
        self.family() == EXPERIMENTAL_PREFIX
    }

    /// True if this id equals `prefix` or lies under it on a segment
    /// boundary (`accel.npu` covers `accel.npu.hailo`, not `accel.npux`).
    pub fn is_under(&self, prefix: &str) -> bool {
        self.0 == prefix
            || (self.0.len() > prefix.len()
                && self.0.starts_with(prefix)
                && self.0.as_bytes()[prefix.len()] == b'.')
    }
}

impl TryFrom<String> for CapabilityId {
    type Error = PlacementTypeError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<CapabilityId> for String {
    fn from(id: CapabilityId) -> Self {
        id.0
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An attribute value. Untagged on the wire: `true`, `42`, `1.5`, `"x"`, `[..]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttrValue {
    /// Boolean.
    Bool(bool),
    /// Signed integer (byte counts fit: 128 GiB is about 1.4e11).
    Int(i64),
    /// Finite float.
    Float(f64),
    /// String.
    Str(String),
    /// List of values (`formats`, `arches_native`, `precisions`).
    List(Vec<AttrValue>),
}

impl AttrValue {
    /// Numeric view for `gte`/`lte` comparisons.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            AttrValue::Int(i) => Some(*i as f64),
            AttrValue::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Equality that treats `Int(2)` and `Float(2.0)` as equal.
    pub fn loose_eq(&self, other: &AttrValue) -> bool {
        match (self.as_f64(), other.as_f64()) {
            (Some(a), Some(b)) => a == b,
            _ => match (self, other) {
                (AttrValue::List(a), AttrValue::List(b)) => {
                    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.loose_eq(y))
                }
                _ => self == other,
            },
        }
    }

    /// Validate bounds (finite floats, sizes); `depth` guards nesting.
    pub(crate) fn validate(&self, depth: usize) -> Result<(), String> {
        match self {
            AttrValue::Float(f) if !f.is_finite() => Err("non-finite float".into()),
            AttrValue::Str(s) if s.len() > MAX_STR_LEN => Err("string too long".into()),
            AttrValue::List(_) if depth > 0 => Err("nested lists are not allowed".into()),
            AttrValue::List(v) if v.len() > MAX_LIST_LEN => Err("list too long".into()),
            AttrValue::List(v) => v.iter().try_for_each(|x| x.validate(depth + 1)),
            _ => Ok(()),
        }
    }
}

impl From<bool> for AttrValue {
    fn from(v: bool) -> Self {
        AttrValue::Bool(v)
    }
}
impl From<i64> for AttrValue {
    fn from(v: i64) -> Self {
        AttrValue::Int(v)
    }
}
impl From<f64> for AttrValue {
    fn from(v: f64) -> Self {
        AttrValue::Float(v)
    }
}
impl From<&str> for AttrValue {
    fn from(v: &str) -> Self {
        AttrValue::Str(v.to_string())
    }
}

/// Validate an attribute name: `[a-z][a-z0-9_]*`, bounded length.
pub(crate) fn valid_attr_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_ATTR_NAME_LEN
        && matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// How much a capability can be trusted. Ordered: `Claimed < Probed < Measured`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Asserted by an operator or adapter; not verified locally.
    Claimed,
    /// A local probe saw it.
    Probed,
    /// A conformance or benchmark run produced it.
    Measured,
}

/// Live state of a capability on its node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    /// Free to use.
    #[default]
    Available,
    /// In use; still shareable unless the capability is `exclusive`.
    Busy,
    /// Held for a pending placement; not offered to others.
    Reserved,
    /// Present but impaired (for example a detached drive); not offered.
    Degraded,
}

/// One advertised capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CapabilityRaw")]
pub struct Capability {
    /// Dotted id, open vocabulary.
    pub id: CapabilityId,
    /// Typed attributes (sorted for deterministic signing and output).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, AttrValue>,
    /// How the node knows this.
    pub provenance: Provenance,
    /// Live state.
    #[serde(default)]
    pub state: CapabilityState,
    /// Only one workload may hold it at a time.
    #[serde(default)]
    pub exclusive: bool,
}

#[derive(Deserialize)]
struct CapabilityRaw {
    id: CapabilityId,
    #[serde(default)]
    attrs: BTreeMap<String, AttrValue>,
    provenance: Provenance,
    #[serde(default)]
    state: CapabilityState,
    #[serde(default)]
    exclusive: bool,
}

impl TryFrom<CapabilityRaw> for Capability {
    type Error = PlacementTypeError;
    fn try_from(r: CapabilityRaw) -> Result<Self, Self::Error> {
        let cap = Capability {
            id: r.id,
            attrs: r.attrs,
            provenance: r.provenance,
            state: r.state,
            exclusive: r.exclusive,
        };
        cap.validate()?;
        Ok(cap)
    }
}

impl Capability {
    /// A new available, non-exclusive capability with no attributes.
    pub fn new(id: CapabilityId, provenance: Provenance) -> Self {
        Self {
            id,
            attrs: BTreeMap::new(),
            provenance,
            state: CapabilityState::Available,
            exclusive: false,
        }
    }

    /// Builder: set one attribute.
    pub fn with_attr(mut self, name: &str, value: impl Into<AttrValue>) -> Self {
        self.attrs.insert(name.to_string(), value.into());
        self
    }

    /// Builder: set the state.
    pub fn with_state(mut self, state: CapabilityState) -> Self {
        self.state = state;
        self
    }

    /// Builder: mark exclusive.
    pub fn exclusive(mut self) -> Self {
        self.exclusive = true;
        self
    }

    /// Check attribute names and values against the bounds above.
    pub fn validate(&self) -> Result<(), PlacementTypeError> {
        if self.attrs.len() > MAX_ATTRS {
            return Err(self.attr_err("*", "too many attributes"));
        }
        for (name, value) in &self.attrs {
            if !valid_attr_name(name) {
                return Err(self.attr_err(name, "name must match [a-z][a-z0-9_]*"));
            }
            value.validate(0).map_err(|why| self.attr_err(name, &why))?;
        }
        Ok(())
    }

    fn attr_err(&self, attr: &str, reason: &str) -> PlacementTypeError {
        PlacementTypeError::InvalidAttr {
            id: self.id.to_string(),
            attr: attr.chars().take(MAX_ATTR_NAME_LEN).collect(),
            reason: reason.to_string(),
        }
    }
}
