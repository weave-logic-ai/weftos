//! voice_speak tool -- On-demand text-to-speech synthesis.
//!
//! Synthesizes text to speech and plays it through the speaker.
//! Gated behind the `voice` feature flag.

use async_trait::async_trait;
use clawft_core::tools::registry::{Tool, ToolError};
use serde_json::{Value, json};

/// Tool that speaks text through the speaker using TTS.
pub struct VoiceSpeakTool;

impl VoiceSpeakTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VoiceSpeakTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for VoiceSpeakTool {
    fn name(&self) -> &str {
        "voice_speak"
    }

    fn description(&self) -> &str {
        "Speak text aloud through the system speaker using text-to-speech synthesis."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to speak aloud"
                },
                "voice": {
                    "type": "string",
                    "description": "Voice ID to use (empty for default)",
                    "default": ""
                },
                "speed": {
                    "type": "number",
                    "description": "Speaking speed multiplier (1.0 = normal)",
                    "default": 1.0
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let voice = args.get("voice").and_then(|v| v.as_str()).unwrap_or("");
        let speed = args.get("speed").and_then(|v| v.as_f64()).unwrap_or(1.0);

        if text.is_empty() {
            return Err(ToolError::InvalidArgs(
                "\"text\" parameter is required and must be non-empty".into(),
            ));
        }

        // Stub: real implementation will use TextToSpeech + AudioPlayback
        tracing::info!(
            text_len = text.len(),
            voice = voice,
            speed = speed,
            "voice_speak tool executed (stub)"
        );

        Ok(json!({
            "spoken": true,
            "text_length": text.len(),
            "duration_ms": 0,
            "status": "stub_not_implemented"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = VoiceSpeakTool::new();
        assert_eq!(tool.name(), "voice_speak");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["text"].is_object());
        assert!(params["properties"]["voice"].is_object());
        assert!(params["properties"]["speed"].is_object());
        assert_eq!(params["required"][0], "text");
    }

    #[tokio::test]
    async fn execute_stub_with_text() {
        let tool = VoiceSpeakTool::new();
        let args = serde_json::json!({"text": "Hello world", "speed": 1.5});
        let result = tool.execute(args).await.expect("stub should not fail");
        assert_eq!(result["status"], "stub_not_implemented");
        assert_eq!(result["spoken"], true);
        assert_eq!(result["text_length"], 11);
    }

    #[tokio::test]
    async fn execute_stub_empty_text_errors() {
        let tool = VoiceSpeakTool::default();
        let args = serde_json::json!({"text": ""});
        let result = tool.execute(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_stub_missing_text_errors() {
        let tool = VoiceSpeakTool::new();
        let args = serde_json::json!({});
        let result = tool.execute(args).await;
        assert!(result.is_err());
    }
}
