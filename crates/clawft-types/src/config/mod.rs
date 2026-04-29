//! Configuration schema types.
//!
//! A faithful port of `nanobot/config/schema.py`. All structs support
//! both `snake_case` and `camelCase` field names in JSON via `#[serde(alias)]`.
//! Unknown fields are silently ignored for forward compatibility.
//!
//! # Module Structure
//!
//! - [`channels`] -- Chat channel configurations (Telegram, Slack, Discord, etc.)
//! - [`policies`] -- Security policy configurations (command execution, URL safety)

pub mod channels;
pub mod kernel;
pub mod personality;
pub mod policies;
pub mod voice;

// Re-export channel types at the config level for backward compatibility.
pub use channels::*;
pub use kernel::*;
pub use personality::*;
pub use policies::*;
pub use voice::*;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::delegation::DelegationConfig;
use crate::routing::RoutingConfig;
use crate::secret::SecretString;

/// Shared default function: returns `true`.
pub(crate) fn default_true() -> bool {
    true
}

// ── Root config ──────────────────────────────────────────────────────────

/// Root configuration for the clawft framework.
///
/// Mirrors the Python `Config(BaseSettings)` class.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Agent defaults and per-agent overrides.
    #[serde(default)]
    pub agents: AgentsConfig,

    /// Chat channel configurations (Telegram, Slack, Discord, etc.).
    #[serde(default)]
    pub channels: ChannelsConfig,

    /// LLM provider credentials and settings.
    #[serde(default)]
    pub providers: ProvidersConfig,

    /// Gateway / HTTP server settings.
    #[serde(default)]
    pub gateway: GatewayConfig,

    /// Tool configurations (web search, exec, MCP servers).
    #[serde(default)]
    pub tools: ToolsConfig,

    /// Task delegation routing configuration.
    #[serde(default)]
    pub delegation: DelegationConfig,

    /// Tiered routing and permission configuration.
    #[serde(default)]
    pub routing: RoutingConfig,

    /// Voice pipeline configuration (STT, TTS, VAD, wake word).
    #[serde(default)]
    pub voice: VoiceConfig,

    /// Kernel subsystem configuration (WeftOS).
    #[serde(default)]
    pub kernel: KernelConfig,

    /// Pipeline stage selection (scorer, learner backends).
    #[serde(default)]
    pub pipeline: PipelineConfig,
}

// ── Pipeline ────────────────────────────────────────────────────────────

/// Pipeline stage backend selection.
///
/// Allows selecting which scorer and learner implementations to use.
/// Defaults to `"noop"` for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    /// Quality scorer backend: `"noop"` (default) or `"fitness"`.
    #[serde(default = "default_scorer")]
    pub scorer: String,

    /// Learning backend: `"noop"` (default) or `"trajectory"`.
    #[serde(default = "default_learner")]
    pub learner: String,
}

fn default_scorer() -> String {
    "noop".into()
}

fn default_learner() -> String {
    "noop".into()
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            scorer: default_scorer(),
            learner: default_learner(),
        }
    }
}

impl Config {
    /// Get the expanded workspace path.
    ///
    /// On native targets (with the `native` feature), this expands `~/` prefixes
    /// using `dirs::home_dir()`. On WASM or when `native` is disabled, `~/`
    /// prefixes are left unexpanded.
    pub fn workspace_path(&self) -> PathBuf {
        let raw = &self.agents.defaults.workspace;
        #[cfg(feature = "native")]
        if let Some(rest) = raw.strip_prefix("~/")
            && let Some(home) = dirs::home_dir()
        {
            return home.join(rest);
        }
        PathBuf::from(raw)
    }

    /// Get the expanded workspace path with an explicit home directory.
    ///
    /// This is the browser-friendly variant that does not depend on `dirs`.
    /// Pass `None` for `home` to skip `~/` expansion.
    pub fn workspace_path_with_home(&self, home: Option<&std::path::Path>) -> PathBuf {
        let raw = &self.agents.defaults.workspace;
        if let Some(rest) = raw.strip_prefix("~/")
            && let Some(home) = home
        {
            return home.join(rest);
        }
        PathBuf::from(raw)
    }
}

