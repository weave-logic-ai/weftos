//! Wake word detection for "Hey Weft" trigger phrase.
//!
//! Provides the `WakeWordDetector` that processes audio frames and
//! fires a detection event when the wake word is recognized.
//!
//! Currently a **stub implementation** -- real rustpotter integration
//! is deferred to the 0.8.x in-process voice backend (see ADR-053).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::error::PluginError;

/// Configuration for the wake word detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeWordConfig {
    /// Path to the wake word model file (.rpw).
    #[serde(default = "default_model_path")]
    pub model_path: PathBuf,

    /// Detection threshold (0.0-1.0). Lower = more sensitive.
    #[serde(default = "default_threshold")]
    pub threshold: f32,

    /// Minimum gap between detections in frames.
    #[serde(default = "default_min_gap")]
    pub min_gap_frames: usize,

    /// Audio sample rate.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Whether to log detection events.
    #[serde(default = "default_true")]
    pub log_detections: bool,
}

fn default_model_path() -> PathBuf {
    PathBuf::from("models/voice/wake/hey-weft.rpw")
}
fn default_threshold() -> f32 {
    0.5
}
fn default_min_gap() -> usize {
    30
}
fn default_sample_rate() -> u32 {
    16000
}
fn default_true() -> bool {
    true
}

impl Default for WakeWordConfig {
    fn default() -> Self {
        Self {
            model_path: default_model_path(),
            threshold: default_threshold(),
            min_gap_frames: default_min_gap(),
            sample_rate: default_sample_rate(),
            log_detections: default_true(),
        }
    }
}

/// Events emitted by the wake word detector.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WakeWordEvent {
    /// Wake word was detected with the given confidence.
    Detected {
        /// Detection confidence score (0.0-1.0).
        confidence: f32,
    },
    /// Detector started listening.
    Started,
    /// Detector stopped listening.
    Stopped,
    /// An error occurred during detection.
    Error {
        /// Error description.
        message: String,
    },
}

/// Wake word detector (STUB implementation).
///
/// Real implementation will use rustpotter for "Hey Weft" detection.
/// This stub provides the API surface for integration testing.
pub struct WakeWordDetector {
    config: WakeWordConfig,
    running: bool,
}

impl WakeWordDetector {
    /// Create a new wake word detector with the given configuration.
    pub fn new(config: WakeWordConfig) -> Result<Self, PluginError> {
        info!(
            model = %config.model_path.display(),
            threshold = config.threshold,
            "wake word detector created (stub)"
        );
        Ok(Self {
            config,
            running: false,
        })
    }

    /// Process a single audio frame. Returns `true` if wake word detected.
    ///
    /// STUB: Always returns `false`.
    pub fn process_frame(&mut self, _samples: &[i16]) -> bool {
        debug!("wake word: processing frame (stub, no detection)");
        false
    }

    /// Start listening for the wake word.
    pub fn start(&mut self) -> WakeWordEvent {
        self.running = true;
        info!("wake word detector started (stub)");
        WakeWordEvent::Started
    }

    /// Stop listening for the wake word.
    pub fn stop(&mut self) -> WakeWordEvent {
        self.running = false;
        info!("wake word detector stopped (stub)");
        WakeWordEvent::Stopped
    }

    /// Check if the detector is currently running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Get the current configuration.
    pub fn config(&self) -> &WakeWordConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_word_config_defaults() {
        let config = WakeWordConfig::default();
        assert_eq!(
            config.model_path,
            PathBuf::from("models/voice/wake/hey-weft.rpw")
        );
        assert!((config.threshold - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.min_gap_frames, 30);
        assert_eq!(config.sample_rate, 16000);
        assert!(config.log_detections);
    }

    #[test]
    fn wake_word_config_serde_roundtrip() {
        let config = WakeWordConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let restored: WakeWordConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.model_path, restored.model_path);
        assert!((config.threshold - restored.threshold).abs() < f32::EPSILON);
        assert_eq!(config.min_gap_frames, restored.min_gap_frames);
        assert_eq!(config.sample_rate, restored.sample_rate);
        assert_eq!(config.log_detections, restored.log_detections);
    }

    #[test]
    fn wake_word_config_custom_values() {
        let json = r#"{
            "model_path": "/custom/model.rpw",
            "threshold": 0.8,
            "min_gap_frames": 50,
            "sample_rate": 48000,
            "log_detections": false
        }"#;
        let config: WakeWordConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.model_path, PathBuf::from("/custom/model.rpw"));
        assert!((config.threshold - 0.8).abs() < f32::EPSILON);
        assert_eq!(config.min_gap_frames, 50);
        assert_eq!(config.sample_rate, 48000);
        assert!(!config.log_detections);
    }

    #[test]
    fn wake_word_detector_create() {
        let config = WakeWordConfig::default();
        let detector = WakeWordDetector::new(config).unwrap();
        assert!(!detector.is_running());
    }

    #[test]
    fn wake_word_detector_start_stop_lifecycle() {
        let config = WakeWordConfig::default();
        let mut detector = WakeWordDetector::new(config).unwrap();

        // Initially not running.
        assert!(!detector.is_running());

        // Start.
        let event = detector.start();
        assert!(matches!(event, WakeWordEvent::Started));
        assert!(detector.is_running());

        // Stop.
        let event = detector.stop();
        assert!(matches!(event, WakeWordEvent::Stopped));
        assert!(!detector.is_running());
    }

    #[test]
    fn wake_word_detector_process_frame_returns_false() {
        let config = WakeWordConfig::default();
        let mut detector = WakeWordDetector::new(config).unwrap();
        detector.start();

        let samples = vec![0i16; 512];
        assert!(!detector.process_frame(&samples));
    }

    #[test]
    fn wake_word_detector_config_accessor() {
        let config = WakeWordConfig {
            threshold: 0.75,
            ..Default::default()
        };
        let detector = WakeWordDetector::new(config).unwrap();
        assert!((detector.config().threshold - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn wake_word_event_serde_detected() {
        let event = WakeWordEvent::Detected { confidence: 0.95 };
        let json = serde_json::to_string(&event).unwrap();
        let restored: WakeWordEvent = serde_json::from_str(&json).unwrap();
        match restored {
            WakeWordEvent::Detected { confidence } => {
                assert!((confidence - 0.95).abs() < f32::EPSILON);
            }
            _ => panic!("expected Detected variant"),
        }
    }

    #[test]
    fn wake_word_event_serde_started() {
        let event = WakeWordEvent::Started;
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"started\""));
        let restored: WakeWordEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, WakeWordEvent::Started));
    }

    #[test]
    fn wake_word_event_serde_stopped() {
        let event = WakeWordEvent::Stopped;
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"stopped\""));
        let restored: WakeWordEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(restored, WakeWordEvent::Stopped));
    }

    #[test]
    fn wake_word_event_serde_error() {
        let event = WakeWordEvent::Error {
            message: "model not found".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: WakeWordEvent = serde_json::from_str(&json).unwrap();
        match restored {
            WakeWordEvent::Error { message } => {
                assert_eq!(message, "model not found");
            }
            _ => panic!("expected Error variant"),
        }
    }

    #[test]
    fn wake_word_event_all_variants_serialize() {
        let events = vec![
            WakeWordEvent::Detected { confidence: 0.5 },
            WakeWordEvent::Started,
            WakeWordEvent::Stopped,
            WakeWordEvent::Error {
                message: "test".into(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let _: WakeWordEvent = serde_json::from_str(&json).unwrap();
        }
    }
}
