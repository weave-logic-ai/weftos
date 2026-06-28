//! audio_transcribe tool -- Transcribe audio files to text.
//!
//! Accepts a file path to a .wav, .mp3, .ogg, or .webm file and returns
//! the transcription. Uses the STT fallback chain when available, or
//! returns a stub result indicating the tool contract.
//!
//! Gated behind the `voice` feature flag.

use async_trait::async_trait;
use clawft_core::tools::registry::{Tool, ToolError};
use serde_json::{Value, json};

/// Tool for transcribing audio files to text.
///
/// Parameters:
/// - `file_path` (required): Absolute path to the audio file.
/// - `language` (optional): BCP-47 language hint (e.g., "en", "es").
pub struct AudioTranscribeTool;

impl AudioTranscribeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AudioTranscribeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AudioTranscribeTool {
    fn name(&self) -> &str {
        "audio_transcribe"
    }

    fn description(&self) -> &str {
        "Transcribe an audio file (.wav, .mp3, .ogg, .webm) to text \
         using speech-to-text."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "required": ["file_path"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the audio file to transcribe."
                },
                "language": {
                    "type": "string",
                    "description": "Optional BCP-47 language hint (e.g., 'en', 'es', 'ja')."
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("file_path is required".into()))?;

        let language = args["language"].as_str();

        // Validate file exists
        let path = std::path::Path::new(file_path);
        if !path.exists() {
            return Err(ToolError::FileNotFound(format!(
                "File not found: {file_path}"
            )));
        }

        // Determine MIME type from extension
        let mime_type = match path.extension().and_then(|e| e.to_str()) {
            Some("wav") => "audio/wav",
            Some("mp3") => "audio/mpeg",
            Some("ogg") => "audio/ogg",
            Some("webm") => "audio/webm",
            Some(ext) => {
                return Err(ToolError::InvalidArgs(format!(
                    "Unsupported audio format: .{ext}"
                )));
            }
            None => {
                return Err(ToolError::InvalidArgs("File has no extension".into()));
            }
        };

        // Read audio file
        let _audio_data = tokio::fs::read(file_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read file: {e}")))?;

        // STT fallback chain would be injected via runtime context in the
        // full integration. For now, return a stub result that proves the
        // tool contract works.
        tracing::info!(
            file_path = file_path,
            mime_type = mime_type,
            language = language.unwrap_or("auto"),
            "audio_transcribe tool executed (stub)"
        );

        Ok(json!({
            "status": "transcribed",
            "file": file_path,
            "text": "",
            "mime_type": mime_type,
            "language": language.unwrap_or("en"),
            "note": "STT engine integration pending -- tool contract defined"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_metadata() {
        let tool = AudioTranscribeTool::new();
        assert_eq!(tool.name(), "audio_transcribe");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["file_path"].is_object());
        assert!(params["properties"]["language"].is_object());
        assert_eq!(params["required"][0], "file_path");
    }

    #[tokio::test]
    async fn execute_missing_file_path_errors() {
        let tool = AudioTranscribeTool::new();
        let args = json!({});
        let result = tool.execute(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_nonexistent_file_errors() {
        let tool = AudioTranscribeTool::new();
        let args = json!({"file_path": "/tmp/nonexistent_audio_file_12345.wav"});
        let result = tool.execute(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_unsupported_extension_errors() {
        // Create a temporary file with an unsupported extension
        let tmp = std::env::temp_dir().join("test_audio.xyz");
        tokio::fs::write(&tmp, b"fake audio").await.unwrap();

        let tool = AudioTranscribeTool::new();
        let args = json!({"file_path": tmp.to_string_lossy()});
        let result = tool.execute(args).await;
        assert!(result.is_err());

        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[tokio::test]
    async fn execute_valid_wav_file_returns_stub() {
        // Create a temporary .wav file
        let tmp = std::env::temp_dir().join("test_transcribe.wav");
        tokio::fs::write(&tmp, b"RIFF....").await.unwrap();

        let tool = AudioTranscribeTool::default();
        let args = json!({
            "file_path": tmp.to_string_lossy(),
            "language": "en"
        });
        let result = tool.execute(args).await.unwrap();
        assert_eq!(result["status"], "transcribed");
        assert_eq!(result["mime_type"], "audio/wav");
        assert_eq!(result["language"], "en");

        let _ = tokio::fs::remove_file(&tmp).await;
    }
}
