//! Requirements and matching (ADR-099 section 2, rule 1).
//!
//! A requirement matches a node iff the node advertises that exact id (or an
//! id under the requested prefix, on a segment boundary), every attribute
//! predicate passes, provenance is high enough, the capability's state lets
//! it be used, and at least `count` such capabilities exist. Matching never
//! consults the vocabulary: unknown and `x.` ids match like any other.

use serde::{Deserialize, Serialize};

use super::PlacementTypeError;
use super::capability::{
    AttrValue, Capability, CapabilityId, CapabilityState, Provenance, valid_attr_name,
    validate_dotted,
};

/// Most predicates on one requirement.
pub const MAX_PREDICATES: usize = 32;
/// Largest `count` a requirement may ask for.
pub const MAX_COUNT: u32 = 64;

/// Which ids a requirement selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdSelector {
    /// Exactly this id.
    Exact(CapabilityId),
    /// This id or any id under it (`accel.npu` covers `accel.npu.hailo`).
    Prefix(String),
}

impl IdSelector {
    /// True if `id` is selected.
    pub fn selects(&self, id: &CapabilityId) -> bool {
        match self {
            IdSelector::Exact(want) => want == id,
            IdSelector::Prefix(p) => id.is_under(p),
        }
    }

    /// The id or prefix text.
    pub fn as_str(&self) -> &str {
        match self {
            IdSelector::Exact(id) => id.as_str(),
            IdSelector::Prefix(p) => p,
        }
    }
}

/// Predicate operator on one attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredicateOp {
    /// Attribute equals the value (ints and floats compare numerically).
    Eq,
    /// Numeric attribute is at least the value.
    Gte,
    /// Numeric attribute is at most the value.
    Lte,
    /// Attribute equals one element of the list value.
    In,
    /// List attribute contains the value.
    Has,
}

/// `attr op value`, for example `mem_bytes gte 8589934592`.
/// Validated on deserialize and on serialize.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "PredicateRaw")]
pub struct AttrPredicate {
    /// Attribute name.
    pub attr: String,
    /// Operator.
    pub op: PredicateOp,
    /// Operand.
    pub value: AttrValue,
}

#[derive(Serialize)]
struct PredicateOut<'a> {
    attr: &'a str,
    op: PredicateOp,
    value: &'a AttrValue,
}

impl Serialize for AttrPredicate {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        PredicateOut {
            attr: &self.attr,
            op: self.op,
            value: &self.value,
        }
        .serialize(s)
    }
}

#[derive(Deserialize)]
struct PredicateRaw {
    attr: String,
    op: PredicateOp,
    value: AttrValue,
}

impl TryFrom<PredicateRaw> for AttrPredicate {
    type Error = PlacementTypeError;
    fn try_from(r: PredicateRaw) -> Result<Self, Self::Error> {
        let p = AttrPredicate {
            attr: r.attr,
            op: r.op,
            value: r.value,
        };
        p.validate()?;
        Ok(p)
    }
}

impl AttrPredicate {
    fn mk(attr: &str, op: PredicateOp, value: AttrValue) -> Self {
        Self {
            attr: attr.to_string(),
            op,
            value,
        }
    }
    /// `attr eq value`.
    pub fn eq(attr: &str, value: impl Into<AttrValue>) -> Self {
        Self::mk(attr, PredicateOp::Eq, value.into())
    }
    /// `attr gte value`.
    pub fn gte(attr: &str, value: f64) -> Self {
        Self::mk(attr, PredicateOp::Gte, AttrValue::Float(value))
    }
    /// `attr lte value`.
    pub fn lte(attr: &str, value: f64) -> Self {
        Self::mk(attr, PredicateOp::Lte, AttrValue::Float(value))
    }
    /// `attr in [values]`.
    pub fn is_in(attr: &str, values: Vec<AttrValue>) -> Self {
        Self::mk(attr, PredicateOp::In, AttrValue::List(values))
    }
    /// `attr has value` (list attribute contains value).
    pub fn has(attr: &str, value: impl Into<AttrValue>) -> Self {
        Self::mk(attr, PredicateOp::Has, value.into())
    }

    /// Check that the operand fits the operator.
    pub fn validate(&self) -> Result<(), PlacementTypeError> {
        let bad = |why: &str| PlacementTypeError::InvalidPredicate {
            attr: self.attr.chars().take(64).collect(),
            reason: why.to_string(),
        };
        if !valid_attr_name(&self.attr) {
            return Err(bad("attribute name must match [a-z][a-z0-9_]*"));
        }
        self.value.validate(0).map_err(|why| bad(&why))?;
        match (self.op, &self.value) {
            (PredicateOp::Gte | PredicateOp::Lte, v) if v.as_f64().is_none() => {
                Err(bad("gte/lte need a numeric value"))
            }
            (PredicateOp::In, v) if !matches!(v, AttrValue::List(_)) => {
                Err(bad("in needs a list value"))
            }
            (PredicateOp::Has, AttrValue::List(_)) => Err(bad("has needs a scalar value")),
            _ => Ok(()),
        }
    }

