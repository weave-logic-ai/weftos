//! Shared state between the network worker and the UI: the merged sample buffer, the latest
//! cog report, and the Seed agent's view of the cog.

use std::collections::{BTreeSet, VecDeque};
use web_time::Instant;

/// Seconds of signal kept locally (matches the cog's own 60 s ring).
pub const KEEP_SECONDS: f64 = 60.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub t_ms: u64,
    pub raw_v: f64,
    pub filtered_mv: f64,
}

/// Samples merged from overlapping `/raw?seconds=N` polls, de-duplicated by timestamp.
#[derive(Default)]
pub struct SignalBuffer {
    pub samples: VecDeque<Sample>,
    pub peaks_ms: BTreeSet<u64>,
    pub sample_rate_hz: f64,
    pub source: String,
}

impl SignalBuffer {
    /// Appends samples newer than the last one held; drops anything older than KEEP_SECONDS.
    pub fn merge(&mut self, incoming: &[Sample], peaks_ms: &[u64]) -> usize {
        // Time going backwards by more than 5 s means the cog restarted: start over.
        let last = self.samples.back().map(|s| s.t_ms);
        if incoming
            .first()
            .zip(last)
            .is_some_and(|(f, l)| f.t_ms + 5_000 < l)
        {
            self.clear();
        }
        let last = self.samples.back().map(|s| s.t_ms);
        let mut added = 0;
        for s in incoming.iter().filter(|s| last.is_none_or(|l| s.t_ms > l)) {
            self.samples.push_back(*s);
            added += 1;
        }
        self.peaks_ms.extend(peaks_ms.iter().copied());
        if let Some(newest) = self.samples.back().map(|s| s.t_ms) {
            let cutoff = newest.saturating_sub((KEEP_SECONDS * 1000.0) as u64);
            while self.samples.front().is_some_and(|s| s.t_ms < cutoff) {
                self.samples.pop_front();
            }
            self.peaks_ms = self.peaks_ms.split_off(&cutoff);
        }
        added
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.peaks_ms.clear();
    }

    /// The last `seconds` of samples.
    pub fn tail(&self, seconds: f64) -> Vec<Sample> {
        let Some(newest) = self.samples.back().map(|s| s.t_ms) else {
            return Vec::new();
        };
        let cutoff = newest.saturating_sub((seconds * 1000.0) as u64);
        self.samples
            .iter()
            .filter(|s| s.t_ms >= cutoff)
            .copied()
            .collect()
    }

    /// Beat-to-beat heart rate as a RuView ADR-293 reference series: `timestamp_ms,value`
    /// (unix ms of each R-peak, 60000 / RR in bpm), strictly increasing, RR limited to
    /// 300-2000 ms. Feed it to `wifi-densepose-vitals` ground-truth ingest (HeartRateBpm).
    pub fn ruview_reference_csv(&self) -> String {
        let mut out = String::from("timestamp_ms,value\n");
        let peaks: Vec<u64> = self.peaks_ms.iter().copied().collect();
        for w in peaks.windows(2) {
            let rr = (w[1] - w[0]) as f64;
            if (300.0..=2000.0).contains(&rr) {
                out.push_str(&format!("{},{:.2}\n", w[1], 60_000.0 / rr));
            }
        }
        out
    }

    pub fn to_csv(&self) -> String {
        let mut out = String::from("t_ms,raw_v,filtered_mv,r_peak\n");
        for s in &self.samples {
            let peak = u8::from(self.peaks_ms.contains(&s.t_ms));
            out.push_str(&format!(
                "{},{:.5},{:.3},{}\n",
                s.t_ms, s.raw_v, s.filtered_mv, peak
            ));
        }
        out
    }
}

/// What a probe of one endpoint last returned.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Probe {
    #[default]
    Unknown,
    Ok,
    Failed(String),
}

impl Probe {
    pub fn is_ok(&self) -> bool {
        matches!(self, Probe::Ok)
    }
}