// ── Agents ───────────────────────────────────────────────────────────────

/// Agent configuration container.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    /// Default settings applied to all agents.
    #[serde(default)]
    pub defaults: AgentDefaults,

    /// Per-conversation cost circuit-breaker (WEFT-322).
    ///
    /// Caps the cumulative spend for a single `conv_id` so a confused
    /// agent loop on a permission prompt cannot burn the daily budget
    /// in one turn. The agent loop checks this BEFORE issuing each
    /// LLM call; on trip the conversation is marked `circuit_open` in
    /// the budget store and all subsequent calls fail-fast until
    /// reset via `agent.chat.reset_budget`.
    #[serde(default, alias = "costBudget")]
    pub cost_budget: CostBudgetConfig,
}

/// Per-conversation cost circuit-breaker config (WEFT-322).
///
/// The agent loop tracks cumulative tokens, USD spend, and iteration
/// count for each `conv_id`. When any cap is exceeded the conversation
/// is marked `circuit_open` and subsequent `agent.chat` calls return
/// [`ClawftError::ConversationBudgetExceeded`](crate::error::ClawftError::ConversationBudgetExceeded)
/// without invoking the LLM. The state survives daemon restarts via
/// the substrate-backed budget store at
/// `derived/chat/<conv_id>/budget.json`.
///
/// Defaults are sized for free-tier OpenRouter use:
/// 200 000 tokens, $1.00, 30 iterations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostBudgetConfig {
    /// Max cumulative input+output tokens per conversation. Default `200_000`.
    #[serde(default = "default_max_tokens_per_conv", alias = "maxTokensPerConv")]
    pub max_tokens_per_conv: u64,

    /// Max cumulative USD spend per conversation. Default `1.00`.
    #[serde(default = "default_max_usd_per_conv", alias = "maxUsdPerConv")]
    pub max_usd_per_conv: f64,

    /// Max cumulative LLM iterations (round-trips) per conversation.
    /// Default `30`. This counts every `pipeline.complete` call inside
    /// `run_tool_loop`, summed across every `handle_turn` for the conv.
    #[serde(default = "default_max_iterations_per_conv", alias = "maxIterationsPerConv")]
    pub max_iterations_per_conv: u32,
}

fn default_max_tokens_per_conv() -> u64 {
    200_000
}
fn default_max_usd_per_conv() -> f64 {
    1.00
}
fn default_max_iterations_per_conv() -> u32 {
    30
}

impl Default for CostBudgetConfig {
    fn default() -> Self {
        Self {
            max_tokens_per_conv: default_max_tokens_per_conv(),
            max_usd_per_conv: default_max_usd_per_conv(),
            max_iterations_per_conv: default_max_iterations_per_conv(),
        }
    }
}

/// Default agent settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefaults {
    /// Working directory for agent file operations.
    #[serde(default = "default_workspace")]
    pub workspace: String,

    /// Default LLM model identifier.
    #[serde(default = "default_model")]
    pub model: String,

    /// Maximum tokens in a single LLM response.
    #[serde(default = "default_max_tokens", alias = "maxTokens")]
    pub max_tokens: i32,

    /// Sampling temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f64,

    /// Maximum tool-use iterations per turn.
    #[serde(default = "default_max_tool_iterations", alias = "maxToolIterations")]
    pub max_tool_iterations: i32,

    /// Number of recent messages to include in context.
    #[serde(default = "default_memory_window", alias = "memoryWindow")]
    pub memory_window: i32,
}

fn default_workspace() -> String {
    "~/.nanobot/workspace".into()
}
fn default_model() -> String {
    "deepseek/deepseek-chat".into()
}
fn default_max_tokens() -> i32 {
    8192
}
fn default_temperature() -> f64 {
    0.7
}
fn default_max_tool_iterations() -> i32 {
    20
}
fn default_memory_window() -> i32 {
    50
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: default_workspace(),
            model: default_model(),
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            max_tool_iterations: default_max_tool_iterations(),
            memory_window: default_memory_window(),
        }
    }
}