    /// Evaluate against a capability's attributes. A missing attribute fails.
    pub fn eval(&self, cap: &Capability) -> bool {
        let Some(have) = cap.attrs.get(&self.attr) else {
            return false;
        };
        match self.op {
            PredicateOp::Eq => have.loose_eq(&self.value),
            PredicateOp::Gte => cmp_num(have, &self.value, |a, b| a >= b),
            PredicateOp::Lte => cmp_num(have, &self.value, |a, b| a <= b),
            PredicateOp::In => match &self.value {
                AttrValue::List(opts) => opts.iter().any(|o| have.loose_eq(o)),
                _ => false,
            },
            PredicateOp::Has => match have {
                AttrValue::List(items) => items.iter().any(|i| i.loose_eq(&self.value)),
                _ => false,
            },
        }
    }
}

fn cmp_num(a: &AttrValue, b: &AttrValue, f: impl Fn(f64, f64) -> bool) -> bool {
    matches!((a.as_f64(), b.as_f64()), (Some(x), Some(y)) if f(x, y))
}

/// What a workload needs from a node. Validated on deserialize and on
/// serialize, so a requirement built in code that would not read back is
/// refused when written.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "RequirementRaw")]
pub struct Requirement {
    /// Exact id or prefix.
    pub selector: IdSelector,
    /// All must pass on the same capability.
    pub where_: Vec<AttrPredicate>,
    /// How many distinct matching capabilities are needed (at least 1).
    pub count: u32,
    /// Needs the capability to itself: only `available` ones qualify.
    pub exclusive: bool,
    /// Lowest acceptable provenance, if any.
    pub min_provenance: Option<Provenance>,
}

#[derive(Serialize, Deserialize)]
struct RequirementRaw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_prefix: Option<String>,
    #[serde(rename = "where", default, skip_serializing_if = "Vec::is_empty")]
    where_: Vec<AttrPredicate>,
    #[serde(default = "one")]
    count: u32,
    #[serde(default)]
    exclusive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_provenance: Option<Provenance>,
}

fn one() -> u32 {
    1
}

impl TryFrom<RequirementRaw> for Requirement {
    type Error = PlacementTypeError;
    fn try_from(r: RequirementRaw) -> Result<Self, Self::Error> {
        let selector = match (r.id, r.id_prefix) {
            (Some(id), None) => IdSelector::Exact(CapabilityId::new(id)?),
            (None, Some(p)) => {
                validate_dotted(&p, 1)?;
                IdSelector::Prefix(p)
            }
            _ => {
                return Err(PlacementTypeError::InvalidRequirement(
                    "exactly one of `id` or `id_prefix` is required".into(),
                ));
            }
        };
        let req = Requirement {
            selector,
            where_: r.where_,
            count: r.count,
            exclusive: r.exclusive,
            min_provenance: r.min_provenance,
        };
        req.validate()?;
        Ok(req)
    }
}

impl Serialize for Requirement {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let (id, id_prefix) = match &self.selector {
            IdSelector::Exact(id) => (Some(id.to_string()), None),
            IdSelector::Prefix(p) => (None, Some(p.clone())),
        };
        RequirementRaw {
            id,
            id_prefix,
            where_: self.where_.clone(),
            count: self.count,
            exclusive: self.exclusive,
            min_provenance: self.min_provenance,
        }
        .serialize(s)
    }
}

/// Why a requirement did not match a node.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchFailure {
    /// The node advertises no id the selector covers.
    NoSuchId {
        /// The id or prefix asked for.
        selector: String,
    },
    /// Every candidate's provenance is below the minimum.
    ProvenanceTooLow {
        /// Best provenance seen.
        best: Provenance,
        /// Required.
        need: Provenance,
    },
    /// A predicate failed on every remaining candidate (first failure named).
    PredicateFailed {
        /// Attribute.
        attr: String,
        /// Operator.
        op: PredicateOp,
    },
    /// Candidates exist but none is usable in its current state.
    Unavailable {
        /// State of the first unusable candidate.
        state: CapabilityState,
    },
    /// Fewer usable capabilities than `count`.
    InsufficientCount {
        /// Usable.
        have: u32,
        /// Required.
        need: u32,
    },
    /// Enough capabilities exist for this requirement alone, but no
    /// assignment satisfies it together with the workload's exclusive
    /// requirements (see [`super::assign::match_all`]).
    Contended {
        /// Required.
        need: u32,
    },
}

impl Requirement {
    /// Require exactly `id`.
    pub fn exact(id: CapabilityId) -> Self {
        Self {
            selector: IdSelector::Exact(id),
            where_: vec![],
            count: 1,
            exclusive: false,
            min_provenance: None,
        }
    }