/// Everything the worker learns, read by the UI each frame.
#[derive(Default)]
pub struct State {
    pub signal: SignalBuffer,
    /// Latest cog report (`/status`), as JSON.
    pub report: Option<serde_json::Value>,
    pub export: Probe,
    pub seed_api: Probe,
    pub cog_installed: Option<bool>,
    pub cog_running: Option<bool>,
    pub last_action: Option<String>,
    /// The cog's current config from the agent (`GET /api/v1/apps/<id>/config`).
    pub cog_config: Option<serde_json::Value>,
    pub last_poll: Option<Instant>,
}

impl State {
    pub fn report_str(&self, key: &str) -> Option<&str> {
        self.report.as_ref()?.get(key)?.as_str()
    }

    pub fn report_f64(&self, key: &str) -> Option<f64> {
        self.report.as_ref()?.get(key)?.as_f64()
    }
}

/// Parses the cog's `/raw` JSON body.
pub fn parse_raw(body: &serde_json::Value) -> (Vec<Sample>, Vec<u64>, f64, String) {
    let samples = body["samples"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|row| {
                    let r = row.as_array()?;
                    Some(Sample {
                        t_ms: r.first()?.as_u64()?,
                        raw_v: r.get(1)?.as_f64()?,
                        filtered_mv: r.get(2)?.as_f64()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let peaks = body["r_peaks_ms"]
        .as_array()
        .map(|a| a.iter().filter_map(serde_json::Value::as_u64).collect())
        .unwrap_or_default();
    let fs = body["sample_rate_hz"].as_f64().unwrap_or(0.0);
    let source = body["source"].as_str().unwrap_or("").to_string();
    (samples, peaks, fs, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: u64) -> Sample {
        Sample {
            t_ms: t,
            raw_v: 1.5,
            filtered_mv: 0.0,
        }
    }

    #[test]
    fn overlapping_polls_merge_without_duplicates() {
        let mut b = SignalBuffer::default();
        assert_eq!(b.merge(&[s(0), s(4), s(8)], &[4]), 3);
        assert_eq!(b.merge(&[s(4), s(8), s(12), s(16)], &[4, 12]), 2);
        assert_eq!(b.samples.len(), 5);
        assert_eq!(b.peaks_ms.len(), 2);
    }

    #[test]
    fn old_samples_and_peaks_age_out_and_restarts_reset() {
        let mut b = SignalBuffer::default();
        b.merge(&[s(0)], &[0]);
        b.merge(&[s(70_000)], &[]);
        assert_eq!(b.samples.len(), 1);
        assert!(b.peaks_ms.is_empty());
        // Clock jumps back far: a different cog run, start over.
        b.merge(&[s(1_000)], &[]);
        assert_eq!(b.samples.front().unwrap().t_ms, 1_000);
    }

    #[test]
    fn parses_the_cog_raw_shape() {
        let v = serde_json::json!({"source":"sim","sample_rate_hz":250.0,
            "samples":[[1,1.5,0.2],[5,1.6,0.3],["bad"]],"r_peaks_ms":[5]});
        let (samples, peaks, fs, src) = parse_raw(&v);
        assert_eq!(samples.len(), 2);
        assert_eq!(peaks, vec![5]);
        assert_eq!(fs, 250.0);
        assert_eq!(src, "sim");
    }

    #[test]
    fn ruview_reference_series_is_beat_to_beat_bpm_and_skips_bad_rr() {
        let mut b = SignalBuffer::default();
        b.merge(&[s(0), s(5_000)], &[1_000, 1_800, 2_600, 2_700, 3_500]);
        let csv = b.ruview_reference_csv();
        let rows: Vec<&str> = csv.lines().collect();
        assert_eq!(rows[0], "timestamp_ms,value");
        // 800 ms -> 75 bpm twice; the 100 ms gap is noise; 2700->3500 is 800 ms again.
        assert_eq!(&rows[1..], &["1800,75.00", "2600,75.00", "3500,75.00"]);
    }

    #[test]
    fn csv_marks_peaks() {
        let mut b = SignalBuffer::default();
        b.merge(&[s(0), s(4)], &[4]);
        let csv = b.to_csv();
        assert_eq!(csv.lines().count(), 3);
        assert!(csv.lines().last().unwrap().ends_with(",1"));
    }
}
