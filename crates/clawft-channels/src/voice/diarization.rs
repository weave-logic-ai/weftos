//! Multi-speaker diarization (WEFT-227).
//!
//! Segments continuous audio into speaker-labeled time ranges so transcripts
//! can attribute multi-party speech ("who spoke when"). Complements
//! [`super::speaker::SpeakerRegistry`] (identity / enrollment over whole
//! utterances) with **within-buffer** speaker change detection.
//!
//! # Layering
//!
//! - Pure types ([`DiarizationSegment`], [`DiarizationResult`],
//!   [`SpeakerLabeledTurn`], [`DiarizedTranscript`]) are serde-ready for
//!   transcript logs and wire events.
//! - [`DiarizationBackend`] is the engine seam — same pattern as
//!   [`super::stt::SttBackend`] / [`super::speaker::SpeakerEmbedder`].
//! - Default / CI path: [`FixtureDiarizer`] + [`EnergyGapDiarizer`] (no
//!   native model deps).
//! - Production path when an embedder is available:
//!   [`EmbeddingDiarizer`] (windowed ECAPA → cosine cluster).
//! - Real pyannote / sherpa-onnx clustering is **not** linked by default
//!   (ADR-053 rejected in-process sherpa-rs as the canonical STT path;
//!   full neural diarization remains a staged engine). See
//!   [`SherpaDiarizer`] and the `diarization-sherpa` feature for the
//!   explicit TODO seam.
//!
//! # Feature flags
//!
//! - (default) fixture + energy + embedding-cluster backends compile with
//!   the `voice` feature alone.
//! - `diarization-sherpa` — compile the sherpa-onnx / pyannote placeholder
//!   that returns a clear "engine not staged" error until models ship.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::speaker::{SpeakerEmbedder, SpeakerId, cosine};
use super::types::VoiceError;

// ── Types ───────────────────────────────────────────────────────────────────

/// Stable label for a diarized speaker within one diarization pass
/// (`"spk-0"`, `"spk-1"`, …). Distinct from enrolled
/// [`SpeakerId`](super::speaker::SpeakerId) until a registry match upgrades
/// the label.
pub type DiarizationLabel = String;

/// One continuous time range attributed to a single speaker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiarizationSegment {
    /// Onset in milliseconds from buffer start.
    pub start_ms: u32,
    /// Offset in milliseconds from buffer start (exclusive end of speech).
    pub end_ms: u32,
    /// Speaker label for this range (`spk-0`, …).
    pub speaker: DiarizationLabel,
    /// Engine confidence in \[0.0, 1.0\] (1.0 for fixture / forced labels).
    pub confidence: f32,
}

impl DiarizationSegment {
    /// Duration of the segment in milliseconds.
    pub fn duration_ms(&self) -> u32 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// Full diarization of one PCM buffer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiarizationResult {
    /// Ordered (non-overlapping preferred) speaker segments.
    pub segments: Vec<DiarizationSegment>,
    /// Distinct speaker count (`segments` labels unique).
    pub num_speakers: usize,
    /// Engine wire name (`fixture`, `energy-gap`, `embedding-cluster`, …).
    pub engine: String,
}

impl DiarizationResult {
    /// Empty result (silence / no speech / backend no-op).
    pub fn empty(engine: impl Into<String>) -> Self {
        Self {
            segments: Vec::new(),
            num_speakers: 0,
            engine: engine.into(),
        }
    }

    /// Build from segments, computing `num_speakers` from unique labels.
    pub fn from_segments(engine: impl Into<String>, segments: Vec<DiarizationSegment>) -> Self {
        let mut labels = segments
            .iter()
            .map(|s| s.speaker.as_str())
            .collect::<Vec<_>>();
        labels.sort_unstable();
        labels.dedup();
        Self {
            num_speakers: labels.len(),
            segments,
            engine: engine.into(),
        }
    }

    /// Unique speaker labels in first-seen order.
    pub fn labels(&self) -> Vec<DiarizationLabel> {
        let mut out = Vec::new();
        for s in &self.segments {
            if !out.iter().any(|l| l == &s.speaker) {
                out.push(s.speaker.clone());
            }
        }
        out
    }
}

/// One speaker turn with optional attributed text (for multi-speaker
/// transcript rendering).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeakerLabeledTurn {
    /// Diarization label or enrolled [`SpeakerId`].
    pub speaker: DiarizationLabel,
    /// Text attributed to this turn (may be empty when only diarization ran).
    pub text: String,
    /// Turn onset (ms).
    pub start_ms: u32,
    /// Turn offset (ms).
    pub end_ms: u32,
    /// Confidence from diarization (and optionally STT).
    pub confidence: f32,
}

