//! `weft status` -- show configuration status and diagnostics.
//!
//! Discovers the active configuration file, parses it, and displays
//! a summary of the current settings. With `--detailed`, also shows
//! channel and tool configuration.
//!
//! When a daemon is reachable, also surfaces live `agent.chat`
//! SystemService metrics (WEFT-333): last-completion time, in-flight
//! dispatch count, and per-conv lock contention.
//!
//! # Example
//!
//! ```text
//! weft status
//! weft status --detailed
//! ```

use clap::Args;
use serde::Deserialize;

use clawft_platform::NativePlatform;

use super::{discover_config_path, load_config};
use super::daemon_guard::warn_on_build_mismatch;

/// Arguments for the `weft status` subcommand.
#[derive(Args)]
pub struct StatusArgs {
    /// Show detailed configuration (channels, tools, providers).
    #[arg(long)]
    pub detailed: bool,
}

/// Wire subset of `kernel.services` → [`ServiceInfo`] (WEFT-333).
///
/// Only the fields we print; unknown extras are ignored so older and
/// newer daemons both deserialize cleanly.
#[derive(Debug, Deserialize)]
struct ServiceRow {
    name: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    health: String,
    #[serde(default)]
    detail: Option<String>,
}

/// Run the status command.
///
/// Discovers the config file, loads it, and prints a summary.
pub async fn run(args: StatusArgs) -> anyhow::Result<()> {
    let platform = NativePlatform::new();

    println!("weft status");
    println!("===========");
    println!();

    // Discover config path.
    let config_path = discover_config_path(&platform);

    match config_path {
        Some(ref path) => {
            println!("Config: {}", path.display());
        }
        None => {
            println!("Config: not found");
            println!("  Searched: ~/.clawft/config.json, ~/.nanobot/config.json");
            println!("  Set CLAWFT_CONFIG env var to override");
            println!();
            println!("Using defaults:");
        }
    }

    // Load and display config (defaults if no file found).
    let config = load_config(&platform, None).await?;

    println!();
    println!("Agent defaults:");
    println!("  Model:              {}", config.agents.defaults.model);
    println!("  Workspace:          {}", config.agents.defaults.workspace);
    println!(
        "  Max tokens:         {}",
        config.agents.defaults.max_tokens
    );
    println!(
        "  Temperature:        {}",
        config.agents.defaults.temperature
    );
    println!(
        "  Max tool iterations: {}",
        config.agents.defaults.max_tool_iterations
    );
    println!(
        "  Memory window:      {}",
        config.agents.defaults.memory_window
    );

    // WEFT-333: live agent.chat SystemService metrics from the daemon.
    print_agent_chat_status().await;

    // WEFT-336: router p50/p99 latency + fallback rate from the decision log.
    print_routing_metrics().await;

    if args.detailed {
        println!();
        println!("Gateway:");
        println!("  Listen: {}:{}", config.gateway.host, config.gateway.port);

        println!();
        println!("Channels:");
        print_channel_status(
            "  telegram",
            config.channels.telegram.enabled,
            !config.channels.telegram.token.is_empty(),
        );
        print_channel_status(
            "  slack",
            config.channels.slack.enabled,
            !config.channels.slack.bot_token.is_empty(),
        );
        print_channel_status(
            "  discord",
            config.channels.discord.enabled,
            !config.channels.discord.token.is_empty(),
        );
        print_channel_status(
            "  whatsapp",
            config.channels.whatsapp.enabled,
            !config.channels.whatsapp.bridge_token.is_empty(),
        );
        print_channel_status(
            "  email",
            config.channels.email.enabled,
            !config.channels.email.imap_host.is_empty(),
        );
        print_channel_status(
            "  feishu",
            config.channels.feishu.enabled,
            !config.channels.feishu.app_id.is_empty(),
        );
        print_channel_status(
            "  dingtalk",
            config.channels.dingtalk.enabled,
            !config.channels.dingtalk.client_id.is_empty(),
        );

        println!();
        println!("Providers:");
        print_provider("  anthropic", config.providers.anthropic.api_key.expose());
        print_provider("  openai", config.providers.openai.api_key.expose());
        print_provider("  openrouter", config.providers.openrouter.api_key.expose());
        print_provider("  deepseek", config.providers.deepseek.api_key.expose());
        print_provider("  groq", config.providers.groq.api_key.expose());
        print_provider("  gemini", config.providers.gemini.api_key.expose());
        print_provider("  custom", config.providers.custom.api_key.expose());

        println!();
        println!("Tools:");
        println!(
            "  Restrict to workspace: {}",
            config.tools.restrict_to_workspace
        );
        println!(
            "  Exec timeout:         {}s",
            config.tools.exec_tool.timeout
        );
        println!(
            "  Search max results:   {}",
            config.tools.web.search.max_results
        );

        if !config.tools.mcp_servers.is_empty() {
            println!();
            println!("  MCP servers:");
            for (name, mcp) in &config.tools.mcp_servers {
                if !mcp.command.is_empty() {
                    println!("    {name}: {} {}", mcp.command, mcp.args.join(" "));
                } else if !mcp.url.is_empty() {
                    println!("    {name}: {}", mcp.url);
                } else {
                    println!("    {name}: (unconfigured)");
                }
            }
        }
    }

    println!();
    Ok(())
}

