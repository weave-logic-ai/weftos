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
        let mut config = super::load_config(&*platform, args.config.as_deref()).await?;

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

        // Spawn a background task to open the browser after a short delay.
        if !args.no_open {
            let open_url = url.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                open_browser(&open_url);
            });
        }

        // Delegate to the gateway with the pre-loaded (mutated) config.
        let intelligent_routing = false;
        super::gateway::run_with_config(config, intelligent_routing, args.ui_dir).await
    }
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
