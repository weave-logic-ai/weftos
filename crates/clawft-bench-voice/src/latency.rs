//! Latency harness: speech-end → first-response-byte.
//!
//! The primary metric for conversational voice (ADR-061 / VS3.3.5) is the
//! wall-clock gap between the moment user speech ends and the first byte of
//! agent audio (or text, when measuring STT alone) becomes available.
//!
//! Real STT/TTS models are **not** required. Implement [`VoicePipeline`] for a
//! mock, a fixture player, or a live stack; the harness only records timestamps.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors raised while driving a pipeline sample.
#[derive(Debug, Error)]
pub enum LatencyError {
    /// The pipeline itself failed (model missing, I/O, etc.).
    #[error("voice pipeline error: {0}")]
    Pipeline(String),
    /// No successful samples were collected.
    #[error("no successful latency samples")]
    Empty,
}

/// Minimal seam a voice stack must expose for latency measurement.
///
/// Callers mark speech-end, then wait for the first response byte. The
/// harness owns timing so implementations stay free of Instant bookkeeping.
pub trait VoicePipeline {
    /// Human-readable label (e.g. `"mock"`, `"parakeet+kokoro"`).
    fn name(&self) -> &str;

    /// Run one turn: simulate or perform speech-end → first response byte.
    ///
    /// Implementations should block until the first response byte is ready
    /// (or fail). The harness stamps `speech_end` immediately before this
    /// call and `first_byte` immediately after it returns `Ok`.
    ///
    /// For live stacks, the implementor may also use
    /// [`LatencySample::from_instants`] with its own high-resolution marks.
    fn run_turn(&mut self) -> Result<(), String>;
}

/// One measured speech-end → first-byte observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencySample {
    /// Pipeline label that produced the sample.
    pub pipeline: String,
    /// Speech-end → first-response-byte duration.
    #[serde(with = "duration_ms")]
    pub speech_end_to_first_byte: Duration,
    /// Optional free-form notes (fixture id, model path, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl LatencySample {
    /// Build a sample from two wall-clock instants.
    pub fn from_instants(pipeline: impl Into<String>, speech_end: Instant, first_byte: Instant) -> Self {
        let elapsed = first_byte.saturating_duration_since(speech_end);
        Self {
            pipeline: pipeline.into(),
            speech_end_to_first_byte: elapsed,
            note: None,
        }
    }

    /// Build a sample from an already-known duration (fixture / replay).
    pub fn from_duration(pipeline: impl Into<String>, duration: Duration) -> Self {
        Self {
            pipeline: pipeline.into(),
            speech_end_to_first_byte: duration,
            note: None,
        }
    }

    /// Attach a note and return self.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Duration in whole milliseconds (rounded down).
    pub fn as_millis(&self) -> u128 {
        self.speech_end_to_first_byte.as_millis()
    }
}

/// Aggregate stats over many [`LatencySample`]s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencyStats {
    /// Number of samples.
    pub count: usize,
    /// Minimum observed duration.
    #[serde(with = "duration_ms")]
    pub min: Duration,
    /// Maximum observed duration.
    #[serde(with = "duration_ms")]
    pub max: Duration,
    /// Arithmetic mean.
    #[serde(with = "duration_ms")]
    pub mean: Duration,
    /// Median (p50).
    #[serde(with = "duration_ms")]
    pub p50: Duration,
    /// 95th percentile.
    #[serde(with = "duration_ms")]
    pub p95: Duration,
    /// 99th percentile.
    #[serde(with = "duration_ms")]
    pub p99: Duration,
}

impl LatencyStats {
    /// Compute stats from a non-empty slice of samples.
    pub fn from_samples(samples: &[LatencySample]) -> Result<Self, LatencyError> {
        if samples.is_empty() {
            return Err(LatencyError::Empty);
        }
        let mut nanos: Vec<u128> = samples
            .iter()
            .map(|s| s.speech_end_to_first_byte.as_nanos())
            .collect();
        nanos.sort_unstable();
        let count = nanos.len();
        let min = Duration::from_nanos(nanos[0] as u64);
        let max = Duration::from_nanos(nanos[count - 1] as u64);
        let sum: u128 = nanos.iter().sum();
        let mean = Duration::from_nanos((sum / count as u128) as u64);
        Ok(Self {
            count,
            min,
            max,
            mean,
            p50: percentile(&nanos, 50),
            p95: percentile(&nanos, 95),
            p99: percentile(&nanos, 99),
        })
    }
}

/// Nearest-rank percentile over a **sorted** nanos slice.
fn percentile(sorted_nanos: &[u128], pct: u8) -> Duration {
    debug_assert!(!sorted_nanos.is_empty());
    debug_assert!(pct <= 100);
    let n = sorted_nanos.len();
    // Nearest-rank: ceil(pct/100 * n), 1-indexed → clamp to [0, n-1].
    let rank = ((pct as usize).saturating_mul(n).div_ceil(100)).max(1);
    let idx = rank.saturating_sub(1).min(n - 1);
    Duration::from_nanos(sorted_nanos[idx] as u64)
}

/// Runs a [`VoicePipeline`] and collects latency samples.
pub struct LatencyHarness<P: VoicePipeline> {
    pipeline: P,
    samples: Vec<LatencySample>,
}

impl<P: VoicePipeline> LatencyHarness<P> {
    /// Wrap a pipeline.
    pub fn new(pipeline: P) -> Self {
        Self {
            pipeline,
            samples: Vec::new(),
        }
    }

