//! Measured-performance capabilities (ADR-099 section 2, "Measured
//! throughput, not only architecture").
//!
//! Parameterised ids such as `perf.cog.cycle_ms{cog_id}` are encoded as one
//! capability per parameter value: the id is `perf.cog.cycle_ms`, the
//! parameter is an attribute (`cog_id`), and the number is the `value`
//! attribute. A node can therefore advertise one entry per cog or model, and
//! a requirement selects the right one with an `eq` predicate on the
//! parameter plus a `lte`/`gte` predicate on `value`.
//!
//! These records are always `measured`: only the conformance harness and
//! admission probes produce them. This module has no measuring code.

use super::PlacementTypeError;
use super::capability::{AttrValue, Capability, CapabilityId, Provenance};
use super::requirement::{AttrPredicate, Requirement};

/// Id for one cog cycle wall time in milliseconds (param `cog_id`).
pub const PERF_COG_CYCLE_MS: &str = "perf.cog.cycle_ms";
/// Id for inference decode throughput in tokens per second (param `model`).
pub const PERF_INFER_TOK_S: &str = "perf.infer.tok_s";
/// Attribute holding the measured number.
pub const VALUE_ATTR: &str = "value";

fn measured(
    id: &str,
    param: &str,
    key: &str,
    value: AttrValue,
) -> Result<Capability, PlacementTypeError> {
    if value.as_f64().is_none_or(|v| !v.is_finite() || v < 0.0) {
        return Err(PlacementTypeError::InvalidAttr {
            id: id.to_string(),
            attr: VALUE_ATTR.to_string(),
            reason: "measured value must be finite and non-negative".to_string(),
        });
    }
    if key.is_empty() || key.len() > super::capability::MAX_STR_LEN {
        return Err(PlacementTypeError::InvalidAttr {
            id: id.to_string(),
            attr: param.to_string(),
            reason: "parameter must be non-empty and bounded".to_string(),
        });
    }
    let cap = Capability::new(CapabilityId::new(id)?, Provenance::Measured)
        .with_attr(param, key)
        .with_attr(VALUE_ATTR, value);
    cap.validate()?;
    Ok(cap)
}

/// `perf.cog.cycle_ms{cog_id}`: measured wall time of one cycle.
pub fn cog_cycle_ms(cog_id: &str, ms: f64) -> Result<Capability, PlacementTypeError> {
    measured(PERF_COG_CYCLE_MS, "cog_id", cog_id, AttrValue::Float(ms))
}

/// `perf.infer.tok_s{model}`: measured decode tokens per second.
pub fn infer_tok_s(model: &str, tok_s: f64) -> Result<Capability, PlacementTypeError> {
    measured(PERF_INFER_TOK_S, "model", model, AttrValue::Float(tok_s))
}

/// Requirement: this cog's measured cycle time is at most `budget_ms`.
/// Requires `measured` provenance, so an unmeasured node does not pass.
pub fn require_cog_cycle_within(
    cog_id: &str,
    budget_ms: f64,
) -> Result<Requirement, PlacementTypeError> {
    Ok(Requirement::exact(CapabilityId::new(PERF_COG_CYCLE_MS)?)
        .try_with_where(AttrPredicate::eq("cog_id", cog_id))?
        .try_with_where(AttrPredicate::lte(VALUE_ATTR, budget_ms))?
        .with_min_provenance(Provenance::Measured))
}

/// Requirement: this model's measured decode rate is at least `min_tok_s`.
pub fn require_infer_tok_s_at_least(
    model: &str,
    min_tok_s: f64,
) -> Result<Requirement, PlacementTypeError> {
    Ok(Requirement::exact(CapabilityId::new(PERF_INFER_TOK_S)?)
        .try_with_where(AttrPredicate::eq("model", model))?
        .try_with_where(AttrPredicate::gte(VALUE_ATTR, min_tok_s))?
        .with_min_provenance(Provenance::Measured))
}
