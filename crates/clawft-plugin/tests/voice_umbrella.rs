//! WEFT-212: assert that the `voice` umbrella feature pulls in the
//! full STT + TTS path, not just VAD + wake.
//!
//! This is an integration test (separate compilation unit) so the
//! `cfg(feature = ...)` checks see the umbrella as the consumer would.
//! When `voice` is enabled, every subfeature must be enabled too —
//! the audit's complaint was that `cargo build --features voice` left
//! `voice-stt` / `voice-tts` orphaned, which we now reject at build
//! time via these assertions.

#![cfg(feature = "voice")]

#[test]
fn umbrella_enables_voice_stt() {
    // If `voice` is on but `voice-stt` is not, this branch fires and
    // the test fails with a clear message.
    #[cfg(not(feature = "voice-stt"))]
    panic!("voice umbrella feature is missing voice-stt (WEFT-212 regression)");

    // When all features are aligned, the STT module is reachable.
    #[cfg(feature = "voice-stt")]
    {
        // Touching the type forces it to compile under the umbrella.
        let _: Option<clawft_plugin::voice::stt::SpeechToText> = None;
    }
}

#[test]
fn umbrella_enables_voice_tts() {
    #[cfg(not(feature = "voice-tts"))]
    panic!("voice umbrella feature is missing voice-tts (WEFT-212 regression)");

    #[cfg(feature = "voice-tts")]
    {
        let _: Option<clawft_plugin::voice::tts::TextToSpeech> = None;
    }
}

#[test]
fn umbrella_enables_voice_vad() {
    #[cfg(not(feature = "voice-vad"))]
    panic!("voice umbrella feature is missing voice-vad");
}

#[test]
fn umbrella_enables_voice_wake() {
    #[cfg(not(feature = "voice-wake"))]
    panic!("voice umbrella feature is missing voice-wake");
}