// ── Providers ────────────────────────────────────────────────────────────

/// LLM provider credentials.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    /// API key for authentication.
    #[serde(default, alias = "apiKey")]
    pub api_key: SecretString,

    /// Base URL override (e.g. for proxies).
    #[serde(default, alias = "apiBase", alias = "baseUrl")]
    pub api_base: Option<String>,

    /// Custom HTTP headers (e.g. `APP-Code` for AiHubMix).
    #[serde(default, alias = "extraHeaders")]
    pub extra_headers: Option<HashMap<String, String>>,

    /// Whether this provider supports direct browser access (no CORS proxy needed).
    #[serde(default, alias = "browserDirect")]
    pub browser_direct: bool,

    /// CORS proxy URL for browser-mode API calls (e.g. "https://proxy.example.com").
    #[serde(default, alias = "corsProxy")]
    pub cors_proxy: Option<String>,
}

/// Configuration for all LLM providers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersConfig {
    /// Custom OpenAI-compatible endpoint.
    #[serde(default)]
    pub custom: ProviderConfig,

    /// Anthropic.
    #[serde(default)]
    pub anthropic: ProviderConfig,

    /// OpenAI.
    #[serde(default)]
    pub openai: ProviderConfig,

    /// OpenRouter gateway.
    #[serde(default)]
    pub openrouter: ProviderConfig,

    /// DeepSeek.
    #[serde(default)]
    pub deepseek: ProviderConfig,

    /// Groq.
    #[serde(default)]
    pub groq: ProviderConfig,

    /// Zhipu AI.
    #[serde(default)]
    pub zhipu: ProviderConfig,

    /// DashScope (Alibaba Cloud Qwen).
    #[serde(default)]
    pub dashscope: ProviderConfig,

    /// vLLM / local server.
    #[serde(default)]
    pub vllm: ProviderConfig,

    /// Google Gemini.
    #[serde(default)]
    pub gemini: ProviderConfig,

    /// Moonshot (Kimi).
    #[serde(default)]
    pub moonshot: ProviderConfig,

    /// MiniMax.
    #[serde(default)]
    pub minimax: ProviderConfig,

    /// AiHubMix gateway.
    #[serde(default)]
    pub aihubmix: ProviderConfig,

    /// OpenAI Codex (OAuth-based).
    #[serde(default)]
    pub openai_codex: ProviderConfig,

    /// xAI (Grok).
    #[serde(default)]
    pub xai: ProviderConfig,

    /// ElevenLabs (TTS).
    #[serde(default)]
    pub elevenlabs: ProviderConfig,
}

// ── Gateway ──────────────────────────────────────────────────────────────

/// Gateway / HTTP server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Bind address.
    #[serde(default = "default_gateway_host")]
    pub host: String,

    /// Listen port.
    #[serde(default = "default_gateway_port")]
    pub port: u16,

    /// Heartbeat interval in minutes (0 = disabled).
    #[serde(default, alias = "heartbeatIntervalMinutes")]
    pub heartbeat_interval_minutes: u64,

    /// Heartbeat prompt text.
    #[serde(default = "default_heartbeat_prompt", alias = "heartbeatPrompt")]
    pub heartbeat_prompt: String,

    /// Port for the UI REST API (separate from gateway port).
    #[serde(default = "default_api_port", alias = "apiPort")]
    pub api_port: u16,

    /// Allowed CORS origins for the UI API.
    #[serde(default = "default_cors_origins", alias = "corsOrigins")]
    pub cors_origins: Vec<String>,

    /// Whether the REST/WS API is enabled.
    #[serde(default, alias = "apiEnabled")]
    pub api_enabled: bool,
}

fn default_gateway_host() -> String {
    "0.0.0.0".into()
}
fn default_gateway_port() -> u16 {
    18790
}
fn default_heartbeat_prompt() -> String {
    "heartbeat".into()
}
fn default_api_port() -> u16 {
    18789
}
fn default_cors_origins() -> Vec<String> {
    vec!["http://localhost:5173".into()]
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: default_gateway_host(),
            port: default_gateway_port(),
            heartbeat_interval_minutes: 0,
            heartbeat_prompt: default_heartbeat_prompt(),
            api_port: default_api_port(),
            cors_origins: default_cors_origins(),
            api_enabled: false,
        }
    }
}

