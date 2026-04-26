//! WeftOS audio-classification service.
//!
//! Sits between the ESP32 mic source (`substrate/<source-node>/sensor/mic/pcm_chunk`)
//! and the whisper STT Sink. Classifies each pcm_chunk window into a
//! tagged `{ class, confidence }` value and republishes under the
//! daemon's own prefix.
//!
//! # Why a Stage, not a sensor or a Sink
//!
//! Per `.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md` §R2, the
//! pipeline primitive splits three ways: Source → Stage → Sink. The
//! classifier is a **pure transform** — substrate in, substrate out,
//! same window cadence, no external dependency for the energy-based
//! impl. It is a Stage in the R2 sense; today we still publish its
//! output to substrate so existing GUIs can subscribe and the whisper
//! Sink can gate on it without a typed-channel runtime.
//!
//! # Why the emission shape is generic on purpose
//!
//! The user has explicitly scoped this work as audio *classification*,
//! not just voice-activity detection. See
//! `~/.claude/projects/.../project_vad_classifier_via_llamacpp.md`:
//!
//! > Use a tagged emission like `{ class: "speech" | "music" |
//! > "silence" | ..., confidence }` so a later swap from
//! > Silero-VAD-ONNX → llama.cpp-hosted classifier is a service
//! > substitution, not an interface change.
//!
//! `Classification::class` is therefore a `String`, not an enum. The
//! initial [`EnergyClassifier`] only emits `"speech"` / `"silence"`,
//! but a future llama.cpp-backed backend can emit `"music"`,
//! `"noise"`, `"silence"`, `"speech"`, or anything else without
//! breaking existing subscribers — the wire shape is stable.
//!
//! # Crate layout
//!
//! - [`classifier`] — the [`ClassifierBackend`] trait + the
//!   [`EnergyClassifier`] (RMS-threshold) impl + the [`Classification`]
//!   wire shape.
//! - [`service`]    — [`ClassifierService`], the substrate-connected
//!   pipeline that mirrors `clawft-service-whisper`'s shape.
//!
//! `classifier` is pure data + math (no async, no substrate); `service`
//! holds all the substrate plumbing.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod classifier;
pub mod service;

pub use classifier::{Classification, ClassifierBackend, EnergyClassifier};
pub use service::{ClassifierService, ClassifierServiceConfig};

/// Environment variable read by [`EnergyClassifier::from_env`].
///
/// Threshold in dBFS. Above → `"speech"`, below → `"silence"`. The
/// default of -45 dB is conservative for a 16 kHz INMP441 MEMS mic in
/// a quiet room; `-50` to `-40` is the practical band depending on
/// gain staging.
pub const VAD_RMS_THRESHOLD_DB_ENV: &str = "VAD_RMS_THRESHOLD_DB";

/// Default RMS threshold in dBFS. Below this, [`EnergyClassifier`]
/// emits `"silence"`; at-or-above, `"speech"`.
pub const DEFAULT_VAD_RMS_THRESHOLD_DB: f32 = -45.0;

/// Class string emitted by [`EnergyClassifier`] when the chunk is
/// at-or-above the threshold. A future backend may emit additional
/// strings; consumers should treat `class` as opaque except for this
/// well-known value.
pub const CLASS_SPEECH: &str = "speech";

/// Class string emitted by [`EnergyClassifier`] when the chunk is
/// below the threshold.
pub const CLASS_SILENCE: &str = "silence";