/// Best-effort: query the daemon's `kernel.services` for the
/// `agent.chat` SystemService and print its metrics (WEFT-333).
///
/// Silent when no daemon is reachable or the service is not
/// registered (pre-WEFT-333 daemons). Never fails the command.
async fn print_agent_chat_status() {
    let Some(mut client) = crate::commands::daemon_conn::connect_opt().await else {
        println!();
        println!("agent.chat:");
        println!("  Daemon: not running (start with `weft daemon` / `weaver daemon`)");
        return;
    };
    warn_on_build_mismatch(&mut client).await;

    let Ok(resp) = client.simple_call("kernel.services").await else {
        println!();
        println!("agent.chat:");
        println!("  Daemon: connected, but kernel.services RPC failed");
        return;
    };
    let Ok(value) = resp.into_result() else {
        println!();
        println!("agent.chat:");
        println!("  Daemon: connected, kernel.services returned error");
        return;
    };
    let rows: Vec<ServiceRow> = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(_) => {
            println!();
            println!("agent.chat:");
            println!("  Daemon: connected, could not parse kernel.services");
            return;
        }
    };

    println!();
    println!("agent.chat:");
    match rows.into_iter().find(|r| r.name == "agent.chat") {
        Some(row) => {
            let state = if row.state.is_empty() {
                "unknown"
            } else {
                row.state.as_str()
            };
            let health = if row.health.is_empty() {
                "unknown"
            } else {
                row.health.as_str()
            };
            println!("  State:              {state}");
            println!("  Health:             {health}");
            // Prefer the structured detail field; fall back to parsing
            // metrics out of the health string for older wire shapes.
            let detail = row
                .detail
                .filter(|d| !d.is_empty())
                .or_else(|| extract_metrics_from_health(health));
            if let Some(detail) = detail {
                print_agent_chat_detail(&detail);
            } else {
                println!("  Last completion:    (no detail)");
                println!("  In-flight:          (no detail)");
                println!("  Lock contentions:   (no detail)");
            }
        }
        None => {
            println!("  Status:             not registered");
            println!("  (SystemService lands at AgentService init — WEFT-333)");
        }
    }
}

/// Parse `last_completion=… in_flight=… lock_contentions=…` (and
/// optional `lock_wait_ms=…`) from a health_detail string and print
/// labelled lines.
fn print_agent_chat_detail(detail: &str) {
    let last = kv_from_detail(detail, "last_completion").unwrap_or_else(|| "unknown".into());
    let in_flight = kv_from_detail(detail, "in_flight").unwrap_or_else(|| "unknown".into());
    let contentions =
        kv_from_detail(detail, "lock_contentions").unwrap_or_else(|| "unknown".into());
    let wait_ms = kv_from_detail(detail, "lock_wait_ms");

    println!("  Last completion:    {last}");
    println!("  In-flight:          {in_flight}");
    println!("  Lock contentions:   {contentions}");
    if let Some(ms) = wait_ms {
        println!("  Lock wait (ms):     {ms}");
    }
}

/// Pull `key=value` tokens from a space-separated detail string.
fn kv_from_detail(detail: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    detail.split_whitespace().find_map(|tok| {
        tok.strip_prefix(&prefix).map(|v| v.to_string())
    })
}

/// Best-effort: if an older health label embedded metrics, recover them.
fn extract_metrics_from_health(health: &str) -> Option<String> {
    if health.contains("last_completion=") && health.contains("in_flight=") {
        Some(health.to_string())
    } else {
        None
    }
}

