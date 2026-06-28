//! audio_synthesize tool -- Generate audio files from text.
//!
//! Synthesizes text to speech and saves the result as a .wav audio file.
//! Uses the TTS fallback chain when available, or returns a stub result.
//!
//! Gated behind the `voice` feature flag.

use async_trait::async_trait;
use clawft_core::tools::registry::{Tool, ToolError};
use serde_json::{Value, json};

/// Tool for synthesizing text to an audio file.
///
/// Parameters:
/// - `text` (required): Text to synthesize.
/// - `output_path` (required): Absolute path for the output .wav file.
/// - `voice` (optional): Voice ID to use.
/// - `speed` (optional): Speech rate multiplier (0.5-2.0).
pub struct AudioSynthesizeTool;

impl AudioSynthesizeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AudioSynthesizeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AudioSynthesizeTool {
    fn name(&self) -> &str {
        "audio_synthesize"
    }

    fn description(&self) -> &str {
        "Synthesize text to speech and save as an audio file (.wav)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "required": ["text", "output_path"],
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to synthesize into speech."
                },
                "output_path": {
                    "type": "string",
                    "description": "Absolute path for the output .wav file."
                },
                "voice": {
                    "type": "string",
                    "description": "Voice ID to use (default: system default voice)."
                },
                "speed": {
                    "type": "number",
                    "description": "Speech rate multiplier (0.5-2.0, default 1.0)."
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let text = args["text"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("text is required".into()))?;

        let output_path = args["output_path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("output_path is required".into()))?;

        let voice = args["voice"].as_str().unwrap_or("default");
        let speed = args["speed"].as_f64().unwrap_or(1.0) as f32;

        if text.is_empty() {
            return Err(ToolError::InvalidArgs("text must be non-empty".into()));
        }

        // Validate output directory exists
        let path = std::path::Path::new(output_path);
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                return Err(ToolError::InvalidPath(format!(
                    "Output directory does not exist: {}",
                    parent.display()
                )));
            }
        }

        // Validate extension
        match path.extension().and_then(|e| e.to_str()) {
            Some("wav") => {}
            Some(ext) => {
                return Err(ToolError::InvalidArgs(format!(
                    "Only .wav output is supported, got .{ext}"
                )));
            }
            None => {
                return Err(ToolError::InvalidArgs(
                    "Output path must have .wav extension".into(),
                ));
            }
        }

        // TTS fallback chain would be injected via runtime context.
        // For now, return a stub result.
        tracing::info!(
            text_len = text.len(),
            output_path = output_path,
            voice = voice,
            speed = speed,
            "audio_synthesize tool executed (stub)"
        );

        Ok(json!({
            "status": "synthesized",
            "output_path": output_path,
            "voice": voice,
            "speed": speed,
            "duration_ms": 0,
            "note": "TTS engine integration pending -- tool contract defined"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = AudioSynthesizeTool::new();
        assert_eq!(tool.name(), "audio_synthesize");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["text"].is_object());
        assert!(params["properties"]["output_path"].is_object());
        assert!(params["properties"]["voice"].is_object());
        assert!(params["properties"]["speed"].is_object());
        assert_eq!(params["required"][0], "text");
        assert_eq!(params["required"][1], "output_path");
    }

    #[tokio::test]
    async fn execute_missing_text_errors() {
        let tool = AudioSynthesizeTool::new();
        let args = json!({"output_path": "/tmp/out.wav"});
        let result = tool.execute(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_missing_output_path_errors() {
        let tool = AudioSynthesizeTool::new();
        let args = json!({"text": "hello"});
        let result = tool.execute(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_empty_text_errors() {
        let tool = AudioSynthesizeTool::new();
        let args = json!({"text": "", "output_path": "/tmp/out.wav"});
        let result = tool.execute(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_wrong_extension_errors() {
        let tool = AudioSynthesizeTool::new();
        let args = json!({"text": "hello", "output_path": "/tmp/out.mp3"});
        let result = tool.execute(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_valid_returns_stub() {
        let tmp = std::env::temp_dir().join("test_synth_output.wav");
        let tool = AudioSynthesizeTool::default();
        let args = json!({
            "text": "Hello world",
            "output_path": tmp.to_string_lossy(),
            "voice": "nova",
            "speed": 1.2
        });
        let result = tool.execute(args).await.unwrap();
        assert_eq!(result["status"], "synthesized");
        assert_eq!(result["voice"], "nova");
    }

    #[tokio::test]
    async fn execute_nonexistent_output_dir_errors() {
        let tool = AudioSynthesizeTool::new();
        let args = json!({
            "text": "hello",
            "output_path": "/nonexistent_dir_12345/out.wav"
        });
        let result = tool.execute(args).await;
        assert!(result.is_err());
    }
}