    /// Require `prefix` or anything under it.
    pub fn prefix(prefix: &str) -> Result<Self, PlacementTypeError> {
        validate_dotted(prefix, 1)?;
        Ok(Self {
            selector: IdSelector::Prefix(prefix.to_string()),
            where_: vec![],
            count: 1,
            exclusive: false,
            min_provenance: None,
        })
    }

    /// Builder: add a predicate, unchecked. [`Self::validate`] and
    /// serialization refuse an invalid one; [`Self::try_with_where`] fails
    /// at the call site instead.
    pub fn with_where(mut self, p: AttrPredicate) -> Self {
        self.where_.push(p);
        self
    }
    /// Builder: add a predicate, validating it and the predicate count.
    pub fn try_with_where(self, p: AttrPredicate) -> Result<Self, PlacementTypeError> {
        let r = self.with_where(p);
        r.validate()?;
        Ok(r)
    }
    /// Builder: set the count.
    pub fn with_count(mut self, count: u32) -> Self {
        self.count = count;
        self
    }
    /// Builder: require exclusive use.
    pub fn exclusive(mut self) -> Self {
        self.exclusive = true;
        self
    }
    /// Builder: set the minimum provenance.
    pub fn with_min_provenance(mut self, p: Provenance) -> Self {
        self.min_provenance = Some(p);
        self
    }

    /// Check the selector, bounds and every predicate.
    pub fn validate(&self) -> Result<(), PlacementTypeError> {
        if let IdSelector::Prefix(p) = &self.selector {
            validate_dotted(p, 1)?;
        }
        if self.count == 0 || self.count > MAX_COUNT {
            return Err(PlacementTypeError::InvalidRequirement(format!(
                "count must be 1..={MAX_COUNT}"
            )));
        }
        if self.where_.len() > MAX_PREDICATES {
            return Err(PlacementTypeError::InvalidRequirement(
                "too many predicates".into(),
            ));
        }
        self.where_.iter().try_for_each(AttrPredicate::validate)
    }

    fn usable(&self, cap: &Capability) -> bool {
        match cap.state {
            CapabilityState::Available => true,
            CapabilityState::Busy => !self.exclusive && !cap.exclusive,
            CapabilityState::Reserved | CapabilityState::Degraded => false,
        }
    }

    /// Match against a node's capabilities. On success, returns the indices
    /// of the `count` capabilities chosen.
    pub fn match_caps(&self, caps: &[Capability]) -> Result<Vec<usize>, MatchFailure> {
        let usable = self.candidates(caps)?;
        let need = self.count as usize;
        if usable.len() < need {
            return Err(MatchFailure::InsufficientCount {
                have: usable.len() as u32,
                need: self.count,
            });
        }
        Ok(usable.into_iter().take(need).collect())
    }

    /// Every capability this requirement could use, ignoring `count`.
    /// Fails with the first stage (id, provenance, predicate, state) that
    /// leaves nothing.
    pub fn candidates(&self, caps: &[Capability]) -> Result<Vec<usize>, MatchFailure> {
        let by_id: Vec<usize> = (0..caps.len())
            .filter(|&i| self.selector.selects(&caps[i].id))
            .collect();
        if by_id.is_empty() {
            return Err(MatchFailure::NoSuchId {
                selector: self.selector.as_str().to_string(),
            });
        }
        let by_prov: Vec<usize> = match self.min_provenance {
            Some(need) => {
                let v: Vec<usize> = by_id
                    .iter()
                    .copied()
                    .filter(|&i| caps[i].provenance >= need)
                    .collect();
                if v.is_empty() {
                    let best = by_id
                        .iter()
                        .map(|&i| caps[i].provenance)
                        .max()
                        .unwrap_or(Provenance::Claimed);
                    return Err(MatchFailure::ProvenanceTooLow { best, need });
                }
                v
            }
            None => by_id,
        };
        let by_attr: Vec<usize> = by_prov
            .iter()
            .copied()
            .filter(|&i| self.where_.iter().all(|p| p.eval(&caps[i])))
            .collect();
        if by_attr.is_empty() {
            let first = &caps[by_prov[0]];
            let p = self
                .where_
                .iter()
                .find(|p| !p.eval(first))
                .expect("some predicate failed");
            return Err(MatchFailure::PredicateFailed {
                attr: p.attr.clone(),
                op: p.op,
            });
        }
        let usable: Vec<usize> = by_attr
            .iter()
            .copied()
            .filter(|&i| self.usable(&caps[i]))
            .collect();
        if usable.is_empty() {
            return Err(MatchFailure::Unavailable {
                state: caps[by_attr[0]].state,
            });
        }
        Ok(usable)
    }

    /// True if [`Self::match_caps`] succeeds.
    pub fn matches(&self, caps: &[Capability]) -> bool {
        self.match_caps(caps).is_ok()
    }
}