/// Best-effort: load last-100 routing decisions and print p50/p99 +
/// fallback rate (WEFT-336). Silent when the daemon is offline or the
/// log is empty — never fails `weft status`.
async fn print_routing_metrics() {
    println!();
    println!("context router:");
    match super::routing_cmd::try_load_status_metrics().await {
        Some(m) if m.count > 0 => {
            println!("  Decisions (window): {}", m.count);
            if let Some(p50) = m.p50_latency_ms {
                println!("  p50 latency:        {p50} ms");
            }
            if let Some(p99) = m.p99_latency_ms {
                println!("  p99 latency:        {p99} ms");
            }
            println!(
                "  fallback rate:      {:.1}% ({}/{})",
                m.fallback_rate * 100.0,
                m.fallback_count,
                m.count
            );
            println!("  (see also: weft routing trace / weft routing replay)");
        }
        Some(_) => {
            println!("  Decisions (window): 0");
            println!("  (no entries under substrate/_derived/agent/routing/recent)");
        }
        None => {
            println!("  Daemon: not reachable (or substrate.list unavailable)");
            println!("  (start daemon, or use `weft routing trace --from-file`)");
        }
    }
}

/// Print channel status: name, enabled/disabled, configured/unconfigured.
fn print_channel_status(label: &str, enabled: bool, has_credentials: bool) {
    let status = match (enabled, has_credentials) {
        (true, true) => "enabled, configured",
        (true, false) => "enabled, NOT configured (missing credentials)",
        (false, true) => "disabled (credentials present)",
        (false, false) => "disabled",
    };
    println!("{label}: {status}");
}

/// Print provider status, masking the API key.
fn print_provider(label: &str, api_key: &str) {
    if api_key.is_empty() {
        println!("{label}: not configured");
    } else {
        let masked = mask_key(api_key);
        println!("{label}: {masked}");
    }
}

/// Mask an API key, showing only the first 4 and last 4 characters.
fn mask_key(key: &str) -> String {
    if key.len() <= 8 {
        return "*".repeat(key.len());
    }
    let prefix = &key[..4];
    let suffix = &key[key.len() - 4..];
    let middle_len = key.len() - 8;
    format!("{prefix}{}...{suffix}", "*".repeat(middle_len.min(8)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_args_defaults() {
        let args = StatusArgs { detailed: false };
        assert!(!args.detailed);
    }

    #[test]
    fn mask_key_short() {
        assert_eq!(mask_key("abc"), "***");
        assert_eq!(mask_key("abcdefgh"), "********");
    }

    #[test]
    fn mask_key_long() {
        let masked = mask_key("sk-ant-test-key-12345");
        assert!(masked.starts_with("sk-a"));
        assert!(masked.ends_with("2345"));
        assert!(masked.contains("*"));
        assert!(masked.contains("..."));
    }

    #[test]
    fn mask_key_medium() {
        let masked = mask_key("123456789");
        assert_eq!(masked, "1234*...6789");
    }

    #[test]
    fn print_channel_status_does_not_panic() {
        // Smoke test for all combinations.
        print_channel_status("test", true, true);
        print_channel_status("test", true, false);
        print_channel_status("test", false, true);
        print_channel_status("test", false, false);
    }

    #[test]
    fn print_provider_does_not_panic() {
        print_provider("test", "");
        print_provider("test", "sk-test-key-12345678");
    }

    #[test]
    fn kv_from_detail_parses_tokens() {
        let d = "last_completion=never in_flight=2 lock_contentions=3 lock_wait_ms=12";
        assert_eq!(kv_from_detail(d, "last_completion").as_deref(), Some("never"));
        assert_eq!(kv_from_detail(d, "in_flight").as_deref(), Some("2"));
        assert_eq!(kv_from_detail(d, "lock_contentions").as_deref(), Some("3"));
        assert_eq!(kv_from_detail(d, "lock_wait_ms").as_deref(), Some("12"));
        assert_eq!(kv_from_detail(d, "missing"), None);
    }

    #[test]
    fn extract_metrics_from_health_requires_keys() {
        assert!(extract_metrics_from_health(
            "last_completion=never in_flight=0 lock_contentions=0"
        )
        .is_some());
        assert!(extract_metrics_from_health("healthy").is_none());
    }

    #[test]
    fn print_agent_chat_detail_does_not_panic() {
        print_agent_chat_detail(
            "last_completion=never in_flight=0 lock_contentions=0 lock_wait_ms=0",
        );
    }

    #[test]
    fn service_row_deserializes_with_and_without_detail() {
        let with = serde_json::json!({
            "name": "agent.chat",
            "state": "running",
            "health": "healthy",
            "detail": "last_completion=never in_flight=0 lock_contentions=0 lock_wait_ms=0"
        });
        let row: ServiceRow = serde_json::from_value(with).unwrap();
        assert_eq!(row.name, "agent.chat");
        assert!(row.detail.unwrap().contains("in_flight=0"));

        let without = serde_json::json!({
            "name": "llm",
            "state": "running",
            "health": "healthy"
        });
        let row: ServiceRow = serde_json::from_value(without).unwrap();
        assert!(row.detail.is_none());
    }
}