/// STT text + diarization, ready for transcript logs / UI multi-speaker view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiarizedTranscript {
    /// Full concatenated transcript (same as plain STT when single-speaker).
    pub text: String,
    /// Per-speaker turns (text split proportionally by segment duration when
    /// no token timing is available).
    pub turns: Vec<SpeakerLabeledTurn>,
    /// Underlying diarization.
    pub diarization: DiarizationResult,
}

// ── Backend trait ───────────────────────────────────────────────────────────

/// Multi-speaker diarization engine seam (WEFT-227).
///
/// Implementations must be cheap to hold behind `Arc<dyn DiarizationBackend>`
/// and safe to call concurrently on independent buffers.
#[async_trait]
pub trait DiarizationBackend: Send + Sync {
    /// Diarize mono PCM (`s16le`) into speaker-labeled time ranges.
    async fn diarize(
        &self,
        samples: &[i16],
        sample_rate: u32,
    ) -> Result<DiarizationResult, VoiceError>;

    /// Wire name for logs / `DiarizationResult.engine`.
    fn engine_name(&self) -> &'static str;

    /// Optional pre-warm (load ONNX, etc.). Default is a no-op.
    async fn warm(&self) -> Result<(), VoiceError> {
        Ok(())
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Format a diarization label for speaker index `i` (`spk-0`, `spk-1`, …).
pub fn speaker_label(i: usize) -> DiarizationLabel {
    format!("spk-{i}")
}

/// Merge full-buffer STT text with diarization segments into a
/// [`DiarizedTranscript`].
///
/// Without per-token timing, text is split across segments by **character
/// share proportional to segment duration** (whitespace-aware best-effort).
/// Single-segment / empty-diarization paths keep the full text on one turn.
pub fn label_transcript(text: impl Into<String>, diarization: DiarizationResult) -> DiarizedTranscript {
    let text = text.into();
    if diarization.segments.is_empty() {
        return DiarizedTranscript {
            text: text.clone(),
            turns: if text.is_empty() {
                Vec::new()
            } else {
                vec![SpeakerLabeledTurn {
                    speaker: speaker_label(0),
                    text: text.clone(),
                    start_ms: 0,
                    end_ms: 0,
                    confidence: 1.0,
                }]
            },
            diarization,
        };
    }

    if diarization.segments.len() == 1 {
        let s = &diarization.segments[0];
        return DiarizedTranscript {
            text: text.clone(),
            turns: vec![SpeakerLabeledTurn {
                speaker: s.speaker.clone(),
                text: text.clone(),
                start_ms: s.start_ms,
                end_ms: s.end_ms,
                confidence: s.confidence,
            }],
            diarization,
        };
    }

    let total_dur: u32 = diarization
        .segments
        .iter()
        .map(DiarizationSegment::duration_ms)
        .sum::<u32>()
        .max(1);
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut turns = Vec::with_capacity(diarization.segments.len());
    let mut cursor = 0usize;
    for (i, seg) in diarization.segments.iter().enumerate() {
        let share = if i + 1 == diarization.segments.len() {
            n.saturating_sub(cursor)
        } else {
            let raw = (n as u64 * u64::from(seg.duration_ms()) / u64::from(total_dur)) as usize;
            raw.min(n.saturating_sub(cursor))
        };
        // Prefer splitting on whitespace so words don't tear mid-token.
        let mut end = cursor + share;
        if end < n && end > cursor
            && let Some(rel) = chars[cursor..end].iter().rposition(|c| c.is_whitespace())
                && rel > 0 {
                    end = cursor + rel + 1;
                }
        if i + 1 == diarization.segments.len() {
            end = n;
        }
        let piece: String = chars[cursor..end].iter().collect();
        cursor = end;
        turns.push(SpeakerLabeledTurn {
            speaker: seg.speaker.clone(),
            text: piece.trim().to_string(),
            start_ms: seg.start_ms,
            end_ms: seg.end_ms,
            confidence: seg.confidence,
        });
    }

    DiarizedTranscript {
        text,
        turns,
        diarization,
    }
}

/// Upgrade diarization labels via a registry map (`spk-0` → enrolled id/name
/// when a caller already matched embeddings). Unmapped labels stay as-is.
pub fn remap_labels(
    result: &mut DiarizationResult,
    map: &HashMap<DiarizationLabel, SpeakerId>,
) {
    for seg in &mut result.segments {
        if let Some(id) = map.get(&seg.speaker) {
            seg.speaker = id.clone();
        }
    }
    // Recompute unique count after remap.
    *result = DiarizationResult::from_segments(result.engine.clone(), result.segments.clone());
}

// ── Fixture backend (tests + deterministic demos) ───────────────────────────

/// Returns preconfigured segments (ignores PCM content except duration clamp).
///
/// Ideal for unit tests and multi-speaker fixture audio where ground-truth
/// diarization is known a priori.
#[derive(Debug, Clone)]
pub struct FixtureDiarizer {
    segments: Vec<DiarizationSegment>,
}

impl FixtureDiarizer {
    /// Build from explicit segments.
    pub fn new(segments: Vec<DiarizationSegment>) -> Self {
        Self { segments }
    }

    /// Two-speaker fixture: first half `spk-0`, second half `spk-1`.
    ///
    /// `total_ms` should match the expected buffer duration; when the actual
    /// buffer differs, [`diarize`](DiarizationBackend::diarize) rescales.
    pub fn two_speaker_split(total_ms: u32) -> Self {
        let mid = total_ms / 2;
        Self::new(vec![
            DiarizationSegment {
                start_ms: 0,
                end_ms: mid,
                speaker: speaker_label(0),
                confidence: 1.0,
            },
            DiarizationSegment {
                start_ms: mid,
                end_ms: total_ms,
                speaker: speaker_label(1),
                confidence: 1.0,
            },
        ])
    }
}

#[async_trait]
impl DiarizationBackend for FixtureDiarizer {
    async fn diarize(
        &self,
        samples: &[i16],
        sample_rate: u32,
    ) -> Result<DiarizationResult, VoiceError> {
        if sample_rate == 0 {
            return Err(VoiceError::Config(
                "diarization: sample_rate must be > 0".into(),
            ));
        }
        let buf_ms = if samples.is_empty() {
            0u32
        } else {
            ((samples.len() as u64 * 1000) / u64::from(sample_rate)) as u32
        };
        if self.segments.is_empty() || buf_ms == 0 {
            return Ok(DiarizationResult::empty(self.engine_name()));
        }
        let max_end = self.segments.iter().map(|s| s.end_ms).max().unwrap_or(0).max(1);
        let segments = self
            .segments
            .iter()
            .map(|s| {
                // Rescale fixture times into the actual buffer length.
                let start_ms = ((u64::from(s.start_ms) * u64::from(buf_ms)) / u64::from(max_end)) as u32;
                let end_ms = ((u64::from(s.end_ms) * u64::from(buf_ms)) / u64::from(max_end)) as u32;
                DiarizationSegment {
                    start_ms,
                    end_ms: end_ms.max(start_ms),
                    speaker: s.speaker.clone(),
                    confidence: s.confidence,
                }
            })
            .filter(|s| s.duration_ms() > 0)
            .collect();
        Ok(DiarizationResult::from_segments(self.engine_name(), segments))
    }

    fn engine_name(&self) -> &'static str {
        "fixture"
    }
}

// ── Energy-gap backend (model-free) ─────────────────────────────────────────

/// Model-free diarizer: split speech islands on silence, then assign speaker
/// labels by clustering crude per-island features (RMS + zero-crossing rate).
///
/// Not a substitute for neural diarization, but produces multi-speaker labels
/// on real multi-talker buffers without ONNX / sherpa, so CI and headless
/// demos work. Prefer [`EmbeddingDiarizer`] when an ECAPA embedder is live.
#[derive(Debug, Clone)]
pub struct EnergyGapDiarizer {
    /// Frame length for energy (ms).
    pub frame_ms: u32,
    /// Silence threshold relative to peak RMS (0..1). Below = silence.
    pub silence_ratio: f32,
    /// Minimum silence gap (ms) that splits speech islands.
    pub min_gap_ms: u32,
    /// Minimum speech island length (ms).
    pub min_speech_ms: u32,
    /// Max speakers to emit (cap cluster count).
    pub max_speakers: usize,
    /// Cosine-like distance threshold on the 2-d feature for same-speaker.
    pub same_speaker_dist: f32,
}

impl Default for EnergyGapDiarizer {
    fn default() -> Self {
        Self {
            frame_ms: 30,
            silence_ratio: 0.12,
            min_gap_ms: 250,
            min_speech_ms: 200,
            max_speakers: 8,
            same_speaker_dist: 0.35,
        }
    }
}

#[async_trait]
impl DiarizationBackend for EnergyGapDiarizer {
    async fn diarize(
        &self,
        samples: &[i16],
        sample_rate: u32,
    ) -> Result<DiarizationResult, VoiceError> {
        if sample_rate == 0 {
            return Err(VoiceError::Config(
                "diarization: sample_rate must be > 0".into(),
            ));
        }
        if samples.is_empty() {
            return Ok(DiarizationResult::empty(self.engine_name()));
        }

        let frame_len = ((u64::from(self.frame_ms) * u64::from(sample_rate)) / 1000)
            .max(1) as usize;
        let n_frames = samples.len().div_ceil(frame_len);
        let mut frame_rms = Vec::with_capacity(n_frames);
        for i in 0..n_frames {
            let start = i * frame_len;
            let end = (start + frame_len).min(samples.len());
            let slice = &samples[start..end];
            let sum_sq: f64 = slice.iter().map(|&s| {
                let x = f64::from(s);
                x * x
            }).sum();
            let rms = (sum_sq / slice.len().max(1) as f64).sqrt();
            frame_rms.push(rms as f32);
        }
        let peak = frame_rms.iter().cloned().fold(0.0_f32, f32::max);
        if peak <= f32::EPSILON {
            return Ok(DiarizationResult::empty(self.engine_name()));
        }
        let thr = peak * self.silence_ratio.clamp(0.01, 0.9);
        let speech: Vec<bool> = frame_rms.iter().map(|&r| r >= thr).collect();

        // Speech islands in frame indices.
        let mut islands: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < speech.len() {
            if !speech[i] {
                i += 1;
                continue;
            }
            let start = i;
            while i < speech.len() && speech[i] {
                i += 1;
            }
            islands.push((start, i)); // [start, end)
        }

        // Merge islands separated by short gaps.
        let min_gap_frames =
            ((u64::from(self.min_gap_ms) * u64::from(sample_rate)) / 1000 / frame_len as u64).max(1)
                as usize;
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (s, e) in islands {
            if let Some(last) = merged.last_mut()
                && s.saturating_sub(last.1) <= min_gap_frames {
                    last.1 = e;
                    continue;
                }
            merged.push((s, e));
        }

        let min_speech_frames =
            ((u64::from(self.min_speech_ms) * u64::from(sample_rate)) / 1000 / frame_len as u64)
                .max(1) as usize;
        merged.retain(|(s, e)| e.saturating_sub(*s) >= min_speech_frames);
        if merged.is_empty() {
            return Ok(DiarizationResult::empty(self.engine_name()));
        }

        // Features: [log_rms_mean, zcr] per island.
        let mut feats: Vec<[f32; 2]> = Vec::with_capacity(merged.len());
        for &(fs, fe) in &merged {
            let start = fs * frame_len;
            let end = (fe * frame_len).min(samples.len());
            let slice = &samples[start..end];
            let rms = {
                let sum_sq: f64 = slice.iter().map(|&s| {
                    let x = f64::from(s);
                    x * x
                }).sum();
                ((sum_sq / slice.len().max(1) as f64).sqrt() as f32).max(1.0)
            };
            let mut zc = 0u32;
            for w in slice.windows(2) {
                if (w[0] >= 0) != (w[1] >= 0) {
                    zc += 1;
                }
            }
            let zcr = zc as f32 / slice.len().max(1) as f32;
            feats.push([rms.ln(), zcr]);
        }

        // Greedy cluster features into speaker ids.
        let mut cluster_centroids: Vec<[f32; 2]> = Vec::new();
        let mut island_spk: Vec<usize> = Vec::with_capacity(feats.len());
        for f in &feats {
            let mut best = None::<(usize, f32)>;
            for (ci, c) in cluster_centroids.iter().enumerate() {
                let d = feature_dist(f, c);
                if d <= self.same_speaker_dist && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                    best = Some((ci, d));
                }
            }
            let spk = if let Some((ci, _)) = best {
                // Running mean update.
                let n = island_spk.iter().filter(|&&s| s == ci).count() as f32;
                let c = &mut cluster_centroids[ci];
                c[0] = (c[0] * n + f[0]) / (n + 1.0);
                c[1] = (c[1] * n + f[1]) / (n + 1.0);
                ci
            } else if cluster_centroids.len() < self.max_speakers.max(1) {
                cluster_centroids.push(*f);
                cluster_centroids.len() - 1
            } else {
                // Force into nearest when at cap.
                let (ci, _) = cluster_centroids
                    .iter()
                    .enumerate()
                    .map(|(ci, c)| (ci, feature_dist(f, c)))
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap_or((0, 0.0));
                ci
            };
            island_spk.push(spk);
        }

