//! Multimodal turn content wire shapes (WEFT-350).
//!
//! Used by the agent-core chat path so substrate JSONL can record
//! text-only, audio-only, or mixed (transcript + substrate-pointed PCM)
//! turns without a schema reshape later. Audio bytes never live inline —
//! only [`AudioRef`] paths into substrate.

use serde::{Deserialize, Serialize};

/// Wire-shape for a per-turn record's content.
///
/// Externally-tagged JSON (`{"text": "..."}` / `{"audio": {...}}` /
/// `{"mixed": [...]}`). Internally-tagged would reject newtype variants
/// over primitives; untagged would be ambiguous.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnContent {
    /// Plain UTF-8 text — the default for every text chat turn.
    Text(String),
    /// A reference to substrate-resident audio. PCM bytes live at
    /// [`AudioRef::substrate_path`]; turn records never inline audio.
    Audio(AudioRef),
    /// Ordered mix of text and audio fragments (voice + transcript).
    Mixed(Vec<TurnContentPart>),
}

/// One fragment of a [`TurnContent::Mixed`] payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnContentPart {
    /// A text segment (usually the STT transcript).
    Text(String),
    /// An audio segment (substrate-pointed; see [`AudioRef`]).
    Audio(AudioRef),
}

/// Pointer to a substrate-resident audio asset.
///
/// The substrate path holds the actual PCM/encoded audio; this struct is
/// the lightweight reference recorded in conversation JSONL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioRef {
    /// Substrate path where the audio bytes live.
    pub substrate_path: String,
    /// MIME type of the encoded audio (e.g. `"audio/wav"`, `"audio/opus"`).
    pub mime: String,
    /// Audio duration in milliseconds.
    pub duration_ms: u64,
}

impl AudioRef {
    /// Build an audio ref from the three wire fields.
    pub fn new(
        substrate_path: impl Into<String>,
        mime: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            substrate_path: substrate_path.into(),
            mime: mime.into(),
            duration_ms,
        }
    }

    /// Parse from a JSON object with `substrate_path` / `mime` /
    /// `duration_ms` keys. Returns `None` when required fields are missing.
    pub fn from_value(v: &serde_json::Value) -> Option<Self> {
        let obj = v.as_object()?;
        let substrate_path = obj.get("substrate_path")?.as_str()?.to_string();
        let mime = obj
            .get("mime")
            .and_then(|m| m.as_str())
            .unwrap_or("audio/wav")
            .to_string();
        let duration_ms = obj
            .get("duration_ms")
            .and_then(|d| d.as_u64())
            .unwrap_or(0);
        Some(Self {
            substrate_path,
            mime,
            duration_ms,
        })
    }
}

impl TurnContent {
    /// Build the richest content shape for a text (+ optional audio) turn.
    ///
    /// | text empty | audio | result |
    /// |------------|-------|--------|
    /// | yes        | None  | `Text("")` |
    /// | no         | None  | `Text(text)` |
    /// | yes        | Some  | `Audio(audio)` |
    /// | no         | Some  | `Mixed([Text, Audio])` |
    pub fn from_text_and_audio(text: impl Into<String>, audio: Option<AudioRef>) -> Self {
        let text = text.into();
        match (text.is_empty(), audio) {
            (true, None) => Self::Text(String::new()),
            (false, None) => Self::Text(text),
            (true, Some(a)) => Self::Audio(a),
            (false, Some(a)) => Self::Mixed(vec![
                TurnContentPart::Text(text),
                TurnContentPart::Audio(a),
            ]),
        }
    }

    /// Wire discriminant: `"text"` / `"audio"` / `"mixed"`.
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Audio(_) => "audio",
            Self::Mixed(_) => "mixed",
        }
    }

    /// Text payload for LLM / display (transcript for audio/mixed).
    pub fn text_payload(&self) -> String {
        match self {
            Self::Text(t) => t.clone(),
            Self::Audio(_) => String::new(),
            Self::Mixed(parts) => {
                let mut out = String::new();
                for p in parts {
                    if let TurnContentPart::Text(t) = p {
                        out.push_str(t);
                    }
                }
                out
            }
        }
    }

    /// Audio ref if this content carries one (first audio in Mixed).
    pub fn audio_ref(&self) -> Option<&AudioRef> {
        match self {
            Self::Audio(a) => Some(a),
            Self::Mixed(parts) => parts.iter().find_map(|p| match p {
                TurnContentPart::Audio(a) => Some(a),
                TurnContentPart::Text(_) => None,
            }),
            Self::Text(_) => None,
        }
    }
}

