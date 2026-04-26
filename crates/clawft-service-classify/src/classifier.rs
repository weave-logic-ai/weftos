//! Classifier backend trait + the energy-based VAD impl.
//!
//! [`ClassifierBackend`] is the seam that lets a future llama.cpp
//! audio-classifier slot in without changing the service plumbing or
//! the wire shape subscribers depend on. Today there is one
//! implementation, [`EnergyClassifier`].

use serde::{Deserialize, Serialize};

use crate::{CLASS_SILENCE, CLASS_SPEECH, DEFAULT_VAD_RMS_THRESHOLD_DB, VAD_RMS_THRESHOLD_DB_ENV};

/// Wire shape published by [`crate::ClassifierService`] on each
/// pcm_chunk.
///
/// Stable across backends. The `class` field is intentionally a
/// `String` (not an enum) so a future backend can emit `"music"`,
/// `"noise"`, or anything else without forcing every subscriber to
/// re-deserialise.
///
/// ```json
/// {
///   "class": "speech",
///   "confidence": 0.42,
///   "rms_db": -32.4,
///   "sample_rate": 16000,
///   "samples": 8000,
///   "ts_ms": 3807924,
///   "source_node": "n-bfc4cd",
///   "source_seq": 3807924
/// }
/// ```
///
/// `confidence` is in `[0.0, 1.0]`. For [`EnergyClassifier`] this is
/// the clipped distance from the RMS threshold normalised against a
/// fixed dB band (see [`EnergyClassifier::CONFIDENCE_DB_SPAN`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Classification {
    /// Tagged class: `"speech"`, `"silence"`, and any future backend
    /// strings (`"music"`, `"noise"`, …). Treat as opaque except for
    /// the well-known constants in `crate::CLASS_*`.
    pub class: String,
    /// Backend-reported confidence in `[0.0, 1.0]`. For the energy
    /// classifier this is `clip((rms_db - threshold).abs() / SPAN)`.
    pub confidence: f32,
    /// RMS amplitude of the analysed PCM, in dBFS. Useful for
    /// rendering an audio meter alongside the classification.
    pub rms_db: f32,
    /// Sample rate the PCM was analysed at (Hz).
    pub sample_rate: u32,
    /// Number of i16 samples analysed for this classification.
    pub samples: u32,
    /// Producer-side timestamp (boot-relative monotonic ms). Echoed
    /// from the upstream `pcm_chunk.start_ts_ms` so downstream joiners
    /// can correlate the classification back to the source chunk.
    pub ts_ms: u64,
    /// Node-id of the audio source (the ESP32 in the mesh, not the
    /// daemon that runs this classifier). Path-as-attribution is the
    /// R3.2 rule for derived outputs.
    pub source_node: String,
    /// Producer-side sequence id of the source chunk. Same value as
    /// `ts_ms` when the source uses start_ts_ms as its seq, but kept
    /// as a separate field so a future source that decouples them
    /// stays correctly attributed.
    pub source_seq: u64,
}

/// Pluggable backend that classifies a window of PCM samples.
///
/// Implementors must be `Send + Sync` so the service can hold them
/// in an `Arc` and dispatch from any tokio worker.
///
/// `pcm_i16` is the **decoded** sample buffer (already converted from
/// the wire's base64-i16le); `sample_rate` is in Hz. The backend is
/// responsible for any silence/speech threshold logic, ML inference,
/// or external HTTP call.
///
/// Returning a `Classification` is mandatory — a backend that wants
/// to "abstain" should still emit a class (e.g. `"unknown"`) with
/// `confidence: 0.0` so subscribers see a stable cadence.
pub trait ClassifierBackend: Send + Sync + std::fmt::Debug {
    /// Classify a single window of PCM samples.
    fn classify(&self, pcm_i16: &[i16], sample_rate: u32) -> Classification;
}

/// Floor for the RMS-in-dB calculation. Pure-zero windows would map
/// to `-inf` dB; we clip to this value so the wire shape carries a
/// finite number that downstream JSON consumers can parse.
const RMS_DB_FLOOR: f32 = -120.0;

/// Energy-based voice-activity classifier.
///
/// Computes the RMS of the i16 PCM window, converts to dBFS against
/// the i16 full-scale value (32767), and emits `"speech"` iff
/// `rms_db >= threshold_db`. The default threshold is
/// [`DEFAULT_VAD_RMS_THRESHOLD_DB`] (`-45 dBFS`); override via the
/// [`VAD_RMS_THRESHOLD_DB_ENV`] env var or the explicit constructor.
///
/// **Why energy and not Silero/llama.cpp now?** Per the project memo
/// (`project_vad_classifier_via_llamacpp.md`), "even if we ship
/// VAD-only first, plan the path so a llama.cpp-hosted classifier
/// slots in later." Energy is the trivial floor; the
/// [`ClassifierBackend`] trait is the seam for the upgrade.
#[derive(Debug, Clone)]
pub struct EnergyClassifier {
    /// dBFS threshold. At-or-above → `"speech"`, below → `"silence"`.
    pub threshold_db: f32,
}

