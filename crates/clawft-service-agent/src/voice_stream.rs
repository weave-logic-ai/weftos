//! Voice ↔ streaming chat hooks (WEFT-350).
//!
//! Connects the product voice path to `agent.chat_stream`:
//!
//! ```text
//!   STT transcript (+ optional AudioRef)
//!        │
//!        ▼
//!   agent.chat_stream  ──► substrate/_derived/chat/<conv>/stream
//!        │                      (delta + speakable frames)
//!        ▼
//!   TTS consumes speakable units  ──► playback
//! ```
//!
//! This module does **not** own STT/TTS engines (those live in
//! `clawft-channels` / `clawft-voice-tts` / tools). It provides:
//!
//! 1. [`build_voice_chat_params`] — STT output → `AgentChatParams`
//! 2. [`SpeakableTracker`] re-export — frame consumer helpers
//! 3. [`tts_units_from_frames`] — extract ordered speakable strings

use clawft_types::agent_chat::{
    cascade_stream_frames, AgentChatMessage, AgentChatParams, AgentChatStreamFrame,
};
use clawft_types::turn_content::{audio_from_metadata, voice_meta, AudioRef, TurnContent};

pub use clawft_types::agent_chat::{
    cascade_stream_frames as cascade_frames, SpeakableTracker, StreamStep,
};

/// Build `agent.chat` / `agent.chat_stream` params from an STT result.
///
/// `transcript` is the recognized text (may be empty for pure-audio
/// archival turns). `audio` is the substrate path for the raw utterance
/// so the loop can persist [`TurnContent::Audio`] / `Mixed`.
pub fn build_voice_chat_params(
    conv_id: impl Into<String>,
    transcript: impl Into<String>,
    audio: Option<AudioRef>,
) -> AgentChatParams {
    let transcript = transcript.into();
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "provenance".into(),
        serde_json::json!({"impulse_source": "voice.stt", "path": "WEFT-350"}),
    );
    if let Some(ref a) = audio
        && let Ok(v) = serde_json::to_value(a) {
            metadata.insert(voice_meta::AUDIO.into(), v);
        }
    AgentChatParams {
        messages: vec![AgentChatMessage {
            role: "user".into(),
            content: transcript,
            audio,
        }],
        temperature: None,
        max_tokens: None,
        conv_id: conv_id.into(),
        caller_id: None,
        metadata: Some(metadata),
    }
}

/// Pull ordered speakable units from a cascade of stream frames.
///
/// Suitable for feeding a TTS engine sentence-by-sentence. Falls back
/// to the terminal frame's full text when no intermediate `speakable`
/// fields were set (older daemons / pure text clients).
pub fn tts_units_from_frames(frames: &[AgentChatStreamFrame]) -> Vec<String> {
    let mut units: Vec<String> = frames
        .iter()
        .filter_map(|f| f.speakable.clone())
        .filter(|s| !s.trim().is_empty())
        .collect();
    if units.is_empty()
        && let Some(last) = frames.iter().rev().find(|f| f.done && f.error.is_none()) {
            let t = last.text.trim();
            if !t.is_empty() {
                units.push(t.to_string());
            }
        }
    units
}

/// Resolve audio from chat params (message-level or metadata).
pub fn resolve_input_audio(params: &AgentChatParams) -> Option<AudioRef> {
    if let Some(a) = params
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.audio.clone())
    {
        return Some(a);
    }
    params
        .metadata
        .as_ref()
        .and_then(audio_from_metadata)
}

/// Build the [`TurnContent`] the substrate sink will persist for a
/// user voice turn.
pub fn user_turn_content(transcript: &str, audio: Option<AudioRef>) -> TurnContent {
    TurnContent::from_text_and_audio(transcript, audio)
}

/// Simulate a voice-friendly stream cascade for tests / offline TTS
/// dry-runs without a live daemon.
pub fn simulate_stream(assistant_text: &str) -> Vec<AgentChatStreamFrame> {
    cascade_stream_frames(assistant_text, 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_params_attaches_audio_and_transcript() {
        let audio = AudioRef::new("substrate/_derived/chat/c/audio/1", "audio/wav", 900);
        let p = build_voice_chat_params("conv-1", "hello world", Some(audio.clone()));
        assert_eq!(p.conv_id, "conv-1");
        assert_eq!(p.messages[0].content, "hello world");
        assert_eq!(p.messages[0].audio.as_ref().unwrap().duration_ms, 900);
        let resolved = resolve_input_audio(&p).unwrap();
        assert_eq!(resolved.substrate_path, audio.substrate_path);
        match user_turn_content("hello world", Some(audio)) {
            TurnContent::Mixed(parts) => assert_eq!(parts.len(), 2),
            other => panic!("expected Mixed, got {other:?}"),
        }
    }

    #[test]
    fn stream_cascade_emits_delta_and_speakable() {
        let frames = simulate_stream("Hello there. How are you?");
        assert!(frames.len() >= 2, "need progressive + done");
        assert!(frames.last().unwrap().done);
        // At least one progressive frame carries a delta.
        assert!(
            frames.iter().any(|f| f.delta.as_ref().is_some_and(|d| !d.is_empty())),
            "expected non-empty delta on some frame"
        );
        let units = tts_units_from_frames(&frames);
        assert!(
            !units.is_empty(),
            "TTS should receive at least one speakable unit"
        );
        // Joined speakable should reconstruct the answer roughly.
        let joined = units.join(" ");
        assert!(
            joined.contains("Hello") || joined.contains("How are you"),
            "units={units:?}"
        );
    }

    #[test]
    fn speakable_tracker_sentence_boundaries() {
        let mut t = SpeakableTracker::new();
        let s1 = t.push("Hello world");
        assert!(s1.speakable.is_none());
        assert_eq!(s1.delta, "Hello world");
        let s2 = t.push("Hello world. Next");
        assert_eq!(s2.speakable.as_deref(), Some("Hello world."));
        assert_eq!(s2.delta, ". Next");
        let tail = t.flush("Hello world. Next");
        assert_eq!(tail.as_deref(), Some("Next"));
    }

    #[test]
    fn tts_units_fallback_to_full_text() {
        // Frames without speakable (legacy) still yield the done text.
        let frames = vec![
            AgentChatStreamFrame::thinking(0),
            AgentChatStreamFrame::generating(1, "hi"),
            AgentChatStreamFrame::done_ok(2, "hi there"),
        ];
        let units = tts_units_from_frames(&frames);
        assert_eq!(units, vec!["hi there".to_string()]);
    }
}
