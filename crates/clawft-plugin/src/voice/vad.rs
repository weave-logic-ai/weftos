//! Voice Activity Detection using Silero VAD.

/// VAD processing result.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum VadEvent {
    /// Speech started at this sample offset.
    SpeechStart { offset: usize },
    /// Speech ended at this sample offset.
    SpeechEnd { offset: usize },
    /// No speech detected in this frame.
    Silence,
}

/// Voice Activity Detector wrapping Silero VAD model.
///
/// Currently a stub -- real sherpa-rs integration is deferred to the
/// 0.8.x in-process voice backend (see ADR-053).
pub struct VoiceActivityDetector {
    threshold: f32,
    silence_timeout_ms: u32,
    active: bool,
}

impl VoiceActivityDetector {
    pub fn new(threshold: f32, silence_timeout_ms: u32) -> Self {
        Self {
            threshold,
            silence_timeout_ms,
            active: false,
        }
    }

    /// Process a chunk of audio samples.
    /// Returns VAD events detected in the chunk.
    pub fn process(&mut self, _samples: &[f32]) -> Vec<VadEvent> {
        // Stub: real Silero VAD inference goes here
        vec![VadEvent::Silence]
    }

    /// Reset the VAD state.
    pub fn reset(&mut self) {
        self.active = false;
    }

    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    pub fn silence_timeout_ms(&self) -> u32 {
        self.silence_timeout_ms
    }
}
