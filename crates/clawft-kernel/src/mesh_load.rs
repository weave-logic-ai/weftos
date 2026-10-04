//! A small load sample a node attaches to its liveness pongs (fleet P3).
//!
//! The responder reads its own load average, core count and available
//! memory when it answers a ping and puts them in the pong as `load`. The
//! pinger keeps the latest sample per peer. It is what the peer says about
//! itself over its verified connection: not signed, so the fleet view labels
//! it `peer_claimed`. A peer that predates this sends no `load`; a value out
//! of range is dropped, never clamped into a plausible number.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Load averages above this are refused as nonsense.
const MAX_LOAD: f64 = 100_000.0;
/// Core counts above this are refused.
const MAX_CORES: u64 = 65_536;

/// One node's load at the moment it answered.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoadSample {
    /// 1-minute load average.
    pub load1: f64,
    /// 5-minute load average.
    pub load5: f64,
    /// Logical CPUs, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cores: Option<u32>,
    /// Available memory, bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_avail: Option<u64>,
    /// Total memory, bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_total: Option<u64>,
}

impl LoadSample {
    /// This host's load now; `None` where the OS gives no load average.
    pub fn local() -> Option<Self> {
        let (load1, load5) = load_avg()?;
        let (mem_avail, mem_total) = memory();
        Some(Self {
            load1,
            load5,
            cores: std::thread::available_parallelism().ok().and_then(|n| u32::try_from(n.get()).ok()),
            mem_avail,
            mem_total,
        })
    }

    /// Parse a peer's `load` object, refusing out-of-range values.
    pub fn from_json(v: &Value) -> Option<Self> {
        let load = |k: &str| v.get(k)?.as_f64().filter(|x| x.is_finite() && (0.0..=MAX_LOAD).contains(x));
        let s = Self {
            load1: load("load1")?,
            load5: load("load5")?,
            cores: match v.get("cores") {
                None | Some(Value::Null) => None,
                Some(c) => Some(c.as_u64().filter(|c| (1..=MAX_CORES).contains(c))? as u32),
            },
            mem_avail: v.get("mem_avail").and_then(Value::as_u64),
            mem_total: v.get("mem_total").and_then(Value::as_u64),
        };
        // Available memory cannot exceed the total.
        match (s.mem_avail, s.mem_total) {
            (Some(a), Some(t)) if a > t => None,
            _ => Some(s),
        }
    }

    /// The `load` object sent in a pong.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[cfg(unix)]
fn load_avg() -> Option<(f64, f64)> {
    let mut l = [0f64; 3];
    // SAFETY: `l` holds 3 doubles and we ask for at most 3.
    let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    (n >= 2).then_some((l[0], l[1]))
}
#[cfg(not(unix))]
fn load_avg() -> Option<(f64, f64)> {
    None
}

#[cfg(target_os = "linux")]
fn memory() -> (Option<u64>, Option<u64>) {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|r| r.split_whitespace().next()?.parse::<u64>().ok())
            .map(|k| k.saturating_mul(1024))
    };
    (kb("MemAvailable"), kb("MemTotal"))
}
#[cfg(not(target_os = "linux"))]
fn memory() -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_sample_round_trips_through_its_json() {
        let s = LoadSample { load1: 0.5, load5: 1.25, cores: Some(4), mem_avail: Some(1), mem_total: Some(2) };
        assert_eq!(LoadSample::from_json(&s.to_json()), Some(s));
        let bare = LoadSample { load1: 0.0, load5: 0.0, cores: None, mem_avail: None, mem_total: None };
        assert_eq!(bare.to_json(), json!({ "load1": 0.0, "load5": 0.0 }));
        assert_eq!(LoadSample::from_json(&bare.to_json()), Some(bare));
    }

    #[test]
    fn out_of_range_values_are_refused_not_clamped() {
        for bad in [
            json!({ "load1": -1.0, "load5": 0.1 }),
            json!({ "load1": 1e9, "load5": 0.1 }),
            json!({ "load1": "1", "load5": 0.1 }),
            json!({ "load5": 0.1 }),
            json!({ "load1": 0.1, "load5": 0.1, "cores": 0 }),
            json!({ "load1": 0.1, "load5": 0.1, "mem_avail": 9, "mem_total": 1 }),
            json!(null),
        ] {
            assert_eq!(LoadSample::from_json(&bad), None, "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn this_host_reports_a_load() {
        let s = LoadSample::local().expect("unix has getloadavg");
        assert!(s.load1 >= 0.0 && s.cores.unwrap_or(1) >= 1);
    }
}
