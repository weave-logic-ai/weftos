//! Plugin-related runtime configuration (WEFT-556 / SC-10).
//!
//! Holds per-plugin operator grants the daemon needs at plugin load
//! time. Currently scoped to voice sub-permission grants; future
//! plugin-wide settings (e.g. install allowlist, network overrides)
//! will land here as well.
//!
//! Schema (under `~/.clawft/config.json`):
//!
//! ```json
//! {
//!   "plugins": {
//!     "voice_grants": {
//!       "com.example.transcribe": {
//!         "read_transcripts": true,
//!         "dispatch_commands": false,
//!         "synthesize_audio": true,
//!         "transcript_topics": ["weftos.voice.transcripts.v1"]
//!       }
//!     }
//!   }
//! }
//! ```
//!
//! The map key is the plugin id as it appears in the manifest. Plugins
//! without an entry receive no voice access — defaults are deny-by-
//! default.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Top-level plugin runtime configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginsConfig {
    /// Per-plugin voice sub-permission grants, keyed by plugin id
    /// (WEFT-556 / SC-10). See [`crate::config::plugins`] for schema.
    #[serde(default, alias = "voiceGrants")]
    pub voice_grants: HashMap<String, PluginVoiceGrant>,
}

/// Operator-approved voice sub-permissions for a single plugin.
///
/// Mirrors `clawft_plugin::VoiceGrants` but lives in `clawft-types` so
/// it can be parsed by the config layer without dragging the plugin
/// crate into clawft-types' dependency graph. The conversion at the
/// daemon edge is one-line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginVoiceGrant {
    /// Operator allows the plugin to receive voice transcripts.
    #[serde(default, alias = "readTranscripts")]
    pub read_transcripts: bool,

    /// Operator allows the plugin to dispatch commands derived from
    /// voice transcripts.
    #[serde(default, alias = "dispatchCommands")]
    pub dispatch_commands: bool,

    /// Operator allows the plugin to synthesize TTS audio.
    #[serde(default, alias = "synthesizeAudio")]
    pub synthesize_audio: bool,

    /// Topics the operator has whitelisted for this plugin. Empty =
    /// any topic the plugin requests is allowed (subject to
    /// `read_transcripts` still being true).
    #[serde(default, alias = "transcriptTopics")]
    pub transcript_topics: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_parses() {
        let cfg: PluginsConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.voice_grants.is_empty());
    }

    #[test]
    fn voice_grant_parses_snake_and_camel() {
        let json = r#"{
            "voice_grants": {
                "com.example.snake": {
                    "read_transcripts": true,
                    "synthesize_audio": true,
                    "transcript_topics": ["weftos.voice.transcripts.v1"]
                }
            }
        }"#;
        let cfg: PluginsConfig = serde_json::from_str(json).unwrap();
        let g = &cfg.voice_grants["com.example.snake"];
        assert!(g.read_transcripts);
        assert!(g.synthesize_audio);
        assert!(!g.dispatch_commands);
        assert_eq!(g.transcript_topics, vec!["weftos.voice.transcripts.v1"]);

        let json_camel = r#"{
            "voiceGrants": {
                "com.example.camel": {
                    "readTranscripts": true,
                    "dispatchCommands": true,
                    "transcriptTopics": ["t1", "t2"]
                }
            }
        }"#;
        let cfg: PluginsConfig = serde_json::from_str(json_camel).unwrap();
        let g = &cfg.voice_grants["com.example.camel"];
        assert!(g.read_transcripts);
        assert!(g.dispatch_commands);
        assert!(!g.synthesize_audio);
        assert_eq!(g.transcript_topics, vec!["t1", "t2"]);
    }

    #[test]
    fn missing_fields_default_to_deny() {
        let json = r#"{"voice_grants": {"com.example.minimal": {}}}"#;
        let cfg: PluginsConfig = serde_json::from_str(json).unwrap();
        let g = &cfg.voice_grants["com.example.minimal"];
        assert!(!g.read_transcripts);
        assert!(!g.dispatch_commands);
        assert!(!g.synthesize_audio);
        assert!(g.transcript_topics.is_empty());
    }

    #[test]
    fn voice_grant_serde_roundtrip() {
        let original = PluginVoiceGrant {
            read_transcripts: true,
            dispatch_commands: false,
            synthesize_audio: true,
            transcript_topics: vec!["topic.a".into(), "topic.b".into()],
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: PluginVoiceGrant = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, original);
    }
}
