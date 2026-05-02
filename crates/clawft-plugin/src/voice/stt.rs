//! Speech-to-text via sherpa-rs streaming recognizer.

/// STT result from processing an audio segment.
#[derive(Debug, Clone)]
pub struct SttResult {
    /// Transcribed text.
    pub text: String,
    /// Whether this is a partial or final result.
    pub is_final: bool,
    /// Confidence score (0.0 - 1.0).
    pub confidence: f32,
}

/// Streaming speech-to-text engine.
///
/// Currently a stub -- real sherpa-rs integration is deferred to the
/// 0.8.x in-process voice backend (see ADR-053). 0.7.0 ships with
/// substrate-side STT via `clawft-service-whisper`.
pub struct SpeechToText {
    model_path: std::path::PathBuf,
    language: String,
}

impl SpeechToText {
    pub fn new(model_path: std::path::PathBuf, language: String) -> Self {
        Self { model_path, language }
    }

    /// Process audio samples and return transcription results.
    pub fn process(&mut self, _samples: &[f32]) -> Vec<SttResult> {
        // Stub: real sherpa-rs streaming recognition goes here
        vec![]
    }

    /// Finalize the current utterance and get the final result.
    pub fn finalize(&mut self) -> Option<SttResult> {
        // Stub
        None
    }

    /// Reset the recognizer state for a new utterance.
    pub fn reset(&mut self) {
        // Stub
    }

    pub fn model_path(&self) -> &std::path::Path {
        &self.model_path
    }

    pub fn language(&self) -> &str {
        &self.language
    }
}
