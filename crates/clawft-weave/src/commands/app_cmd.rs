//! `weaver app` — application management commands.

use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::protocol::Request;

#[derive(Args)]
pub struct AppArgs {
    #[command(subcommand)]
    pub command: AppCommand,
}

#[derive(Subcommand)]
pub enum AppCommand {
    /// Install application from a manifest.
    Install {
        /// Path to directory containing weftapp.toml (or JSON manifest).
        path: String,
    },
    /// Start an installed application.
    Start {
        /// Application name.
        name: String,
    },
    /// Stop a running application.
    Stop {
        /// Application name.
        name: String,
    },
    /// Remove an installed application.
    Remove {
        /// Application name.
        name: String,
    },
    /// List installed applications.
    List,
    /// Show application details.
    Inspect {
        /// Application name.
        name: String,
    },
}

pub async fn run(args: AppArgs) -> anyhow::Result<()> {
    let mut client = DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no daemon running — start with 'weaver kernel start'"))?;

    match args.command {
        AppCommand::Install { path } => {
            let params = serde_json::json!({"path": path});
            let resp = client
                .call(Request::with_params("app.install", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let name = resp
                .result
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            println!("Application installed: {name}");
        }
        AppCommand::Start { name } => {
            let params = serde_json::json!({"name": name});
            let resp = client
                .call(Request::with_params("app.start", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            println!("Application started: {name}");
        }
        AppCommand::Stop { name } => {
            let params = serde_json::json!({"name": name});
            let resp = client
                .call(Request::with_params("app.stop", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            println!("Application stopped: {name}");
        }
        AppCommand::Remove { name } => {
            let params = serde_json::json!({"name": name});
            let resp = client
                .call(Request::with_params("app.remove", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            println!("Application removed: {name}");
        }
        AppCommand::List => {
            let resp = client.simple_call("app.list").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let result = resp.result.unwrap_or_default();
            if let Some(apps) = result.as_array() {
                if apps.is_empty() {
                    println!("No applications installed");
                } else {
                    println!("{:<20} {:<12} VERSION", "NAME", "STATUS");
                    for app in apps {
                        println!(
                            "{:<20} {:<12} {}",
                            app.get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?"),
                            app.get("state")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?"),
                            app.get("version")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?"),
                        );
                    }
                }
            }
        }
        AppCommand::Inspect { name } => {
            let params = serde_json::json!({"name": name});
            let resp = client
                .call(Request::with_params("app.inspect", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            if let Some(result) = resp.result {
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }
    }
    Ok(())
}