impl EnergyClassifier {
    /// dB span used to normalise the confidence value. The further
    /// the observed RMS sits from `threshold_db` (in either
    /// direction), the higher the confidence, capped at 1.0 once the
    /// distance reaches this value. 12 dB is a deliberate "loud
    /// enough or quiet enough that the classifier is sure" band — it
    /// matches roughly a 4× amplitude separation.
    pub const CONFIDENCE_DB_SPAN: f32 = 12.0;

    /// Build with a specific dBFS threshold.
    pub fn new(threshold_db: f32) -> Self {
        Self { threshold_db }
    }

    /// Build a classifier honouring the [`VAD_RMS_THRESHOLD_DB_ENV`]
    /// env var. Falls back to [`DEFAULT_VAD_RMS_THRESHOLD_DB`] when
    /// the var is unset, empty, or unparsable.
    pub fn from_env() -> Self {
        let threshold_db = std::env::var(VAD_RMS_THRESHOLD_DB_ENV)
            .ok()
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(DEFAULT_VAD_RMS_THRESHOLD_DB);
        Self::new(threshold_db)
    }

    /// Compute the RMS of a PCM-i16 window, returning dBFS against
    /// the i16 full-scale value (32767). Empty input → [`RMS_DB_FLOOR`].
    pub fn rms_db(pcm_i16: &[i16]) -> f32 {
        if pcm_i16.is_empty() {
            return RMS_DB_FLOOR;
        }
        // Accumulate the sum of squares in f64 to avoid overflow on
        // long windows (a 3 s 16 kHz window is 48 000 samples; the
        // worst-case sum-of-squares is ~5e13, fits in u64 but f64 is
        // simpler and the precision is plenty).
        let mut acc: f64 = 0.0;
        for &s in pcm_i16 {
            let f = s as f64;
            acc += f * f;
        }
        let mean = acc / (pcm_i16.len() as f64);
        let rms = mean.sqrt();
        if rms <= 0.0 {
            return RMS_DB_FLOOR;
        }
        let db = 20.0 * (rms / 32_767.0).log10();
        (db as f32).max(RMS_DB_FLOOR)
    }

    /// Confidence as the clipped distance from the threshold,
    /// normalised by [`Self::CONFIDENCE_DB_SPAN`] into `[0.0, 1.0]`.
    fn confidence(rms_db: f32, threshold_db: f32) -> f32 {
        let distance = (rms_db - threshold_db).abs();
        (distance / Self::CONFIDENCE_DB_SPAN).clamp(0.0, 1.0)
    }
}

impl Default for EnergyClassifier {
    fn default() -> Self {
        Self::new(DEFAULT_VAD_RMS_THRESHOLD_DB)
    }
}

