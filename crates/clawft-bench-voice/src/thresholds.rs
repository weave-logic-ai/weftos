//! Pass/fail thresholds for voice benchmarks (CI gate structure).
//!
//! Defaults come from `.planning/voice_development.md` (VS2.1.8, VS3.3.5–7):
//! - latency p95 ≤ 500 ms (speech-end → first-response-byte)
//! - WER ≤ 0.15
//! - wake CPU &lt; 2%; full pipeline &lt; 10%
//!
//! Thresholds are data — they do not run benchmarks. Pair with
//! [`crate::latency`], [`crate::wer`], and [`crate::cpu`] samples, then call
//! the `check_*` helpers (or build a [`crate::report::VoiceBenchReport`]).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cpu::{CpuBudget, CpuBudgetSummary, CpuSample};
use crate::latency::{LatencySample, LatencyStats};
use crate::report::MetricStatus;
use crate::wer::WerResult;

/// Full set of voice-bench thresholds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[derive(Default)]
pub struct VoiceBenchThresholds {
    /// Latency bounds.
    pub latency: LatencyThresholds,
    /// WER bounds.
    pub wer: WerThresholds,
    /// CPU bounds (wake + pipeline).
    pub cpu: CpuThresholds,
}


impl VoiceBenchThresholds {
    /// Parse thresholds from a JSON string.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Serialize to pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Evaluate all metric groups against collected results.
    pub fn evaluate(
        &self,
        latency_stats: Option<&LatencyStats>,
        wer: Option<&WerResult>,
        wake_samples: &[CpuSample],
        pipeline_samples: &[CpuSample],
    ) -> ThresholdEvaluation {
        let latency = latency_stats.map(|s| self.latency.check_stats(s));
        let wer_status = wer.map(|w| self.wer.check(w));
        let wake = self.cpu.wake_budget().check_all(wake_samples);
        let pipeline = self.cpu.pipeline_budget().check_all(pipeline_samples);
        let passed = latency.as_ref().map(|s| s.passed).unwrap_or(true)
            && wer_status.as_ref().map(|s| s.passed).unwrap_or(true)
            && wake.passed
            && pipeline.passed;
        ThresholdEvaluation {
            passed,
            latency,
            wer: wer_status,
            wake_cpu: wake,
            pipeline_cpu: pipeline,
        }
    }
}

/// Aggregate threshold check outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThresholdEvaluation {
    /// Overall pass (missing metrics treated as pass — caller decides required set).
    pub passed: bool,
    /// Latency status if stats were provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency: Option<MetricStatus>,
    /// WER status if a result was provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wer: Option<MetricStatus>,
    /// Wake CPU summary.
    pub wake_cpu: CpuBudgetSummary,
    /// Pipeline CPU summary.
    pub pipeline_cpu: CpuBudgetSummary,
}

/// Latency threshold knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencyThresholds {
    /// Maximum allowed p95 speech-end → first-byte (milliseconds).
    pub max_p95_ms: u64,
    /// Optional hard cap on any single sample (milliseconds). `None` = no per-sample cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sample_ms: Option<u64>,
}

impl Default for LatencyThresholds {
    fn default() -> Self {
        Self {
            max_p95_ms: 500,
            max_sample_ms: Some(2_000),
        }
    }
}

impl LatencyThresholds {
    /// Check aggregate stats (p95 gate).
    pub fn check_stats(&self, stats: &LatencyStats) -> MetricStatus {
        let p95_ms = stats.p95.as_millis() as u64;
        let passed = p95_ms <= self.max_p95_ms;
        MetricStatus {
            name: "latency_p95".into(),
            passed,
            observed: format!("{p95_ms}ms"),
            limit: format!("≤{}ms", self.max_p95_ms),
            detail: Some(format!(
                "n={}, mean={}ms, p50={}ms, p99={}ms",
                stats.count,
                stats.mean.as_millis(),
                stats.p50.as_millis(),
                stats.p99.as_millis()
            )),
        }
    }