/// Well-known `metadata` / message keys for the voice chat path (WEFT-350).
///
/// STT pipelines attach an [`AudioRef`] under these keys so
/// `agent.chat` / `agent.chat_stream` can persist `TurnContent::Audio`
/// or `Mixed` without a separate RPC.
pub mod voice_meta {
    /// Top-level `AgentChatParams.metadata` or message-level key holding
    /// an [`super::AudioRef`] object.
    pub const AUDIO: &str = "audio";
    /// Alternate: bare substrate path string (mime defaults to wav).
    pub const AUDIO_PATH: &str = "audio_substrate_path";
    /// MIME when using [`AUDIO_PATH`].
    pub const AUDIO_MIME: &str = "audio_mime";
    /// Duration ms when using [`AUDIO_PATH`].
    pub const AUDIO_DURATION_MS: &str = "audio_duration_ms";
    /// When true, the voice path already recorded the user turn (skip
    /// double-append). Same semantics as `user_turn_recorded`.
    pub const USER_TURN_RECORDED: &str = "user_turn_recorded";
}

/// Extract an [`AudioRef`] from free-form chat metadata.
///
/// Accepts either a nested `audio: { substrate_path, mime, duration_ms }`
/// object or the flattened `audio_substrate_path` + optional mime/duration
/// keys used by thin STT adapters.
pub fn audio_from_metadata(meta: &serde_json::Map<String, serde_json::Value>) -> Option<AudioRef> {
    if let Some(v) = meta.get(voice_meta::AUDIO)
        && let Some(a) = AudioRef::from_value(v)
    {
        return Some(a);
    }
    let path = meta.get(voice_meta::AUDIO_PATH)?.as_str()?;
    if path.is_empty() {
        return None;
    }
    let mime = meta
        .get(voice_meta::AUDIO_MIME)
        .and_then(|m| m.as_str())
        .unwrap_or("audio/wav")
        .to_string();
    let duration_ms = meta
        .get(voice_meta::AUDIO_DURATION_MS)
        .and_then(|d| d.as_u64())
        .unwrap_or(0);
    Some(AudioRef::new(path, mime, duration_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_text_and_audio_matrix() {
        assert_eq!(
            TurnContent::from_text_and_audio("hi", None),
            TurnContent::Text("hi".into())
        );
        let a = AudioRef::new("substrate/_derived/chat/c/audio/1", "audio/wav", 500);
        assert_eq!(
            TurnContent::from_text_and_audio("", Some(a.clone())),
            TurnContent::Audio(a.clone())
        );
        match TurnContent::from_text_and_audio("hi", Some(a.clone())) {
            TurnContent::Mixed(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0], TurnContentPart::Text("hi".into()));
                assert_eq!(parts[1], TurnContentPart::Audio(a));
            }
            other => panic!("expected Mixed, got {other:?}"),
        }
    }

    #[test]
    fn audio_from_metadata_nested_and_flat() {
        let mut nested = serde_json::Map::new();
        nested.insert(
            "audio".into(),
            serde_json::json!({
                "substrate_path": "substrate/_derived/chat/c/audio/0",
                "mime": "audio/opus",
                "duration_ms": 1200
            }),
        );
        let a = audio_from_metadata(&nested).unwrap();
        assert_eq!(a.mime, "audio/opus");
        assert_eq!(a.duration_ms, 1200);

        let mut flat = serde_json::Map::new();
        flat.insert(
            "audio_substrate_path".into(),
            serde_json::json!("substrate/_derived/chat/c/audio/9"),
        );
        flat.insert("audio_duration_ms".into(), serde_json::json!(300));
        let b = audio_from_metadata(&flat).unwrap();
        assert_eq!(b.substrate_path, "substrate/_derived/chat/c/audio/9");
        assert_eq!(b.mime, "audio/wav");
        assert_eq!(b.duration_ms, 300);
    }

    #[test]
    fn turn_content_serde_round_trips() {
        let mixed = TurnContent::Mixed(vec![
            TurnContentPart::Text("hello ".into()),
            TurnContentPart::Audio(AudioRef::new("p", "audio/wav", 10)),
        ]);
        let s = serde_json::to_string(&mixed).unwrap();
        let back: TurnContent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, mixed);
        assert_eq!(back.content_type(), "mixed");
        assert_eq!(back.text_payload(), "hello ");
    }
}
