//! Noise suppression for voice pipeline.
//!
//! Reduces background noise from mic input to improve STT accuracy.
//! Current implementation: stub (passthrough).

use serde::{Deserialize, Serialize};

/// Configuration for noise suppression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseSuppressorConfig {
    /// Enable noise suppression.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Suppression aggressiveness (0 = gentle, 3 = aggressive).
    #[serde(default = "default_aggressiveness")]
    pub aggressiveness: u8,
}

fn default_true() -> bool {
    true
}
fn default_aggressiveness() -> u8 {
    2
}

impl Default for NoiseSuppressorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            aggressiveness: 2,
        }
    }
}

/// Noise suppressor state.
pub struct NoiseSuppressor {
    config: NoiseSuppressorConfig,
    /// Estimated noise floor level.
    noise_floor: f32,
    /// Number of processed frames.
    frames_processed: u64,
}

impl NoiseSuppressor {
    /// Create a new noise suppressor with the given configuration.
    pub fn new(config: NoiseSuppressorConfig) -> Self {
        Self {
            config,
            noise_floor: 0.0,
            frames_processed: 0,
        }
    }

    /// Process audio frame, suppressing noise.
    /// Currently a passthrough stub.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.frames_processed += 1;
        if !self.config.enabled {
            return input.to_vec();
        }
        // Update noise floor estimate (simple exponential moving average)
        if !input.is_empty() {
            let rms = (input.iter().map(|s| s * s).sum::<f32>() / input.len() as f32).sqrt();
            self.noise_floor = 0.95 * self.noise_floor + 0.05 * rms;
        }
        // STUB: Real noise suppression would use spectral subtraction or RNNoise
        input.to_vec()
    }

    /// Get the current estimated noise floor.
    pub fn noise_floor(&self) -> f32 {
        self.noise_floor
    }

    /// Get number of frames processed.
    pub fn frames_processed(&self) -> u64 {
        self.frames_processed
    }

    /// Reset the noise suppressor state.
    pub fn reset(&mut self) {
        self.noise_floor = 0.0;
        self.frames_processed = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_with_defaults() {
        let ns = NoiseSuppressor::new(NoiseSuppressorConfig::default());
        assert_eq!(ns.frames_processed(), 0);
        assert!((ns.noise_floor() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn process_passthrough_preserves_input() {
        let mut ns = NoiseSuppressor::new(NoiseSuppressorConfig::default());
        let input = vec![0.1, 0.2, -0.3, 0.4, -0.5];
        let output = ns.process(&input);
        assert_eq!(input, output);
        assert_eq!(ns.frames_processed(), 1);
    }

    #[test]
    fn noise_floor_updates_on_process() {
        let mut ns = NoiseSuppressor::new(NoiseSuppressorConfig::default());
        // Process a frame of constant signal
        let input = vec![0.5; 160]; // 10ms at 16kHz
        ns.process(&input);

        // Noise floor should have moved from 0.0 toward 0.5
        let floor = ns.noise_floor();
        assert!(
            floor > 0.0,
            "Noise floor should be positive after processing"
        );
        assert!(
            floor < 0.5,
            "Noise floor should not have reached signal level yet"
        );

        // Process many frames to let noise floor converge
        for _ in 0..200 {
            ns.process(&input);
        }
        let floor = ns.noise_floor();
        assert!(
            (floor - 0.5).abs() < 0.05,
            "Noise floor should converge near 0.5, got {}",
            floor
        );
    }

    #[test]
    fn disabled_passthrough_no_noise_floor_update() {
        let config = NoiseSuppressorConfig {
            enabled: false,
            ..Default::default()
        };
        let mut ns = NoiseSuppressor::new(config);
        let input = vec![0.5; 160];
        let output = ns.process(&input);
        assert_eq!(input, output);
        assert!((ns.noise_floor() - 0.0).abs() < f32::EPSILON);
        assert_eq!(ns.frames_processed(), 1);
    }

    #[test]
    fn reset_clears_state() {
        let mut ns = NoiseSuppressor::new(NoiseSuppressorConfig::default());
        ns.process(&[0.5; 160]);
        ns.process(&[0.5; 160]);
        assert!(ns.noise_floor() > 0.0);
        assert_eq!(ns.frames_processed(), 2);

        ns.reset();
        assert!((ns.noise_floor() - 0.0).abs() < f32::EPSILON);
        assert_eq!(ns.frames_processed(), 0);
    }

    #[test]
    fn config_defaults_are_correct() {
        let config = NoiseSuppressorConfig::default();
        assert!(config.enabled);
        assert_eq!(config.aggressiveness, 2);
    }
}