// ── Tools ────────────────────────────────────────────────────────────────

/// Tools configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsConfig {
    /// Web tools (search, etc.).
    #[serde(default)]
    pub web: WebToolsConfig,

    /// Shell exec tool settings.
    #[serde(default, rename = "exec")]
    pub exec_tool: ExecToolConfig,

    /// Whether to restrict all tool access to the workspace directory.
    #[serde(default, alias = "restrictToWorkspace")]
    pub restrict_to_workspace: bool,

    /// MCP server connections.
    #[serde(default, alias = "mcpServers")]
    pub mcp_servers: HashMap<String, MCPServerConfig>,

    /// Command execution policy (allowlist/denylist).
    #[serde(default, alias = "commandPolicy")]
    pub command_policy: CommandPolicyConfig,

    /// URL safety policy (SSRF protection).
    #[serde(default, alias = "urlPolicy")]
    pub url_policy: UrlPolicyConfig,

    /// Tools allowed by `weft mcp-server` over the wire.
    ///
    /// Each entry is a glob pattern (`*`, `?`). When the list is empty
    /// (default), all tools registered in the daemon are exposed —
    /// preserves prior behavior for upgrades. When non-empty, only
    /// matching tools are visible to MCP clients and other tools are
    /// rejected with `PermissionDenied` before execution.
    ///
    /// Used by [`PermissionFilter::from_patterns`] in
    /// `crates/clawft-services/src/mcp/middleware.rs` (WEFT-189).
    #[serde(default, alias = "allowedTools")]
    pub allowed_tools: Vec<String>,
}

/// Web tools configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebToolsConfig {
    /// Search engine settings.
    #[serde(default)]
    pub search: WebSearchConfig,
}

/// Web search tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchConfig {
    /// Search API key (e.g. Brave Search).
    #[serde(default, alias = "apiKey")]
    pub api_key: SecretString,

    /// Maximum number of search results.
    #[serde(default = "default_max_results", alias = "maxResults")]
    pub max_results: u32,
}

fn default_max_results() -> u32 {
    5
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            api_key: SecretString::default(),
            max_results: default_max_results(),
        }
    }
}

/// Shell exec tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecToolConfig {
    /// Command timeout in seconds.
    #[serde(default = "default_exec_timeout")]
    pub timeout: u32,
}

fn default_exec_timeout() -> u32 {
    60
}

impl Default for ExecToolConfig {
    fn default() -> Self {
        Self {
            timeout: default_exec_timeout(),
        }
    }
}

/// MCP server connection configuration (stdio or HTTP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerConfig {
    /// Command to run (for stdio transport, e.g. `"npx"`).
    #[serde(default)]
    pub command: String,

    /// Command arguments (for stdio transport).
    #[serde(default)]
    pub args: Vec<String>,

    /// Extra environment variables (for stdio transport).
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Streamable HTTP endpoint URL (for HTTP transport).
    #[serde(default)]
    pub url: String,

    /// If true, MCP session is created but tools are NOT registered in ToolRegistry.
    /// Infrastructure servers (claude-flow, claude-code) should be internal.
    #[serde(default = "default_true", alias = "internalOnly")]
    pub internal_only: bool,
}

