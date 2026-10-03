//! `weft ui` -- start the web dashboard (gateway + API + browser).
//!
//! This command is a convenience wrapper around the gateway that:
//!
//! 1. Forces `gateway.api_enabled = true`
//! 2. Optionally overrides the API port
//! 3. Optionally serves a built frontend from a static directory
//! 4. Opens the browser automatically (unless `--no-open` is passed)

use clap::Args;
use tracing::info;

/// Start the web dashboard (gateway + API + browser).
#[derive(Args)]
pub struct UiArgs {
    /// Config file path (overrides auto-discovery).
    #[arg(short, long)]
    pub config: Option<String>,

    /// Port for the UI API (overrides config.gateway.api_port).
    #[arg(short, long)]
    pub port: Option<u16>,

    /// Don't open the browser automatically.
    #[arg(long)]
    pub no_open: bool,

    /// Directory containing the built UI (for static serving).
    #[arg(long)]
    pub ui_dir: Option<String>,
}

/// Run the `weft ui` command.
///
/// Loads configuration, forces the API to be enabled, applies any
/// port/static-dir overrides, then delegates to the gateway inner logic.
/// Optionally opens the browser after a short delay.
pub async fn run(args: UiArgs) -> anyhow::Result<()> {
    #[cfg(not(feature = "channels"))]
    {
        let _ = args;
        anyhow::bail!(
            "the ui command requires the 'channels' feature. \
             Rebuild with: cargo build -p clawft-cli --features channels"
        );
    }

    #[cfg(feature = "channels")]
    {
        let platform = std::sync::Arc::new(clawft_platform::NativePlatform::new());
        let loaded = super::load_config_layered(&*platform, args.config.as_deref()).await?;
        let mut config = loaded.config;

        // Force the API on -- that's the whole point of `weft ui`.
        config.gateway.api_enabled = true;

        // Apply port override if provided.
        if let Some(port) = args.port {
            config.gateway.api_port = port;
        }

        let port = config.gateway.api_port;
        let host = config.gateway.host.clone();
        let url = format!("http://{}:{}", host, port);

        info!(url = %url, "starting web dashboard");
        eprintln!("starting web dashboard at {url}");

        // The API needs a daemon-issued token (ADR-102). Print the link
        // rather than put the secret on a browser command line, where other
        // local users could read it from the process list.
        match super::token_cmd::issue_dashboard_link(&host, port).await {
            Ok((link, expires)) => {
                eprintln!("sign in (token expires {expires}): {link}");
                if plain_http_off_loopback(&link) {
                    eprintln!(
                        "warning: this link is plain http:// to a non-loopback host; \
                         anyone on the network path can read the token. Put TLS in front."
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "no dashboard token issued ({e}); start the daemon and run \
                     `weft token issue` for a sign-in link"
                );
            }
        }

        // Spawn a background task to open the browser after a short delay.
        if !args.no_open {
            let open_url = url.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                open_browser(&open_url);
            });
        }

        // Delegate to the gateway with the pre-loaded (mutated) config.
        // WEFT-10: pass split routing layers for PermissionResolver ceiling.
        // WEFT-493: pass config path so MCP hot-reload watcher can attach.
        let intelligent_routing = false;
        let config_watch_path = args
            .config
            .as_ref()
            .map(std::path::PathBuf::from)
            .or_else(|| super::discover_config_path(&*platform));
        super::gateway::run_with_config(
            config,
            intelligent_routing,
            args.ui_dir,
            Some(loaded.global_routing),
            loaded.workspace_routing,
            config_watch_path,
        )
        .await
    }
}

/// Is `link` plain `http://` to a host that is not loopback? Such a link
/// carries a full-power token across the network in the clear.
fn plain_http_off_loopback(link: &str) -> bool {
    let Some(rest) = link.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '#', '?']).next().unwrap_or(rest);
    let host = match authority.rsplit_once(':') {
        // `[::1]:18789` / `host:18789`; a bare IPv6 has no port to strip.
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    };
    clawft_services::mcp::classify_bind_host(host) == clawft_services::mcp::BindKind::Public
}

/// Attempt to open a URL in the user's default browser.
///
/// This is best-effort -- failures are silently ignored because the user
/// can always navigate manually.
fn open_browser(url: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", url])
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_only_for_plain_http_off_loopback() {
        for l in [
            "http://localhost:18789/#token=t",
            "http://127.0.0.1:18789/#token=t",
            "http://127.0.0.2:1/#token=t",
            "http://[::1]:18789/#token=t",
            "http://[::ffff:127.0.0.1]:18789/#token=t",
            "https://gw.example/#token=t",
        ] {
            assert!(!plain_http_off_loopback(l), "{l}");
        }
        for l in [
            "http://192.0.2.10:18789/#token=t",
            "http://gw.lan:18789/#token=t",
            "http://[2001:db8::1]:18789/#token=t",
        ] {
            assert!(plain_http_off_loopback(l), "{l}");
        }
    }

    #[test]
    fn ui_args_defaults() {
        let args = UiArgs {
            config: None,
            port: None,
            no_open: false,
            ui_dir: None,
        };
        assert!(args.config.is_none());
        assert!(args.port.is_none());
        assert!(!args.no_open);
        assert!(args.ui_dir.is_none());
    }

    #[test]
    fn ui_args_with_overrides() {
        let args = UiArgs {
            config: Some("/tmp/config.json".into()),
            port: Some(9000),
            no_open: true,
            ui_dir: Some("./clawft-ui/dist".into()),
        };
        assert_eq!(args.config.as_deref(), Some("/tmp/config.json"));
        assert_eq!(args.port, Some(9000));
        assert!(args.no_open);
        assert_eq!(args.ui_dir.as_deref(), Some("./clawft-ui/dist"));
    }

    #[test]
    fn open_browser_does_not_panic() {
        // Just verify the function doesn't panic with an invalid URL.
        // It's best-effort, so failures are fine.
        open_browser("http://localhost:99999");
    }
}
