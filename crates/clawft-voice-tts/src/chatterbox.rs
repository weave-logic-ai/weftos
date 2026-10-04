//! Chatterbox cloned-voice fast-tier scaffold (WEFT-613 / ADR-061 §4).
//!
//! The voicelab **david** profile uses Chatterbox with a cloned reference voice
//! (`am_onyx`, ~0.64 s TTFA) for the Speculative ack so the fast tier matches
//! slow-tier timbre. Upstream Chatterbox is Python; a native Rust/ONNX (or
//! sherpa-onnx voice-clone) port is **not shipped in 0.8** — full inference is
//! deferred to **0.9.x** (see `docs/plans/wave-ws10-WEFT-613-result.md`).
//!
//! This module is the honest 0.8 slice:
//!
//! 1. **Bundle discovery** — same idiom as [`KokoroTts`] (`$WEFTOS_CHATTERBOX_DIR`,
//!    `.weftos/models/chatterbox`, `$HOME/.weftos/models/chatterbox`).
//! 2. **Staging probe** — whether operators have laid down the expected files
//!    (ONNX + reference wav) for a future port.
//! 3. **Readiness gate** — [`ChatterboxTts::is_runtime_available`] is `true` only
//!    when both the bundle is staged **and** native inference is implemented
//!    ([`CHATTERBOX_INFERENCE_IMPLEMENTED`]). Today the constant is `false`.
//! 4. **[`TtsEngine`] seam** — tier is always Fast; synthesize fails with a
//!    clear `VoiceError` until inference lands (never fabricates audio).
//!
//! Prefer [`crate::fast_tier::build_native_fast_engine`] for production wiring:
//! it selects Chatterbox when ready, otherwise Kokoro (accepted 0.8 default).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use clawft_channels::voice::tts::{TtsChunk, TtsEngine, TtsTier};
use clawft_channels::voice::types::VoiceError;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Expected Chatterbox / clone-export sample rate (matches Kokoro / Orpheus PCM).
pub const CHATTERBOX_SAMPLE_RATE: u32 = 24_000;

/// Default voicelab clone voice id (david profile).
pub const DEFAULT_CLONE_VOICE_ID: &str = "am_onyx";

/// Bundle directory name under `.weftos/models/`.
pub const BUNDLE_DIR: &str = "chatterbox";

/// Env override for the bundle directory.
pub const MODEL_ENV: &str = "WEFTOS_CHATTERBOX_DIR";

/// Expected ONNX (or future Candle) model filename inside the bundle.
pub const MODEL_FILE: &str = "chatterbox.onnx";

/// Expected reference-wav filename for voice cloning (david: am_onyx).
pub const REFERENCE_WAV: &str = "reference.wav";

/// Optional voice-id marker file (plain text, e.g. `am_onyx`).
pub const VOICE_ID_FILE: &str = "voice_id.txt";

/// **False until a real native inference path ships (0.9.x).**
///
/// Flip this (and implement session load / tensor IO) only when an official or
/// pinned ONNX/Candle export is available. Staging files alone must not claim
/// runtime readiness.
pub const CHATTERBOX_INFERENCE_IMPLEMENTED: bool = false;

/// Scaffold for the cloned-voice fast tier. Construction never requires model
/// files; missing artifacts and unimplemented inference both surface at
/// synthesize time via a clear [`VoiceError`].
#[derive(Debug, Clone)]
pub struct ChatterboxTts {
    model_dir: PathBuf,
    voice_id: String,
    /// Bundle layout present on disk (model + reference).
    bundle_staged: bool,
    /// Real inference active — requires implemented port **and** staged bundle.
    runtime_available: bool,
}

impl ChatterboxTts {
    /// Auto-discover bundle: `$WEFTOS_CHATTERBOX_DIR` → cwd → `$HOME`.
    pub fn new() -> Self {
        Self::with_dir(default_model_dir())
    }

    /// Point at an explicit bundle directory. Never fails.
    pub fn with_dir(model_dir: impl Into<PathBuf>) -> Self {
        let model_dir = model_dir.into();
        let voice_id = load_voice_id(&model_dir).unwrap_or_else(|| DEFAULT_CLONE_VOICE_ID.into());
        let bundle_staged = probe_bundle_staged(&model_dir);
        let runtime_available = CHATTERBOX_INFERENCE_IMPLEMENTED && bundle_staged;
        Self {
            model_dir,
            voice_id,
            bundle_staged,
            runtime_available,
        }
    }

    /// Resolved bundle directory (may not exist).
    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    /// Clone voice id (default [`DEFAULT_CLONE_VOICE_ID`]).
    pub fn voice_id(&self) -> &str {
        &self.voice_id
    }

    /// True when the expected staging files are on disk (model + reference wav).
    /// Does **not** mean inference works — see [`Self::is_runtime_available`].
    pub fn is_bundle_staged(&self) -> bool {
        self.bundle_staged
    }

    /// True only when native clone inference can run (implemented + staged).
    pub fn is_runtime_available(&self) -> bool {
        self.runtime_available
    }

