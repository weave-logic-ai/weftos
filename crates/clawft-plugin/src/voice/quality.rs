//! Audio quality monitoring for voice pipeline.
//!
//! Provides real-time metrics: RMS level, peak level, clipping detection,
//! signal-to-noise ratio estimation.

/// Audio quality metrics for a single frame.
#[derive(Debug, Clone, Default)]
pub struct AudioMetrics {
    /// Root mean square level (0.0 - 1.0 for normalized audio).
    pub rms_level: f32,
    /// Peak absolute sample value.
    pub peak_level: f32,
    /// Whether clipping was detected (samples at +/-1.0).
    pub clipping_detected: bool,
    /// Number of clipped samples in frame.
    pub clipped_sample_count: usize,
    /// Estimated signal-to-noise ratio in dB (higher is better).
    pub estimated_snr_db: f32,
}

/// Computes audio quality metrics for a frame of samples.
///
/// # Arguments
/// * `samples` - Audio samples (typically normalized to -1.0..1.0)
/// * `noise_floor` - Estimated noise floor level from NoiseSuppressor
///
/// # Returns
/// `AudioMetrics` with computed values for the frame.
pub fn analyze_frame(samples: &[f32], noise_floor: f32) -> AudioMetrics {
    if samples.is_empty() {
        return AudioMetrics::default();
    }

    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

    let clipping_threshold = 0.99;
    let clipped = samples
        .iter()
        .filter(|s| s.abs() >= clipping_threshold)
        .count();

    let snr = if noise_floor > 0.0001 {
        20.0 * (rms / noise_floor).log10()
    } else {
        60.0 // Assume good SNR if no noise
    };

    AudioMetrics {
        rms_level: rms,
        peak_level: peak,
        clipping_detected: clipped > 0,
        clipped_sample_count: clipped,
        estimated_snr_db: snr.clamp(-20.0, 80.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_metrics() {
        let silence = vec![0.0f32; 160];
        let metrics = analyze_frame(&silence, 0.0);
        assert!((metrics.rms_level - 0.0).abs() < f32::EPSILON);
        assert!((metrics.peak_level - 0.0).abs() < f32::EPSILON);
        assert!(!metrics.clipping_detected);
        assert_eq!(metrics.clipped_sample_count, 0);
        // With zero noise floor, SNR defaults to 60
        assert!((metrics.estimated_snr_db - 60.0).abs() < f32::EPSILON);
    }

    #[test]
    fn sine_wave_metrics() {
        // Generate a simple sine wave at ~1kHz for 10ms at 16kHz
        let samples: Vec<f32> = (0..160)
            .map(|i| {
                let t = i as f32 / 16000.0;
                (2.0 * std::f32::consts::PI * 1000.0 * t).sin() * 0.5
            })
            .collect();

        let metrics = analyze_frame(&samples, 0.01);

        // RMS of a 0.5 amplitude sine wave = 0.5 / sqrt(2) ~ 0.354
        assert!(
            (metrics.rms_level - 0.354).abs() < 0.02,
            "RMS should be ~0.354, got {}",
            metrics.rms_level
        );
        // Peak should be close to 0.5
        assert!(
            (metrics.peak_level - 0.5).abs() < 0.01,
            "Peak should be ~0.5, got {}",
            metrics.peak_level
        );
        assert!(!metrics.clipping_detected);
        // SNR should be positive (signal is much louder than noise floor)
        assert!(metrics.estimated_snr_db > 20.0);
    }

    #[test]
    fn clipping_detection() {
        let mut samples = vec![0.5f32; 160];
        // Add some clipped samples
        samples[0] = 1.0;
        samples[1] = -1.0;
        samples[2] = 0.995;

        let metrics = analyze_frame(&samples, 0.0);
        assert!(metrics.clipping_detected);
        assert_eq!(metrics.clipped_sample_count, 3);
    }

    #[test]
    fn snr_estimation() {
        // Signal at 0.5 RMS with noise floor at 0.01
        let samples = vec![0.5f32; 160];
        let metrics = analyze_frame(&samples, 0.01);

        // SNR = 20 * log10(0.5 / 0.01) = 20 * log10(50) ~ 34 dB
        assert!(
            (metrics.estimated_snr_db - 34.0).abs() < 1.0,
            "SNR should be ~34 dB, got {}",
            metrics.estimated_snr_db
        );
    }

    #[test]
    fn empty_frame_returns_defaults() {
        let metrics = analyze_frame(&[], 0.0);
        assert!((metrics.rms_level - 0.0).abs() < f32::EPSILON);
        assert!((metrics.peak_level - 0.0).abs() < f32::EPSILON);
        assert!(!metrics.clipping_detected);
        assert_eq!(metrics.clipped_sample_count, 0);
        assert!((metrics.estimated_snr_db - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn snr_clamped_to_range() {
        // Very low signal with high noise floor -> low SNR, should clamp at -20
        let samples = vec![0.0001f32; 160];
        let metrics = analyze_frame(&samples, 1.0);
        assert!(
            metrics.estimated_snr_db >= -20.0,
            "SNR should be clamped at -20, got {}",
            metrics.estimated_snr_db
        );
    }
}