impl Default for MCPServerConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: String::new(),
            internal_only: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Path to the test fixture config.
    const FIXTURE_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/config.json"
    );

    fn load_fixture() -> Config {
        let content =
            std::fs::read_to_string(FIXTURE_PATH).expect("fixture config.json should exist");
        serde_json::from_str(&content).expect("fixture should deserialize")
    }

    #[test]
    fn deserialize_fixture() {
        let cfg = load_fixture();

        // Agents
        assert_eq!(cfg.agents.defaults.model, "deepseek/deepseek-chat");
        assert_eq!(cfg.agents.defaults.max_tokens, 8192);
        assert_eq!(cfg.agents.defaults.temperature, 0.7);
        assert_eq!(cfg.agents.defaults.max_tool_iterations, 20);
        assert_eq!(cfg.agents.defaults.memory_window, 50);

        // Channels
        assert!(cfg.channels.telegram.enabled);
        assert_eq!(cfg.channels.telegram.token.expose(), "test-bot-token-123");
        assert_eq!(cfg.channels.telegram.allow_from, vec!["user1", "user2"]);
        assert!(!cfg.channels.slack.enabled);
        assert!(!cfg.channels.discord.enabled);

        // Providers
        assert_eq!(cfg.providers.anthropic.api_key.expose(), "sk-ant-test-key");
        assert_eq!(cfg.providers.openrouter.api_key.expose(), "sk-or-test-key");
        assert_eq!(
            cfg.providers.openrouter.api_base.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        assert!(cfg.providers.deepseek.api_key.is_empty());

        // Gateway
        assert_eq!(cfg.gateway.host, "0.0.0.0");
        assert_eq!(cfg.gateway.port, 18790);

        // Tools
        assert_eq!(cfg.tools.web.search.max_results, 5);
        assert_eq!(cfg.tools.exec_tool.timeout, 60);
        assert!(!cfg.tools.restrict_to_workspace);
        assert!(cfg.tools.mcp_servers.contains_key("test-server"));
        let mcp = &cfg.tools.mcp_servers["test-server"];
        assert_eq!(mcp.command, "npx");
        assert_eq!(mcp.args, vec!["-y", "test-mcp-server"]);
    }

    #[test]
    fn camel_case_aliases() {
        // The fixture uses camelCase (maxTokens, allowFrom, etc.)
        // This test is essentially the same as deserialize_fixture
        // but focuses on alias correctness.
        let cfg = load_fixture();
        assert_eq!(cfg.agents.defaults.max_tokens, 8192); // maxTokens
        assert_eq!(cfg.agents.defaults.max_tool_iterations, 20); // maxToolIterations
        assert_eq!(cfg.agents.defaults.memory_window, 50); // memoryWindow
        assert_eq!(cfg.channels.telegram.allow_from, vec!["user1", "user2"]); // allowFrom
    }

    #[test]
    fn default_values_for_missing_fields() {
        let json = r#"{}"#;
        let cfg: Config = serde_json::from_str(json).unwrap();

        // Agent defaults
        assert_eq!(cfg.agents.defaults.workspace, "~/.nanobot/workspace");
        assert_eq!(cfg.agents.defaults.model, "deepseek/deepseek-chat");
        assert_eq!(cfg.agents.defaults.max_tokens, 8192);
        assert!((cfg.agents.defaults.temperature - 0.7).abs() < f64::EPSILON);
        assert_eq!(cfg.agents.defaults.max_tool_iterations, 20);
        assert_eq!(cfg.agents.defaults.memory_window, 50);

        // Channel defaults
        assert!(!cfg.channels.telegram.enabled);
        assert!(cfg.channels.telegram.token.is_empty());
        assert!(!cfg.channels.slack.enabled);
        assert_eq!(cfg.channels.slack.mode, "socket");
        assert!(!cfg.channels.discord.enabled);
        assert_eq!(cfg.channels.discord.intents, 37377);

        // Gateway defaults
        assert_eq!(cfg.gateway.host, "0.0.0.0");
        assert_eq!(cfg.gateway.port, 18790);

        // Tool defaults
        assert_eq!(cfg.tools.exec_tool.timeout, 60);
        assert_eq!(cfg.tools.web.search.max_results, 5);
    }

    #[test]
    fn serde_roundtrip() {
        let cfg = load_fixture();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agents.defaults.model, cfg.agents.defaults.model);
        assert_eq!(restored.gateway.port, cfg.gateway.port);
        // SecretString serializes to "" for security, so after round-trip
        // the restored api_key is empty (by design).
        assert!(restored.providers.anthropic.api_key.is_empty());
    }

    #[test]
    fn unknown_fields_ignored() {
        let json = r#"{
            "agents": { "defaults": { "model": "test" } },
            "unknown_top_level": true,
            "channels": {
                "telegram": { "enabled": false, "some_future_field": 42 }
            },
            "providers": {
                "anthropic": { "apiKey": "k", "newField": "x" }
            }
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.agents.defaults.model, "test");
        assert!(!cfg.channels.telegram.enabled);
        assert_eq!(cfg.providers.anthropic.api_key.expose(), "k");
    }

    #[test]
    fn unknown_channel_plugins_in_extra() {
        let json = r#"{
            "channels": {
                "telegram": { "enabled": true },
                "my_custom_channel": { "url": "wss://custom.io" }
            }
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.channels.telegram.enabled);
        assert!(cfg.channels.extra.contains_key("my_custom_channel"));
    }

    #[test]
    fn workspace_path_expansion() {
        let mut cfg = Config::default();
        cfg.agents.defaults.workspace = "~/.clawft/workspace".into();
        let path = cfg.workspace_path();
        // Should not start with "~" after expansion
        assert!(!path.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn provider_config_with_extra_headers() {
        let json = r#"{
            "apiKey": "test",
            "extraHeaders": { "X-Custom": "value" }
        }"#;
        let cfg: ProviderConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.api_key.expose(), "test");
        let headers = cfg.extra_headers.unwrap();
        assert_eq!(headers["X-Custom"], "value");
    }

    #[test]
    fn email_config_defaults() {
        let cfg = EmailConfig::default();
        assert_eq!(cfg.imap_port, 993);
        assert!(cfg.imap_use_ssl);
        assert_eq!(cfg.smtp_port, 587);
        assert!(cfg.smtp_use_tls);
        assert!(!cfg.smtp_use_ssl);
        assert!(cfg.auto_reply_enabled);
        assert_eq!(cfg.poll_interval_seconds, 30);
        assert!(cfg.mark_seen);
        assert_eq!(cfg.max_body_chars, 12000);
        assert_eq!(cfg.subject_prefix, "Re: ");
    }

    #[test]
    fn mochat_config_defaults() {
        let cfg = MochatConfig::default();
        assert_eq!(cfg.base_url, "https://mochat.io");
        assert_eq!(cfg.socket_path, "/socket.io");
        assert_eq!(cfg.socket_reconnect_delay_ms, 1000);
        assert_eq!(cfg.socket_max_reconnect_delay_ms, 10000);
        assert_eq!(cfg.socket_connect_timeout_ms, 10000);
        assert_eq!(cfg.refresh_interval_ms, 30000);
        assert_eq!(cfg.watch_timeout_ms, 25000);
        assert_eq!(cfg.watch_limit, 100);
        assert_eq!(cfg.retry_delay_ms, 500);
        assert_eq!(cfg.max_retry_attempts, 0);
        assert_eq!(cfg.reply_delay_mode, "non-mention");
        assert_eq!(cfg.reply_delay_ms, 120000);
    }

    #[test]
    fn slack_dm_config_defaults() {
        let cfg = SlackDMConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.policy, "open");
    }

    #[test]
    fn gateway_heartbeat_defaults() {
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.heartbeat_interval_minutes, 0);
        assert_eq!(cfg.heartbeat_prompt, "heartbeat");
    }

    #[test]
    fn gateway_heartbeat_from_json() {
        let json = r#"{
            "host": "0.0.0.0",
            "port": 8080,
            "heartbeatIntervalMinutes": 15,
            "heartbeatPrompt": "status check"
        }"#;
        let cfg: GatewayConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.heartbeat_interval_minutes, 15);
        assert_eq!(cfg.heartbeat_prompt, "status check");
    }

    #[test]
    fn gateway_heartbeat_disabled_by_default() {
        let json = r#"{"host": "0.0.0.0"}"#;
        let cfg: GatewayConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.heartbeat_interval_minutes, 0);
        assert_eq!(cfg.heartbeat_prompt, "heartbeat");
    }

    #[test]
    fn mcp_server_config_roundtrip() {
        let cfg = MCPServerConfig {
            command: "npx".into(),
            args: vec!["-y".into(), "test-server".into()],
            env: {
                let mut m = HashMap::new();
                m.insert("API_KEY".into(), "secret".into());
                m
            },
            url: String::new(),
            internal_only: false,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: MCPServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.command, "npx");
        assert_eq!(restored.args.len(), 2);
        assert_eq!(restored.env["API_KEY"], "secret");
        assert!(!restored.internal_only);
    }

    #[test]
    fn command_policy_config_defaults() {
        let config: CommandPolicyConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.mode, "allowlist");
        assert!(config.allowlist.is_empty());
        assert!(config.denylist.is_empty());
    }

    #[test]
    fn command_policy_config_custom() {
        let json = r#"{"mode": "denylist", "allowlist": ["echo", "ls"], "denylist": ["rm -rf /"]}"#;
        let config: CommandPolicyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.mode, "denylist");
        assert_eq!(config.allowlist, vec!["echo", "ls"]);
        assert_eq!(config.denylist, vec!["rm -rf /"]);
    }

    #[test]
    fn url_policy_config_defaults() {
        let config: UrlPolicyConfig = serde_json::from_str("{}").unwrap();
        assert!(config.enabled);
        assert!(!config.allow_private);
        assert!(config.allowed_domains.is_empty());
        assert!(config.blocked_domains.is_empty());
    }

    #[test]
    fn url_policy_config_custom() {
        let json = r#"{"enabled": false, "allowPrivate": true, "allowedDomains": ["internal.corp"], "blockedDomains": ["evil.com"]}"#;
        let config: UrlPolicyConfig = serde_json::from_str(json).unwrap();
        assert!(!config.enabled);
        assert!(config.allow_private);
        assert_eq!(config.allowed_domains, vec!["internal.corp"]);
        assert_eq!(config.blocked_domains, vec!["evil.com"]);
    }

    #[test]
    fn tools_config_includes_policies() {
        let json = r#"{"commandPolicy": {"mode": "denylist"}, "urlPolicy": {"enabled": false}}"#;
        let config: ToolsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.command_policy.mode, "denylist");
        assert!(!config.url_policy.enabled);
    }

    #[test]
    fn tools_config_policies_default_when_absent() {
        let config: ToolsConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.command_policy.mode, "allowlist");
        assert!(config.url_policy.enabled);
    }

    // ── Step 0: Three-workstream config field tests ──────────────────────

    #[test]
    fn voice_config_defaults() {
        let cfg: VoiceConfig = serde_json::from_str("{}").unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.audio.sample_rate, 16000);
        assert_eq!(cfg.audio.chunk_size, 512);
        assert_eq!(cfg.audio.channels, 1);
        assert!(cfg.audio.input_device.is_none());
        assert!(cfg.audio.output_device.is_none());
        assert!(cfg.stt.enabled);
        assert_eq!(cfg.stt.model, "sherpa-onnx-streaming-zipformer-en-20M");
        assert!(cfg.stt.language.is_empty());
        assert!(cfg.tts.enabled);
        assert_eq!(cfg.tts.model, "vits-piper-en_US-amy-medium");
        assert!(cfg.tts.voice.is_empty());
        assert!((cfg.tts.speed - 1.0).abs() < f32::EPSILON);
        assert!((cfg.vad.threshold - 0.5).abs() < f32::EPSILON);
        assert_eq!(cfg.vad.silence_timeout_ms, 1500);
        assert_eq!(cfg.vad.min_speech_ms, 250);
        assert!(!cfg.wake.enabled);
        assert_eq!(cfg.wake.phrase, "hey weft");
        assert!((cfg.wake.sensitivity - 0.5).abs() < f32::EPSILON);
        assert!(cfg.wake.model_path.is_none());
        assert!(!cfg.cloud_fallback.enabled);
        assert!(cfg.cloud_fallback.stt_provider.is_empty());
        assert!(cfg.cloud_fallback.tts_provider.is_empty());
    }

    #[test]
    fn gateway_api_fields_defaults() {
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.api_port, 18789);
        assert_eq!(cfg.cors_origins, vec!["http://localhost:5173"]);
        assert!(!cfg.api_enabled);
    }

    #[test]
    fn provider_browser_fields_defaults() {
        let cfg: ProviderConfig = serde_json::from_str("{}").unwrap();
        assert!(!cfg.browser_direct);
        assert!(cfg.cors_proxy.is_none());
    }

    #[test]
    fn provider_base_url_alias() {
        let json = r#"{"baseUrl": "https://example.com"}"#;
        let cfg: ProviderConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.api_base.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn config_with_voice_section() {
        let json = r#"{"voice": {"enabled": true}}"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.voice.enabled);
        // Sub-structs should still be default
        assert_eq!(cfg.voice.audio.sample_rate, 16000);
        assert!(cfg.voice.stt.enabled);
    }

    #[test]
    fn config_with_all_new_fields() {
        let json = r#"{
            "voice": {
                "enabled": true,
                "audio": { "sampleRate": 48000, "chunkSize": 1024, "channels": 2 },
                "stt": { "model": "custom-stt", "language": "zh" },
                "tts": { "model": "custom-tts", "voice": "alloy", "speed": 1.5 },
                "vad": { "threshold": 0.8, "silenceTimeoutMs": 2000, "minSpeechMs": 500 },
                "wake": { "enabled": true, "phrase": "ok clawft", "sensitivity": 0.7 },
                "cloudFallback": { "enabled": true, "sttProvider": "whisper", "ttsProvider": "elevenlabs" }
            },
            "gateway": {
                "host": "127.0.0.1",
                "port": 9000,
                "apiPort": 9001,
                "corsOrigins": ["http://localhost:3000", "https://app.example.com"],
                "apiEnabled": true
            },
            "providers": {
                "openai": {
                    "apiKey": "sk-test",
                    "baseUrl": "https://api.openai.com/v1",
                    "browserDirect": true,
                    "corsProxy": "https://proxy.example.com"
                }
            }
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();

        // Voice
        assert!(cfg.voice.enabled);
        assert_eq!(cfg.voice.audio.sample_rate, 48000);
        assert_eq!(cfg.voice.audio.chunk_size, 1024);
        assert_eq!(cfg.voice.audio.channels, 2);
        assert_eq!(cfg.voice.stt.model, "custom-stt");
        assert_eq!(cfg.voice.stt.language, "zh");
        assert_eq!(cfg.voice.tts.model, "custom-tts");
        assert_eq!(cfg.voice.tts.voice, "alloy");
        assert!((cfg.voice.tts.speed - 1.5).abs() < f32::EPSILON);
        assert!((cfg.voice.vad.threshold - 0.8).abs() < f32::EPSILON);
        assert_eq!(cfg.voice.vad.silence_timeout_ms, 2000);
        assert_eq!(cfg.voice.vad.min_speech_ms, 500);
        assert!(cfg.voice.wake.enabled);
        assert_eq!(cfg.voice.wake.phrase, "ok clawft");
        assert!((cfg.voice.wake.sensitivity - 0.7).abs() < f32::EPSILON);
        assert!(cfg.voice.cloud_fallback.enabled);
        assert_eq!(cfg.voice.cloud_fallback.stt_provider, "whisper");
        assert_eq!(cfg.voice.cloud_fallback.tts_provider, "elevenlabs");

        // Gateway new fields
        assert_eq!(cfg.gateway.api_port, 9001);
        assert_eq!(
            cfg.gateway.cors_origins,
            vec!["http://localhost:3000", "https://app.example.com"]
        );
        assert!(cfg.gateway.api_enabled);

        // Provider browser fields
        assert!(cfg.providers.openai.browser_direct);
        assert_eq!(
            cfg.providers.openai.cors_proxy.as_deref(),
            Some("https://proxy.example.com")
        );
        assert_eq!(
            cfg.providers.openai.api_base.as_deref(),
            Some("https://api.openai.com/v1")
        );
    }
}