    /// Borrow the underlying pipeline.
    pub fn pipeline(&self) -> &P {
        &self.pipeline
    }

    /// Mutable access to the pipeline (e.g. reconfigure mock delay).
    pub fn pipeline_mut(&mut self) -> &mut P {
        &mut self.pipeline
    }

    /// Collected samples so far.
    pub fn samples(&self) -> &[LatencySample] {
        &self.samples
    }

    /// Run one measured turn and store the sample.
    pub fn run_once(&mut self) -> Result<LatencySample, LatencyError> {
        let speech_end = Instant::now();
        self.pipeline
            .run_turn()
            .map_err(LatencyError::Pipeline)?;
        let first_byte = Instant::now();
        let sample = LatencySample::from_instants(self.pipeline.name(), speech_end, first_byte);
        self.samples.push(sample.clone());
        Ok(sample)
    }

    /// Run `n` turns.
    pub fn run_n(&mut self, n: usize) -> Result<Vec<LatencySample>, LatencyError> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.run_once()?);
        }
        Ok(out)
    }

    /// Aggregate stats over collected samples.
    pub fn stats(&self) -> Result<LatencyStats, LatencyError> {
        LatencyStats::from_samples(&self.samples)
    }

    /// Inject a pre-measured sample (fixture replay / external timer).
    pub fn push_sample(&mut self, sample: LatencySample) {
        self.samples.push(sample);
    }
}

/// Deterministic mock pipeline for CI — no models, no audio devices.
///
/// Simulates speech-end → first-byte by sleeping (or returning immediately
/// when `delay` is zero). Optionally fails after N successful turns to
/// exercise error paths.
#[derive(Debug, Clone)]
pub struct MockPipeline {
    /// Label returned by [`VoicePipeline::name`].
    pub name: String,
    /// Artificial processing delay.
    pub delay: Duration,
    /// How many successful turns remain before the next call fails (`None` = never).
    pub fail_after: Option<usize>,
    turns: usize,
}

impl MockPipeline {
    /// Instant-return mock (sub-millisecond on typical hosts).
    pub fn instant() -> Self {
        Self::with_delay(Duration::ZERO)
    }

    /// Mock that sleeps `delay` before "first byte".
    pub fn with_delay(delay: Duration) -> Self {
        Self {
            name: "mock".into(),
            delay,
            fail_after: None,
            turns: 0,
        }
    }

    /// Rename the pipeline label.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Fail on the (n+1)-th `run_turn` call (0-based: `fail_after(0)` fails first call).
    pub fn fail_after(mut self, n: usize) -> Self {
        self.fail_after = Some(n);
        self
    }
}

impl VoicePipeline for MockPipeline {
    fn name(&self) -> &str {
        &self.name
    }

    fn run_turn(&mut self) -> Result<(), String> {
        if let Some(limit) = self.fail_after
            && self.turns >= limit {
                return Err(format!(
                    "mock pipeline forced failure after {limit} turn(s)"
                ));
            }
        self.turns += 1;
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        Ok(())
    }
}

/// Serde helper: store durations as whole milliseconds in JSON.
mod duration_ms {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(d: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(d.as_millis() as u64)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ms = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_instant_records_sub_second_sample() {
        let mut h = LatencyHarness::new(MockPipeline::instant());
        let s = h.run_once().expect("ok");
        assert!(s.as_millis() < 500, "instant mock should be fast, got {}ms", s.as_millis());
        assert_eq!(s.pipeline, "mock");
    }

    #[test]
    fn mock_delay_is_reflected_in_sample() {
        let mut h = LatencyHarness::new(MockPipeline::with_delay(Duration::from_millis(20)));
        let s = h.run_once().expect("ok");
        // Allow scheduler jitter; require at least ~10ms observed.
        assert!(
            s.speech_end_to_first_byte >= Duration::from_millis(10),
            "expected ≥10ms, got {:?}",
            s.speech_end_to_first_byte
        );
    }

    #[test]
    fn stats_percentiles_sorted() {
        let samples: Vec<_> = [10u64, 20, 30, 40, 50, 60, 70, 80, 90, 100]
            .into_iter()
            .map(|ms| LatencySample::from_duration("fix", Duration::from_millis(ms)))
            .collect();
        let stats = LatencyStats::from_samples(&samples).unwrap();
        assert_eq!(stats.count, 10);
        assert_eq!(stats.min, Duration::from_millis(10));
        assert_eq!(stats.max, Duration::from_millis(100));
        assert_eq!(stats.p50, Duration::from_millis(50));
        // nearest-rank p95 of 10 → rank 10 → 100ms
        assert_eq!(stats.p95, Duration::from_millis(100));
    }

    #[test]
    fn empty_stats_err() {
        assert!(matches!(
            LatencyStats::from_samples(&[]),
            Err(LatencyError::Empty)
        ));
    }

    #[test]
    fn mock_fail_after() {
        let mut h = LatencyHarness::new(MockPipeline::instant().fail_after(1));
        assert!(h.run_once().is_ok());
        assert!(matches!(h.run_once(), Err(LatencyError::Pipeline(_))));
    }

    #[test]
    fn sample_json_roundtrip_ms() {
        let s = LatencySample::from_duration("mock", Duration::from_millis(42)).with_note("fixture");
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("42"));
        let back: LatencySample = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_millis(), 42);
        assert_eq!(back.note.as_deref(), Some("fixture"));
    }
}
