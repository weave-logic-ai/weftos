//! Text-to-speech via sherpa-rs streaming synthesizer.

/// TTS synthesis result.
#[derive(Debug, Clone)]
pub struct TtsResult {
    /// Audio samples (f32, mono, at configured sample rate).
    pub samples: Vec<f32>,
    /// Sample rate of the output audio.
    pub sample_rate: u32,
}

/// Abort handle for cancelling TTS playback.
#[derive(Clone)]
pub struct TtsAbortHandle {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl TtsAbortHandle {
    pub fn new() -> Self {
        Self {
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for TtsAbortHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Streaming text-to-speech engine.
///
/// Currently a stub -- real sherpa-rs integration is deferred to the
/// 0.8.x in-process voice backend (see ADR-053).
pub struct TextToSpeech {
    model_path: std::path::PathBuf,
    voice: String,
    speed: f32,
}

impl TextToSpeech {
    pub fn new(model_path: std::path::PathBuf, voice: String, speed: f32) -> Self {
        Self { model_path, voice, speed }
    }

    /// Synthesize text to audio samples.
    pub fn synthesize(&self, _text: &str) -> Result<TtsResult, String> {
        // Stub: real sherpa-rs synthesis goes here
        Ok(TtsResult {
            samples: vec![],
            sample_rate: 16000,
        })
    }

    /// Synthesize with an abort handle for interruption support.
    pub fn synthesize_with_abort(
        &self,
        _text: &str,
        _abort: &TtsAbortHandle,
    ) -> Result<TtsResult, String> {
        // Stub
        Ok(TtsResult {
            samples: vec![],
            sample_rate: 16000,
        })
    }

    pub fn model_path(&self) -> &std::path::Path {
        &self.model_path
    }

    pub fn voice(&self) -> &str {
        &self.voice
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }
}
