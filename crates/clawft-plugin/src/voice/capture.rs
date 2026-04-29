//! Audio capture (microphone input) via cpal.
//!
//! Provides `AudioCapture` for streaming PCM audio from the default
//! input device at 16 kHz, 16-bit, mono.

/// Audio capture configuration.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub chunk_size: u32,
    pub device_name: Option<String>,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
            chunk_size: 512,
            device_name: None,
        }
    }
}

/// Microphone audio capture stream.
///
/// Wraps cpal input stream. Currently a stub -- real cpal integration
/// is deferred to the 0.8.x in-process voice backend (see ADR-053).
/// 0.7.0 ships with substrate-side capture + transcription, so this
/// scaffolding is a placeholder for the second `SttBackend` implementor.
pub struct AudioCapture {
    config: CaptureConfig,
    active: bool,
}

impl AudioCapture {
    /// Create a new audio capture with the given configuration.
    pub fn new(config: CaptureConfig) -> Self {
        Self { config, active: false }
    }

    /// Start capturing audio.
    pub fn start(&mut self) -> Result<(), String> {
        // Stub: real cpal stream creation deferred to 0.8.x in-process
        // backend (see ADR-053).
        self.active = true;
        tracing::info!(
            sample_rate = self.config.sample_rate,
            channels = self.config.channels,
            "Audio capture started (stub)"
        );
        Ok(())
    }

    /// Stop capturing audio.
    pub fn stop(&mut self) {
        self.active = false;
        tracing::info!("Audio capture stopped");
    }

    /// Check if capture is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get the capture configuration.
    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }
}
