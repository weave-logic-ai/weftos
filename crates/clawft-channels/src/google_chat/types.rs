//! Google Chat channel configuration types.

use serde::{Deserialize, Serialize};

/// Configuration for the Google Chat channel adapter.
///
/// Connects to Google Chat via the Workspace Chat REST API for outbound
/// messages and to a Cloud Pub/Sub subscription for inbound events
/// (the supported delivery mechanism for asynchronous Google Chat
/// events; see <https://developers.google.com/workspace/chat/events-overview>).
///
/// # Authentication
///
/// In 0.7.0 we accept a pre-issued OAuth2 access token via
/// [`bearer_token_env`](Self::bearer_token_env). This is the same model
/// used by GKE Workload Identity, Cloud Run, and `gcloud auth
/// print-access-token`-driven deployments. JWT-signed
/// service-account auth (RS256) requires additional crypto deps and is
/// tracked as a 0.8.x follow-up.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoogleChatAdapterConfig {
    /// Google Cloud project ID. Used for log/metric tagging and to
    /// validate that [`pubsub_subscription`](Self::pubsub_subscription)
    /// references the same project.
    #[serde(default, alias = "projectId")]
    pub project_id: String,

    /// Path to a service account JSON key file. Reserved for the 0.8.x
    /// JWT-signed flow; ignored in 0.7.0 in favour of
    /// [`bearer_token_env`](Self::bearer_token_env).
    #[serde(default, alias = "serviceAccountKeyPath")]
    pub service_account_key_path: String,

    /// Name of an environment variable containing a current OAuth2
    /// bearer access token. The adapter re-reads this env var on every
    /// HTTP call so external rotation (e.g. a sidecar refreshing a
    /// file-backed env or `gcloud auth application-default
    /// print-access-token` cron) is picked up automatically.
    ///
    /// Defaults to `GOOGLE_CHAT_ACCESS_TOKEN`.
    #[serde(default = "default_bearer_token_env", alias = "bearerTokenEnv")]
    pub bearer_token_env: String,

    /// Fully qualified Pub/Sub subscription, e.g.
    /// `projects/my-project/subscriptions/chat-events`. Required for
    /// inbound delivery. Empty disables inbound (send-only mode).
    #[serde(default, alias = "pubsubSubscription")]
    pub pubsub_subscription: String,

    /// Default Google Chat space to send to when `target` is omitted
    /// or unrecognised, e.g. `spaces/AAAAAA`. Optional.
    #[serde(default, alias = "defaultSpaceId")]
    pub default_space_id: String,

    /// Override base URL for the Chat REST API. Defaults to
    /// `https://chat.googleapis.com`. Tests point this at a local
    /// `wiremock` server.
    #[serde(default, alias = "chatBaseUrl")]
    pub chat_base_url: Option<String>,

    /// Override base URL for the Pub/Sub REST API. Defaults to
    /// `https://pubsub.googleapis.com`. Tests point this at a local
    /// `wiremock` server.
    #[serde(default, alias = "pubsubBaseUrl")]
    pub pubsub_base_url: Option<String>,

    /// Maximum messages per Pub/Sub `pull` call. Defaults to 10.
    #[serde(default = "default_pull_max", alias = "pullMaxMessages")]
    pub pull_max_messages: u32,

    /// Sleep (ms) between Pub/Sub pulls when the previous pull
    /// returned zero messages. Defaults to 1000ms.
    #[serde(default = "default_pull_idle_ms", alias = "pullIdleMs")]
    pub pull_idle_ms: u64,

    /// Spaces (rooms) to listen to. Empty = accept events from all
    /// spaces the bot is in.
    #[serde(default)]
    pub spaces: Vec<String>,

    /// Allowed user emails. Empty = allow all.
    #[serde(default, alias = "allowedUsers")]
    pub allowed_users: Vec<String>,
}

fn default_bearer_token_env() -> String {
    "GOOGLE_CHAT_ACCESS_TOKEN".to_string()
}

fn default_pull_max() -> u32 {
    10
}

fn default_pull_idle_ms() -> u64 {
    1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = GoogleChatAdapterConfig::default();
        assert!(cfg.project_id.is_empty());
        assert!(cfg.service_account_key_path.is_empty());
        assert!(cfg.spaces.is_empty());
        assert!(cfg.allowed_users.is_empty());
    }

    #[test]
    fn config_serde_roundtrip() {
        let json = r#"{
            "projectId": "my-project-123",
            "serviceAccountKeyPath": "/etc/keys/sa.json",
            "spaces": ["spaces/AAAA"],
            "allowedUsers": ["admin@company.com"]
        }"#;
        let cfg: GoogleChatAdapterConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.project_id, "my-project-123");
        assert_eq!(cfg.service_account_key_path, "/etc/keys/sa.json");
        assert_eq!(cfg.spaces, vec!["spaces/AAAA"]);
        assert_eq!(cfg.allowed_users, vec!["admin@company.com"]);
    }
}