    /// Human-readable status for logs / `weft voice` diagnostics.
    pub fn status_summary(&self) -> String {
        if self.runtime_available {
            format!(
                "chatterbox clone ready (voice={}, dir={})",
                self.voice_id,
                self.model_dir.display()
            )
        } else if self.bundle_staged && !CHATTERBOX_INFERENCE_IMPLEMENTED {
            format!(
                "chatterbox bundle staged at {} but native inference not implemented (WEFT-613 → 0.9.x); using Kokoro fallback",
                self.model_dir.display()
            )
        } else if !self.bundle_staged {
            format!(
                "chatterbox bundle not staged (looked in {}); set {MODEL_ENV} or place {}+{} under .weftos/models/{BUNDLE_DIR}",
                self.model_dir.display(),
                MODEL_FILE,
                REFERENCE_WAV
            )
        } else {
            format!(
                "chatterbox unavailable (dir={})",
                self.model_dir.display()
            )
        }
    }
}

impl Default for ChatterboxTts {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TtsEngine for ChatterboxTts {
    async fn synthesize_stream(
        &self,
        text: &str,
        tx: mpsc::Sender<TtsChunk>,
        cancel: CancellationToken,
    ) -> Result<(), VoiceError> {
        let _ = (text, tx, cancel);
        if self.runtime_available {
            // Guard for the day inference lands: until then this branch is dead.
            // Intentionally not synthesizing placeholder PCM.
            return Err(VoiceError::Audio(
                "Chatterbox runtime flagged ready but synthesize is not wired (WEFT-613 bug)"
                    .into(),
            ));
        }
        Err(model_unavailable_error(self))
    }

    fn tier(&self) -> TtsTier {
        TtsTier::Fast
    }
}

/// True when `dir` has the minimum clone-bundle layout for future inference.
pub fn probe_bundle_staged(dir: &Path) -> bool {
    dir.join(MODEL_FILE).is_file() && dir.join(REFERENCE_WAV).is_file()
}

/// Resolve bundle dir: env → cwd `.weftos` → `$HOME/.weftos`.
pub fn default_model_dir() -> PathBuf {
    if let Ok(p) = std::env::var(MODEL_ENV) {
        return PathBuf::from(p);
    }
    let rel = PathBuf::from(".weftos/models").join(BUNDLE_DIR);
    if rel.exists() {
        return rel;
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".weftos/models")
            .join(BUNDLE_DIR);
    }
    rel
}

fn load_voice_id(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(VOICE_ID_FILE)).ok()?;
    let id = text.lines().next()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

fn model_unavailable_error(engine: &ChatterboxTts) -> VoiceError {
    VoiceError::Audio(format!(
        "Chatterbox cloned-voice TTS unavailable: {}. \
         Full native port is WEFT-613 (0.9.x). 0.8 accepts Kokoro fast + Orpheus slow \
         (timbre split). Stage {MODEL_FILE}+{REFERENCE_WAV} under {} and wait for \
         CHATTERBOX_INFERENCE_IMPLEMENTED.",
        engine.status_summary(),
        engine.model_dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction_never_requires_model() {
        let c = ChatterboxTts::with_dir("/nonexistent/chatterbox-weft613");
        assert!(!c.is_bundle_staged());
        assert!(!c.is_runtime_available());
        assert_eq!(c.tier(), TtsTier::Fast);
        assert_eq!(c.voice_id(), DEFAULT_CLONE_VOICE_ID);
    }

    #[test]
    fn inference_gate_is_off_in_0_8() {
        const {
            assert!(
                !CHATTERBOX_INFERENCE_IMPLEMENTED,
                "do not flip readiness without a real ONNX/Candle path"
            );
        }
    }

    #[test]
    fn probe_bundle_requires_model_and_reference() {
        let dir = std::env::temp_dir().join(format!(
            "clawft_chatterbox_probe_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!probe_bundle_staged(&dir));

        std::fs::write(dir.join(MODEL_FILE), b"not-a-real-onnx").unwrap();
        assert!(!probe_bundle_staged(&dir));

        std::fs::write(dir.join(REFERENCE_WAV), b"RIFF....").unwrap();
        assert!(probe_bundle_staged(&dir));

        let c = ChatterboxTts::with_dir(&dir);
        assert!(c.is_bundle_staged());
        // Staged but inference not implemented → not runtime-available.
        assert!(!c.is_runtime_available());
        assert!(c.status_summary().contains("not implemented") || c.status_summary().contains("0.9"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn voice_id_file_overrides_default() {
        let dir = std::env::temp_dir().join(format!(
            "clawft_chatterbox_voice_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(VOICE_ID_FILE), "custom_clone\n").unwrap();
        let c = ChatterboxTts::with_dir(&dir);
        assert_eq!(c.voice_id(), "custom_clone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn synthesize_without_runtime_errors_gracefully() {
        let c = ChatterboxTts::with_dir("/nonexistent/chatterbox-weft613");
        let (tx, _rx) = mpsc::channel::<TtsChunk>(4);
        let err = c
            .synthesize_stream("one sec.", tx, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, VoiceError::Audio(_)));
        let msg = err.to_string();
        assert!(msg.contains("Chatterbox") || msg.contains("chatterbox"));
        assert!(msg.contains("WEFT-613") || msg.contains("0.9"));
    }

    #[test]
    fn default_dir_honors_env() {
        // SAFETY: single-threaded test; restore afterwards.
        unsafe { std::env::set_var(MODEL_ENV, "/tmp/custom-chatterbox-weft613") };
        assert_eq!(
            default_model_dir(),
            PathBuf::from("/tmp/custom-chatterbox-weft613")
        );
        unsafe { std::env::remove_var(MODEL_ENV) };
    }
}