    /// Check a single sample against `max_sample_ms` (if set).
    pub fn check_sample(&self, sample: &LatencySample) -> MetricStatus {
        let ms = sample.as_millis() as u64;
        let (passed, limit) = match self.max_sample_ms {
            Some(cap) => (ms <= cap, format!("≤{cap}ms")),
            None => (true, "none".into()),
        };
        MetricStatus {
            name: "latency_sample".into(),
            passed,
            observed: format!("{ms}ms"),
            limit,
            detail: Some(sample.pipeline.clone()),
        }
    }

    /// Convenience: p95 duration bound.
    pub fn max_p95(&self) -> Duration {
        Duration::from_millis(self.max_p95_ms)
    }
}

/// WER threshold knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WerThresholds {
    /// Maximum allowed WER (0.0–1.0+). Default 0.15 (15%).
    pub max_wer: f64,
}

impl Default for WerThresholds {
    fn default() -> Self {
        Self { max_wer: 0.15 }
    }
}

impl WerThresholds {
    /// Check a WER result.
    pub fn check(&self, result: &WerResult) -> MetricStatus {
        let passed = result.wer <= self.max_wer;
        MetricStatus {
            name: "wer".into(),
            passed,
            observed: format!("{:.4}", result.wer),
            limit: format!("≤{:.4}", self.max_wer),
            detail: Some(format!(
                "S={} D={} I={} N={}",
                result.substitutions, result.deletions, result.insertions, result.reference_words
            )),
        }
    }
}

/// CPU threshold knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CpuThresholds {
    /// Wake-word max percent of one core (strict under). Default 2.0.
    pub wake_max_percent: f64,
    /// Full pipeline max percent of one core (strict under). Default 10.0.
    pub pipeline_max_percent: f64,
}

impl Default for CpuThresholds {
    fn default() -> Self {
        Self {
            wake_max_percent: 2.0,
            pipeline_max_percent: 10.0,
        }
    }
}

impl CpuThresholds {
    /// Wake budget object.
    pub fn wake_budget(&self) -> CpuBudget {
        CpuBudget::new("wake", self.wake_max_percent)
    }

    /// Pipeline budget object.
    pub fn pipeline_budget(&self) -> CpuBudget {
        CpuBudget::new("pipeline", self.pipeline_max_percent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::latency::LatencySample;
    use crate::wer::word_error_rate;
    use std::time::Duration;

    #[test]
    fn defaults_match_planning_notes() {
        let t = VoiceBenchThresholds::default();
        assert_eq!(t.latency.max_p95_ms, 500);
        assert!((t.wer.max_wer - 0.15).abs() < 1e-9);
        assert!((t.cpu.wake_max_percent - 2.0).abs() < 1e-9);
        assert!((t.cpu.pipeline_max_percent - 10.0).abs() < 1e-9);
    }

    #[test]
    fn latency_p95_gate() {
        let t = LatencyThresholds::default();
        let samples: Vec<_> = (0..20)
            .map(|i| LatencySample::from_duration("m", Duration::from_millis(100 + i * 5)))
            .collect();
        let stats = LatencyStats::from_samples(&samples).unwrap();
        // max is 100+19*5=195 → p95 under 500
        assert!(t.check_stats(&stats).passed);

        let strict = LatencyThresholds {
            max_p95_ms: 50,
            max_sample_ms: None,
        };
        assert!(!strict.check_stats(&stats).passed);
    }

    #[test]
    fn wer_gate() {
        let t = WerThresholds::default();
        assert!(t.check(&word_error_rate("a b c d e f g h", "a b c d e f g h")).passed);
        // 4/4 = 1.0 WER
        assert!(!t.check(&word_error_rate("a b c d", "w x y z")).passed);
    }

    #[test]
    fn json_roundtrip() {
        let t = VoiceBenchThresholds::default();
        let s = t.to_json_pretty().unwrap();
        let back = VoiceBenchThresholds::from_json(&s).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn evaluate_all_pass_with_mocks() {
        let t = VoiceBenchThresholds::default();
        let samples = vec![LatencySample::from_duration("mock", Duration::from_millis(10))];
        let stats = LatencyStats::from_samples(&samples).unwrap();
        let wer = word_error_rate("hello", "hello");
        let wake = [CpuSample::from_percent(0.5)];
        let pipe = [CpuSample::from_percent(3.0)];
        let ev = t.evaluate(Some(&stats), Some(&wer), &wake, &pipe);
        assert!(ev.passed);
    }
}