impl ClassifierBackend for EnergyClassifier {
    fn classify(&self, pcm_i16: &[i16], sample_rate: u32) -> Classification {
        let rms_db = Self::rms_db(pcm_i16);
        let class = if rms_db >= self.threshold_db {
            CLASS_SPEECH
        } else {
            CLASS_SILENCE
        };
        let confidence = Self::confidence(rms_db, self.threshold_db);
        Classification {
            class: class.to_string(),
            confidence,
            rms_db,
            sample_rate,
            samples: pcm_i16.len() as u32,
            // ts_ms / source_node / source_seq are filled in by the
            // service layer — the backend has no opinion on them.
            ts_ms: 0,
            source_node: String::new(),
            source_seq: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Synthesise a 16 kHz mono sine wave at the given amplitude
    /// (peak in i16 units) and duration in ms. Used by the
    /// energy-classifier tests so the fixture controls both the RMS
    /// and the duration without coupling to a real mic capture.
    fn sine_i16(sample_rate: u32, freq_hz: f32, peak: i16, duration_ms: u32) -> Vec<i16> {
        let n = ((sample_rate as u64) * (duration_ms as u64) / 1000) as usize;
        let mut out = Vec::with_capacity(n);
        let step = 2.0 * PI * freq_hz / (sample_rate as f32);
        for i in 0..n {
            let v = (peak as f32) * (step * (i as f32)).sin();
            out.push(v.round() as i16);
        }
        out
    }

    #[test]
    fn rms_db_of_silence_is_floor() {
        let zeros = vec![0i16; 16_000];
        let db = EnergyClassifier::rms_db(&zeros);
        assert!(db <= RMS_DB_FLOOR + 0.01, "got {db}");
    }

    #[test]
    fn rms_db_of_full_scale_sine_is_near_minus_three() {
        // A pure sine at peak=full_scale has RMS = full_scale / sqrt(2),
        // i.e. -3.01 dBFS. Sanity-check the formula.
        let sine = sine_i16(16_000, 440.0, i16::MAX, 100);
        let db = EnergyClassifier::rms_db(&sine);
        assert!(
            (db - (-3.01)).abs() < 0.5,
            "expected ~-3 dBFS, got {db}"
        );
    }

    #[test]
    fn energy_classifier_emits_speech_above_threshold() {
        // -20 dBFS: peak ≈ 32767 * 10^(-20/20) / sqrt(2) * sqrt(2)
        //         ≈ 32767 * 0.1 ≈ 3277 (peak of sine at -20 dB RMS)
        // Using peak ≈ 3277 gives RMS ≈ 2317 ≈ -23 dBFS. To target
        // -20 dBFS RMS exactly, peak = 32767 * 10^(-20/20) * sqrt(2)
        //                            ≈ 32767 * 0.1 * 1.414 ≈ 4632.
        let sine = sine_i16(16_000, 440.0, 4_632, 200);
        let cls = EnergyClassifier::new(-45.0);
        let out = cls.classify(&sine, 16_000);
        assert_eq!(out.class, CLASS_SPEECH, "expected speech, got {out:?}");
        // Confidence should be high: -20 dBFS is 25 dB above -45,
        // well past the 12 dB span → clipped to 1.0.
        assert!((out.confidence - 1.0).abs() < 1e-6, "got {}", out.confidence);
        assert!(out.rms_db > -25.0 && out.rms_db < -15.0, "got {}", out.rms_db);
        assert_eq!(out.sample_rate, 16_000);
        assert_eq!(out.samples as usize, sine.len());
    }

    #[test]
    fn energy_classifier_emits_silence_below_threshold() {
        let zeros = vec![0i16; 16_000];
        let cls = EnergyClassifier::new(-45.0);
        let out = cls.classify(&zeros, 16_000);
        assert_eq!(out.class, CLASS_SILENCE);
        // Confidence: -120 dBFS is 75 dB below -45 — well past the
        // 12 dB span, so clipped to 1.0 ("very confident silent").
        assert!((out.confidence - 1.0).abs() < 1e-6, "got {}", out.confidence);
        assert!(out.rms_db <= RMS_DB_FLOOR + 0.01);
    }

    #[test]
    fn energy_classifier_confidence_scales_near_threshold() {
        // A signal exactly at the threshold should return ~0.0
        // confidence (we're indifferent between speech and silence).
        // Build a sine whose RMS is ≈ -45 dBFS:
        //   peak = 32767 * 10^(-45/20) * sqrt(2)
        //        ≈ 32767 * 0.005623 * 1.414 ≈ 261
        let sine = sine_i16(16_000, 440.0, 261, 200);
        let cls = EnergyClassifier::new(-45.0);
        let out = cls.classify(&sine, 16_000);
        // RMS will land within ~1 dB of -45; confidence should be
        // small (< 0.1) regardless of which side it lands on.
        assert!(
            out.confidence < 0.2,
            "expected near-zero confidence at threshold, got {} (rms_db={})",
            out.confidence,
            out.rms_db
        );
    }

    #[test]
    fn classification_value_serializes_with_expected_keys() {
        // Pin the wire shape: subscribers (Explorer, future llama.cpp
        // gate, future fusion stage) read these field names verbatim.
        let cls = Classification {
            class: "speech".to_string(),
            confidence: 0.42,
            rms_db: -32.4,
            sample_rate: 16_000,
            samples: 8_000,
            ts_ms: 3_807_924,
            source_node: "n-bfc4cd".to_string(),
            source_seq: 3_807_924,
        };
        let v = serde_json::to_value(&cls).unwrap();
        let obj = v.as_object().expect("Classification serializes as object");
        let expected = [
            "class",
            "confidence",
            "rms_db",
            "sample_rate",
            "samples",
            "ts_ms",
            "source_node",
            "source_seq",
        ];
        for k in expected {
            assert!(obj.contains_key(k), "missing field: {k}");
        }
        assert_eq!(obj["class"], "speech");
        assert_eq!(obj["sample_rate"], 16_000);
        assert_eq!(obj["source_node"], "n-bfc4cd");
        // Round-trip back to the typed shape.
        let back: Classification = serde_json::from_value(v).unwrap();
        assert_eq!(back.class, cls.class);
        assert_eq!(back.samples, cls.samples);
    }

    #[test]
    fn from_env_uses_default_when_var_unset() {
        // SAFETY: set/remove_var are unsafe on Rust 2024 (race with
        // reader threads); test is single-threaded.
        unsafe { std::env::remove_var(VAD_RMS_THRESHOLD_DB_ENV) };
        let cls = EnergyClassifier::from_env();
        assert!((cls.threshold_db - DEFAULT_VAD_RMS_THRESHOLD_DB).abs() < f32::EPSILON);
    }

    #[test]
    fn from_env_parses_override() {
        unsafe { std::env::set_var(VAD_RMS_THRESHOLD_DB_ENV, "-30.0") };
        let cls = EnergyClassifier::from_env();
        assert!((cls.threshold_db - (-30.0)).abs() < f32::EPSILON);
        unsafe { std::env::remove_var(VAD_RMS_THRESHOLD_DB_ENV) };
    }
}