        let ms_per_frame = self.frame_ms.max(1);
        let segments = merged
            .iter()
            .zip(island_spk.iter())
            .map(|(&(fs, fe), &spk)| DiarizationSegment {
                start_ms: (fs as u32).saturating_mul(ms_per_frame),
                end_ms: (fe as u32).saturating_mul(ms_per_frame),
                speaker: speaker_label(spk),
                confidence: 0.55,
            })
            .collect();

        Ok(DiarizationResult::from_segments(self.engine_name(), segments))
    }

    fn engine_name(&self) -> &'static str {
        "energy-gap"
    }
}

fn feature_dist(a: &[f32; 2], b: &[f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

// ── Embedding-cluster backend (ECAPA / SpeakerEmbedder) ─────────────────────

/// Windowed embedding diarizer over any [`SpeakerEmbedder`].
///
/// Splits the buffer into fixed windows, embeds each, and merges consecutive
/// windows whose cosine similarity exceeds `merge_threshold`. New clusters
/// open when similarity falls below threshold. This is the preferred live
/// path once ECAPA weights are staged (`clawft-voice-onnx`), without pulling
/// sherpa-rs.
pub struct EmbeddingDiarizer {
    embedder: Arc<dyn SpeakerEmbedder>,
    /// Analysis window (ms).
    pub window_ms: u32,
    /// Hop between windows (ms).
    pub hop_ms: u32,
    /// Cosine threshold to stay on the same speaker (default ~0.55).
    pub merge_threshold: f32,
    /// Drop windows shorter than this after merge (ms).
    pub min_segment_ms: u32,
}

impl EmbeddingDiarizer {
    /// Build with defaults: 1500 ms window, 750 ms hop, 0.55 merge.
    pub fn new(embedder: Arc<dyn SpeakerEmbedder>) -> Self {
        Self {
            embedder,
            window_ms: 1_500,
            hop_ms: 750,
            merge_threshold: 0.55,
            min_segment_ms: 300,
        }
    }
}

#[async_trait]
impl DiarizationBackend for EmbeddingDiarizer {
    async fn diarize(
        &self,
        samples: &[i16],
        sample_rate: u32,
    ) -> Result<DiarizationResult, VoiceError> {
        if sample_rate == 0 {
            return Err(VoiceError::Config(
                "diarization: sample_rate must be > 0".into(),
            ));
        }
        if samples.is_empty() {
            return Ok(DiarizationResult::empty(self.engine_name()));
        }

        let win = ((u64::from(self.window_ms) * u64::from(sample_rate)) / 1000).max(1) as usize;
        let hop = ((u64::from(self.hop_ms) * u64::from(sample_rate)) / 1000).max(1) as usize;
        if samples.len() < win / 4 {
            // Too short — single speaker covering the whole buffer.
            return Ok(DiarizationResult::from_segments(
                self.engine_name(),
                vec![DiarizationSegment {
                    start_ms: 0,
                    end_ms: ((samples.len() as u64 * 1000) / u64::from(sample_rate)) as u32,
                    speaker: speaker_label(0),
                    confidence: 0.5,
                }],
            ));
        }

        struct Win {
            start_ms: u32,
            end_ms: u32,
            emb: Vec<f32>,
        }
        let mut windows: Vec<Win> = Vec::new();
        let mut offset = 0usize;
        while offset < samples.len() {
            let end = (offset + win).min(samples.len());
            if end.saturating_sub(offset) < win / 4 {
                break;
            }
            let emb = self
                .embedder
                .embed(&samples[offset..end], sample_rate)
                .await?;
            let start_ms = ((offset as u64 * 1000) / u64::from(sample_rate)) as u32;
            let end_ms = ((end as u64 * 1000) / u64::from(sample_rate)) as u32;
            windows.push(Win {
                start_ms,
                end_ms,
                emb,
            });
            if end >= samples.len() {
                break;
            }
            offset += hop;
        }
        if windows.is_empty() {
            return Ok(DiarizationResult::empty(self.engine_name()));
        }

        // Online cluster: keep centroid per speaker; assign each window.
        let mut centroids: Vec<Vec<f32>> = Vec::new();
        let mut assignments: Vec<usize> = Vec::with_capacity(windows.len());
        for w in &windows {
            let mut best: Option<(usize, f32)> = None;
            for (ci, c) in centroids.iter().enumerate() {
                let score = cosine(&w.emb, c);
                if score >= self.merge_threshold
                    && best.map(|(_, b)| score > b).unwrap_or(true)
                {
                    best = Some((ci, score));
                }
            }
            let spk = if let Some((ci, _)) = best {
                // Fold into centroid.
                let n = assignments.iter().filter(|&&a| a == ci).count() as f32;
                for (c, e) in centroids[ci].iter_mut().zip(w.emb.iter()) {
                    *c = (*c * n + *e) / (n + 1.0);
                }
                ci
            } else {
                centroids.push(w.emb.clone());
                centroids.len() - 1
            };
            assignments.push(spk);
        }

        // Merge consecutive same-label windows into segments.
        let mut segments: Vec<DiarizationSegment> = Vec::new();
        let mut i = 0;
        while i < windows.len() {
            let spk = assignments[i];
            let start_ms = windows[i].start_ms;
            let mut end_ms = windows[i].end_ms;
            let mut j = i + 1;
            while j < windows.len() && assignments[j] == spk {
                end_ms = windows[j].end_ms;
                j += 1;
            }
            if end_ms.saturating_sub(start_ms) >= self.min_segment_ms {
                // Confidence = mean cosine to final centroid.
                let mut scores = 0.0_f32;
                let mut n = 0_u32;
                for w in &windows[i..j] {
                    scores += cosine(&w.emb, &centroids[spk]);
                    n += 1;
                }
                segments.push(DiarizationSegment {
                    start_ms,
                    end_ms,
                    speaker: speaker_label(spk),
                    confidence: if n == 0 { 0.5 } else { (scores / n as f32).clamp(0.0, 1.0) },
                });
            }
            i = j;
        }

        if segments.is_empty() {
            return Ok(DiarizationResult::empty(self.engine_name()));
        }
        Ok(DiarizationResult::from_segments(self.engine_name(), segments))
    }

    fn engine_name(&self) -> &'static str {
        "embedding-cluster"
    }

    async fn warm(&self) -> Result<(), VoiceError> {
        // Drive a tiny throwaway embed so the first real call is warm.
        let _ = self.embedder.embed(&[0i16; 1600], 16_000).await;
        Ok(())
    }
}

// ── Sherpa / pyannote seam (feature-gated placeholder) ──────────────────────

/// Placeholder for a future sherpa-onnx / pyannote neural diarization engine.
///
/// # Status (WEFT-227)
///
/// ADR-053 rejected in-process **sherpa-rs STT** as the canonical path
/// (substrate whisper remains). Neural **diarization** may still use
/// in-process ONNX (pyannote segmentation + embedding) without replacing
/// STT — but the weights, export, and `sherpa-onnx` / `ort` wiring are
/// **not staged** in this tree yet.
///
/// ## TODO(engine)
///
/// 1. Stage a pyannote-style segmentation + embedding ONNX bundle under
///    `.weftos/models/diarization/`.
/// 2. Prefer `ort` (already used by `clawft-voice-onnx`) over pulling the
///    full `sherpa-rs` crate for STT again.
/// 3. Implement [`DiarizationBackend`] here; gate with feature
///    `diarization-sherpa`.
/// 4. Keep [`FixtureDiarizer`] / [`EnergyGapDiarizer`] /
///    [`EmbeddingDiarizer`] as the default zero-weight path.
///
/// Constructing this type always succeeds; [`diarize`](DiarizationBackend::diarize)
/// returns [`VoiceError::Config`] until the engine is implemented.
#[derive(Debug, Clone, Default)]
pub struct SherpaDiarizer {
    /// Reserved model directory (unused until engine lands).
    pub model_dir: Option<String>,
}

impl SherpaDiarizer {
    /// Create an unstaged sherpa/pyannote diarizer placeholder.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DiarizationBackend for SherpaDiarizer {
    async fn diarize(
        &self,
        _samples: &[i16],
        _sample_rate: u32,
    ) -> Result<DiarizationResult, VoiceError> {
        // Always-on seam so callers can depend on the type without a
        // feature flag; the feature only exists for future native deps.
        Err(VoiceError::Config(
            "diarization: sherpa/pyannote engine not staged (WEFT-227 TODO). \
             Use FixtureDiarizer, EnergyGapDiarizer, or EmbeddingDiarizer; \
             see clawft_channels::voice::diarization::SherpaDiarizer docs."
                .into(),
        ))
    }

    fn engine_name(&self) -> &'static str {
        "sherpa-placeholder"
    }
}

