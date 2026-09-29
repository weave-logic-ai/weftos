//! Load measured `perf.*` capabilities written by the cog conformance
//! harness (`scripts/build.sh cogs-conformance sweep|probe`) or an
//! admission probe.
//!
//! Accepts the harness `capabilities.json` (a list) or a probe document
//! (`{"capabilities": [...]}`). Only `perf.*` entries with provenance
//! `measured` are kept; attributes the harness writes as `null` are dropped
//! before validation. Everything else is skipped and counted, never
//! silently upgraded.

use clawft_types::placement::{Capability, Provenance};
use serde_json::Value;

/// Largest measured-results file read, in bytes.
pub const MAX_MEASURED_BYTES: usize = 4 << 20;

/// Parsed results: kept capabilities and how many entries were skipped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Measured {
    /// Valid measured `perf.*` capabilities.
    pub caps: Vec<Capability>,
    /// Entries skipped (wrong family, not measured, or invalid).
    pub skipped: usize,
}

/// Parse a harness results document.
pub fn parse_measured(text: &str) -> Result<Measured, String> {
    if text.len() > MAX_MEASURED_BYTES {
        return Err("measured results file too large".into());
    }
    let doc: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let list = match &doc {
        Value::Array(v) => v.clone(),
        Value::Object(o) => match o.get("capabilities") {
            Some(Value::Array(v)) => v.clone(),
            _ => return Err("expected a list or {\"capabilities\": [...]}".into()),
        },
        _ => return Err("expected a list or {\"capabilities\": [...]}".into()),
    };
    let mut out = Measured::default();
    for mut entry in list {
        if let Some(Value::Object(attrs)) = entry.get_mut("attrs") {
            attrs.retain(|_, v| !v.is_null());
        }
        match serde_json::from_value::<Capability>(entry) {
            Ok(c) if c.id.family() == "perf" && c.provenance == Provenance::Measured => {
                out.caps.push(c);
            }
            _ => out.skipped += 1,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_measured_perf_and_drops_nulls() {
        let text = r#"[
          {"id":"perf.cog.cycle_ms","attrs":{"cog_id":"fall-detect","value":790.5,"interval_s":null},
           "provenance":"measured","state":"available","exclusive":false},
          {"id":"perf.cog.cycle_ms","attrs":{"cog_id":"x","value":1.0},"provenance":"claimed"},
          {"id":"cpu.arch.aarch64","provenance":"measured"},
          {"id":"BAD ID","provenance":"measured"}
        ]"#;
        let m = parse_measured(text).unwrap();
        assert_eq!(m.caps.len(), 1);
        assert_eq!(m.skipped, 3);
        assert!(!m.caps[0].attrs.contains_key("interval_s"));
    }

    #[test]
    fn accepts_probe_document_and_rejects_other_shapes() {
        let doc = r#"{"cog":"a","capabilities":[{"id":"perf.infer.tok_s","attrs":{"model":"m","value":42.0},"provenance":"measured"}]}"#;
        assert_eq!(parse_measured(doc).unwrap().caps.len(), 1);
        assert!(parse_measured(r#"{"x":1}"#).is_err());
        assert!(parse_measured("nope").is_err());
    }
}
