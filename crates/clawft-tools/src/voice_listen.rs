//! voice_listen tool -- On-demand speech-to-text transcription.
//!
//! Captures audio from the microphone, runs VAD + STT, and returns
//! the transcribed text. Gated behind the `voice` feature flag.

use async_trait::async_trait;
use clawft_core::tools::registry::{Tool, ToolError};
use serde_json::{Value, json};

/// Tool that listens to microphone and returns transcribed text.
pub struct VoiceListenTool;

impl VoiceListenTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VoiceListenTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for VoiceListenTool {
    fn name(&self) -> &str {
        "voice_listen"
    }

    fn description(&self) -> &str {
        "Listen to the microphone and transcribe speech to text. \
         Returns the transcribed text when the user stops speaking."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "timeout_seconds": {
                    "type": "number",
                    "description": "Maximum time to listen in seconds (default: 30)",
                    "default": 30
                },
                "language": {
                    "type": "string",
                    "description": "Language code for STT (e.g., 'en', 'es'). Empty for auto-detect.",
                    "default": ""
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let timeout = args
            .get("timeout_seconds")
            .and_then(|v| v.as_f64())
            .unwrap_or(30.0);
        let language = args.get("language").and_then(|v| v.as_str()).unwrap_or("");

        // Stub: real implementation will use AudioCapture + VAD + STT
        tracing::info!(
            timeout = timeout,
            language = language,
            "voice_listen tool executed (stub)"
        );

        Ok(json!({
            "text": "",
            "confidence": 0.0,
            "language": language,
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
        let tool = VoiceListenTool::new();
        assert_eq!(tool.name(), "voice_listen");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["timeout_seconds"].is_object());
        assert!(params["properties"]["language"].is_object());
    }

    #[tokio::test]
    async fn execute_stub_returns_not_implemented() {
        let tool = VoiceListenTool::new();
        let args = serde_json::json!({"timeout_seconds": 10, "language": "en"});
        let result = tool.execute(args).await.expect("stub should not fail");
        assert_eq!(result["status"], "stub_not_implemented");
        assert_eq!(result["language"], "en");
        assert_eq!(result["text"], "");
    }

    #[tokio::test]
    async fn execute_stub_uses_defaults() {
        let tool = VoiceListenTool::default();
        let args = serde_json::json!({});
        let result = tool.execute(args).await.expect("stub should not fail");
        assert_eq!(result["status"], "stub_not_implemented");
        assert_eq!(result["duration_ms"], 0);
    }
}