/// Preferred default backend for headless / CI: energy-gap clustering.
pub fn default_diarizer() -> Arc<dyn DiarizationBackend> {
    Arc::new(EnergyGapDiarizer::default())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build multi-speaker synthetic PCM: two alternating tone bursts with
    /// different amplitudes (stand-in for different speakers) separated by
    /// silence. Returns (samples, sample_rate, expected_min_speakers).
    fn synthetic_two_speaker_pcm() -> (Vec<i16>, u32) {
        let sr = 16_000u32;
        let mut samples = Vec::new();
        // Speaker A: loud 440 Hz-ish square, 800 ms
        for i in 0..(sr as usize * 800 / 1000) {
            let v = if (i / 18) % 2 == 0 { 12_000i16 } else { -12_000 };
            samples.push(v);
        }
        // Silence 400 ms
        samples.extend(std::iter::repeat_n(0i16, sr as usize * 400 / 1000));
        // Speaker B: quieter higher-rate square, 800 ms
        for i in 0..(sr as usize * 800 / 1000) {
            let v = if (i / 7) % 2 == 0 { 3_000i16 } else { -3_000 };
            samples.push(v);
        }
        // Silence 300 ms
        samples.extend(std::iter::repeat_n(0i16, sr as usize * 300 / 1000));
        // Speaker A again: loud, 600 ms
        for i in 0..(sr as usize * 600 / 1000) {
            let v = if (i / 18) % 2 == 0 { 11_000i16 } else { -11_000 };
            samples.push(v);
        }
        (samples, sr)
    }

    #[tokio::test]
    async fn fixture_two_speaker_labels_transcript() {
        let (pcm, sr) = synthetic_two_speaker_pcm();
        let total_ms = ((pcm.len() as u64 * 1000) / u64::from(sr)) as u32;
        let diar = FixtureDiarizer::two_speaker_split(total_ms);
        let result = diar.diarize(&pcm, sr).await.unwrap();
        assert_eq!(result.engine, "fixture");
        assert_eq!(result.num_speakers, 2);
        assert_eq!(result.segments.len(), 2);
        assert_eq!(result.segments[0].speaker, "spk-0");
        assert_eq!(result.segments[1].speaker, "spk-1");
        assert!(result.segments[0].end_ms <= result.segments[1].start_ms + 1);

        let labeled = label_transcript(
            "hello from alice and then bob replies clearly",
            result,
        );
        assert_eq!(labeled.turns.len(), 2);
        assert_eq!(labeled.turns[0].speaker, "spk-0");
        assert_eq!(labeled.turns[1].speaker, "spk-1");
        assert!(!labeled.turns[0].text.is_empty());
        assert!(!labeled.turns[1].text.is_empty());
        // Full text preserved.
        assert!(labeled.text.contains("alice"));
        assert!(labeled.text.contains("bob"));
    }

    #[tokio::test]
    async fn energy_gap_finds_multiple_islands_on_synthetic() {
        let (pcm, sr) = synthetic_two_speaker_pcm();
        let diar = EnergyGapDiarizer::default();
        let result = diar.diarize(&pcm, sr).await.unwrap();
        assert_eq!(result.engine, "energy-gap");
        // Three speech islands → at least 2 segments after clustering.
        assert!(
            result.segments.len() >= 2,
            "expected multi-island segments, got {:?}",
            result.segments
        );
        // Different amplitude/ZCR should yield more than one speaker label.
        assert!(
            result.num_speakers >= 2,
            "expected >=2 speakers on synthetic multi-talker, got {} ({:?})",
            result.num_speakers,
            result.labels()
        );
        // Segments cover non-zero duration and stay ordered.
        for w in result.segments.windows(2) {
            assert!(w[0].start_ms <= w[1].start_ms);
        }
        for s in &result.segments {
            assert!(s.duration_ms() > 0);
            assert!(s.confidence > 0.0);
        }
    }

    #[tokio::test]
    async fn energy_gap_silence_is_empty() {
        let diar = EnergyGapDiarizer::default();
        let result = diar.diarize(&vec![0i16; 16_000], 16_000).await.unwrap();
        assert_eq!(result.num_speakers, 0);
        assert!(result.segments.is_empty());
    }

    #[tokio::test]
    async fn sherpa_placeholder_errors_clearly() {
        let diar = SherpaDiarizer::new();
        let err = diar.diarize(&[1i16; 100], 16_000).await.unwrap_err();
        match err {
            VoiceError::Config(msg) => {
                assert!(msg.contains("WEFT-227"), "{msg}");
                assert!(msg.contains("not staged"), "{msg}");
            }
            other => panic!("expected Config, got {other:?}"),
        }
        assert_eq!(diar.engine_name(), "sherpa-placeholder");
    }

    /// Fake embedder: projects window mean |sample| + ZCR into a 4-d vector
    /// so loud/low-ZCR and quiet/high-ZCR windows land far apart.
    struct FakeEmbedder;

    #[async_trait]
    impl SpeakerEmbedder for FakeEmbedder {
        async fn embed(&self, audio: &[i16], _sample_rate: u32) -> Result<Vec<f32>, VoiceError> {
            let n = audio.len().max(1) as f32;
            let mean_abs = audio.iter().map(|&s| s.unsigned_abs() as f32).sum::<f32>() / n;
            let mut zc = 0u32;
            for w in audio.windows(2) {
                if (w[0] >= 0) != (w[1] >= 0) {
                    zc += 1;
                }
            }
            let zcr = zc as f32 / n;
            // Two roughly orthogonal axes so clustering separates A vs B.
            Ok(vec![mean_abs / 16_000.0, zcr, mean_abs / 32_000.0, 1.0 - zcr])
        }
        fn dim(&self) -> usize {
            4
        }
    }

    #[tokio::test]
    async fn embedding_cluster_multi_speaker_synthetic() {
        let (pcm, sr) = synthetic_two_speaker_pcm();
        let diar = EmbeddingDiarizer {
            window_ms: 400,
            hop_ms: 200,
            merge_threshold: 0.85,
            min_segment_ms: 150,
            embedder: Arc::new(FakeEmbedder),
        };
        let result = diar.diarize(&pcm, sr).await.unwrap();
        assert_eq!(result.engine, "embedding-cluster");
        assert!(
            result.num_speakers >= 2,
            "embedding cluster should separate synthetic speakers: {:?}",
            result
        );
        assert!(result.segments.len() >= 2);

        let labeled = label_transcript("alpha beta gamma delta epsilon", result.clone());
        assert!(!labeled.turns.is_empty());
        // Every turn speaker must appear in diarization labels.
        let labels = result.labels();
        for t in &labeled.turns {
            assert!(labels.contains(&t.speaker), "turn speaker {}", t.speaker);
        }
    }

    #[test]
    fn diarization_result_serde_roundtrip() {
        let r = DiarizationResult::from_segments(
            "fixture",
            vec![
                DiarizationSegment {
                    start_ms: 0,
                    end_ms: 500,
                    speaker: "spk-0".into(),
                    confidence: 0.9,
                },
                DiarizationSegment {
                    start_ms: 500,
                    end_ms: 1200,
                    speaker: "spk-1".into(),
                    confidence: 0.8,
                },
            ],
        );
        assert_eq!(r.num_speakers, 2);
        let json = serde_json::to_string(&r).unwrap();
        let back: DiarizationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn remap_labels_upgrades_to_enrolled_ids() {
        let mut r = DiarizationResult::from_segments(
            "fixture",
            vec![
                DiarizationSegment {
                    start_ms: 0,
                    end_ms: 100,
                    speaker: "spk-0".into(),
                    confidence: 1.0,
                },
                DiarizationSegment {
                    start_ms: 100,
                    end_ms: 200,
                    speaker: "spk-1".into(),
                    confidence: 1.0,
                },
            ],
        );
        let mut map = HashMap::new();
        map.insert("spk-0".into(), "spk-enrolled-alice".into());
        remap_labels(&mut r, &map);
        assert_eq!(r.segments[0].speaker, "spk-enrolled-alice");
        assert_eq!(r.segments[1].speaker, "spk-1");
        assert_eq!(r.num_speakers, 2);
    }

    #[test]
    fn label_transcript_single_segment_keeps_full_text() {
        let d = DiarizationResult::from_segments(
            "fixture",
            vec![DiarizationSegment {
                start_ms: 0,
                end_ms: 1000,
                speaker: "spk-0".into(),
                confidence: 1.0,
            }],
        );
        let labeled = label_transcript("the whole utterance", d);
        assert_eq!(labeled.turns.len(), 1);
        assert_eq!(labeled.turns[0].text, "the whole utterance");
    }

    #[tokio::test]
    async fn default_diarizer_is_energy_gap() {
        let d = default_diarizer();
        assert_eq!(d.engine_name(), "energy-gap");
        let (pcm, sr) = synthetic_two_speaker_pcm();
        let r = d.diarize(&pcm, sr).await.unwrap();
        assert!(!r.segments.is_empty());
    }
}
