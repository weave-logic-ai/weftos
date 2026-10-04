//! `weft token` -- issue, revoke and list daemon bearer tokens (ADR-102 D3).
//!
//! The daemon is the token authority: these commands call its
//! `auth.token.*` RPCs over the local socket, which is the proof of being
//! the local owner. A daemon that is down is a clear error, not a
//! fallback. The secret is printed once and never stored.
//!
//! ```text
//! weft token issue [--ttl 15m] [--label playground] [--project <ulid>] [--read-only]
//! weft token revoke <id>
//! weft token list
//! ```

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use clawft_rpc::resolve::{ResolveFlags, resolve};
use clawft_rpc::{DaemonClient, Request};
use serde_json::{Value, json};

/// Arguments for `weft token`.
#[derive(Args)]
pub struct TokenArgs {
    #[command(subcommand)]
    pub action: TokenAction,
}

/// `weft token` subcommands.
#[derive(Subcommand)]
pub enum TokenAction {
    /// Issue a token (shown once). Default lifetime 15 minutes, maximum 24 hours.
    Issue {
        /// Lifetime, e.g. `90s`, `15m`, `2h` (a bare number is seconds).
        #[arg(long, default_value = "15m", value_parser = parse_ttl)]
        ttl: u64,

        /// Label recorded with the token.
        #[arg(long, default_value = "playground")]
        label: String,

        /// Scope the token to a project (ULID). The daemon treats it as the
        /// request's project and refuses a request that names another
        /// (`project_scope_mismatch`). It is a claim guard, not a capability
        /// limit: the token still carries owner scope.
        #[arg(long)]
        project: Option<String>,

        /// Mint a read-only token: it opens only the gateway's GET read routes
        /// (`/api/fleet/snapshot`, `/api/health`, ...) and every other route
        /// answers 403. Use it for a console. Cannot be combined with `--project`.
        #[arg(long, conflicts_with = "project")]
        read_only: bool,

        /// Config file override (used to find the gateway address).
        #[arg(long)]
        config: Option<String>,
    },

    /// Revoke a token by id.
    ///
    /// The daemon refuses the token at once. A running gateway caches a good
    /// validation for up to 30 seconds, so the token can still work there for
    /// that long (revoking through the gateway's own `/api/auth/revoke` takes
    /// effect immediately). Open WebSocket connections are not closed.
    Revoke {
        /// Token id (first 16 hex characters of its hash, from `weft token list`).
        id: String,
    },

    /// List live tokens (ids only; secrets are never stored).
    List,
}

/// Parse `15m` / `2h` / `90s` / `45` into seconds.
pub fn parse_ttl(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (digits, mult) = match s.chars().last() {
        Some('s') => (&s[..s.len() - 1], 1),
        Some('m') => (&s[..s.len() - 1], 60),
        Some('h') => (&s[..s.len() - 1], 3600),
        Some(c) if c.is_ascii_digit() => (s, 1),
        _ => return Err(format!("invalid ttl {s:?}: use e.g. 90s, 15m, 2h")),
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("invalid ttl {s:?}: use e.g. 90s, 15m, 2h"))?;
    if n == 0 {
        return Err("ttl must be positive".into());
    }
    n.checked_mul(mult).ok_or_else(|| "ttl too large".into())
}

/// The dashboard link for `secret`, on the API port. The dashboard reads
/// `#token=` once and strips it. The fragment is never sent to the server,
/// so the token stays out of access logs and `Referer`.
pub fn dashboard_link(host: &str, api_port: u16, secret: &str) -> String {
    let host = match host {
        "0.0.0.0" | "::" | "" => "localhost".to_owned(),
        h if h.contains(':') && !h.starts_with('[') => format!("[{h}]"),
        h => h.to_owned(),
    };
    format!("http://{host}:{api_port}/#token={secret}")
}

/// Default lifetime of a token issued for the dashboard.
pub const DASHBOARD_TTL_SECS: u64 = 15 * 60;

/// Ask the daemon for an owner token and return the dashboard link and the
/// token's expiry. Used by `weft ui`.
pub async fn issue_dashboard_link(host: &str, api_port: u16) -> anyhow::Result<(String, String)> {
    let v = daemon_call(
        "auth.token.issue",
        issue_params(DASHBOARD_TTL_SECS, "dashboard", None),
    )
    .await?;
    let secret = v["secret"].as_str().context("daemon returned no secret")?;
    let expires = v["expires_at"].as_str().unwrap_or("?").to_owned();
    Ok((dashboard_link(host, api_port, secret), expires))
}

/// Request params for `auth.token.issue`.
pub fn issue_params(ttl_secs: u64, label: &str, project: Option<&str>) -> Value {
    issue_params_scoped(ttl_secs, label, project, false)
}

/// [`issue_params`] with the read-only scope when `read_only`.
pub fn issue_params_scoped(ttl_secs: u64, label: &str, project: Option<&str>, read_only: bool) -> Value {
    let mut p = json!({ "ttl_secs": ttl_secs, "label": label });
    if read_only {
        p["scope"] = json!("read");
    }
    if let Some(project) = project {
        p["project"] = json!(project);
    }
    p
}

async fn daemon_call(method: &str, params: Value) -> anyhow::Result<Value> {
    let res = resolve(&ResolveFlags::default()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut connected = DaemonClient::connect_resolved(&res)
        .await
        .map_err(|e| anyhow::anyhow!("{e}\nstart the daemon: weft kernel start"))?;
    connected
        .client
        .call(Request::with_params(method, params))
        .await?
        .into_result()
        .with_context(|| format!("{method} failed"))
}

/// Run `weft token`.
pub async fn run<P: clawft_platform::Platform>(
    args: TokenArgs,
    platform: &P,
) -> anyhow::Result<()> {
    match args.action {
        TokenAction::Issue {
            ttl,
            label,
            project,
            read_only,
            config,
        } => {
            let v = daemon_call(
                "auth.token.issue",
                issue_params_scoped(ttl, &label, project.as_deref(), read_only),
            )
            .await?;
            let secret = v["secret"].as_str().context("daemon returned no secret")?;
            println!("token:   {secret}");
            println!("id:      {}", v["id"].as_str().unwrap_or("?"));
            println!("expires: {}", v["expires_at"].as_str().unwrap_or("?"));
            if read_only {
                println!("scope:   read-only (GET read routes only)");
            }
            println!("This is the only time the token is shown.");
            if read_only {
                // The dashboard link opens the full UI; a read-only token is for API clients.
            } else if let Ok(cfg) = super::load_config(platform, config.as_deref()).await {
                println!(
                    "link:    {}",
                    dashboard_link(&cfg.gateway.host, cfg.gateway.api_port, secret)
                );
            }
        }
        TokenAction::Revoke { id } => {
            let v = daemon_call("auth.token.revoke", json!({ "id": id })).await?;
            if v["revoked"] == true {
                println!("revoked {id}");
            } else {
                bail!("no live token with id {id}");
            }
        }
        TokenAction::List => {
            let v = daemon_call("auth.token.list", Value::Null).await?;
            let tokens = v["tokens"].as_array().cloned().unwrap_or_default();
            if tokens.is_empty() {
                println!("no live tokens");
            }
            for t in tokens {
                println!(
                    "{}  {:<16}  expires {}  project {}  scope {}",
                    t["id"].as_str().unwrap_or("?"),
                    t["label"].as_str().unwrap_or(""),
                    t["expires_at"].as_str().unwrap_or("?"),
                    t["project"].as_str().unwrap_or("-"),
                    t["scope"].as_str().unwrap_or("owner"),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Wrap {
        #[command(flatten)]
        args: TokenArgs,
    }

    #[test]
    fn read_only_flag_sets_the_read_scope_and_refuses_a_project() {
        assert_eq!(issue_params_scoped(60, "console", None, true)["scope"], "read");
        assert!(issue_params(60, "x", None).get("scope").is_none(), "owner is the unmarked default");
        let w = Wrap::try_parse_from(["t", "issue", "--read-only", "--label", "console"]).unwrap();
        assert!(matches!(w.args.action, TokenAction::Issue { read_only: true, .. }));
        assert!(
            Wrap::try_parse_from(["t", "issue", "--read-only", "--project", "01ARZ3NDEKTSV4RRFFQ69G5FAV"]).is_err()
        );
    }

    #[test]
    fn ttl_parsing() {
        assert_eq!(parse_ttl("15m"), Ok(900));
        assert_eq!(parse_ttl("2h"), Ok(7200));
        assert_eq!(parse_ttl("90s"), Ok(90));
        assert_eq!(parse_ttl("45"), Ok(45));
        assert!(parse_ttl("0m").is_err());
        assert!(parse_ttl("abc").is_err());
        assert!(parse_ttl("").is_err());
        assert!(parse_ttl("-5m").is_err());
    }

    #[test]
    fn issue_defaults_and_flags() {
        let w = Wrap::try_parse_from(["t", "issue"]).unwrap();
        match w.args.action {
            TokenAction::Issue {
                ttl,
                label,
                project,
                ..
            } => {
                assert_eq!((ttl, label.as_str(), project), (900, "playground", None));
            }
            _ => panic!(),
        }
        let w = Wrap::try_parse_from([
            "t",
            "issue",
            "--ttl",
            "1h",
            "--label",
            "x",
            "--project",
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        ])
        .unwrap();
        match w.args.action {
            TokenAction::Issue {
                ttl,
                label,
                project,
                ..
            } => {
                assert_eq!(ttl, 3600);
                assert_eq!(label, "x");
                assert_eq!(project.as_deref(), Some("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
            }
            _ => panic!(),
        }
        assert!(Wrap::try_parse_from(["t", "issue", "--ttl", "zz"]).is_err());
    }

    #[test]
    fn revoke_requires_id_and_list_parses() {
        assert!(Wrap::try_parse_from(["t", "revoke"]).is_err());
        assert!(Wrap::try_parse_from(["t", "revoke", "abc"]).is_ok());
        assert!(Wrap::try_parse_from(["t", "list"]).is_ok());
    }

    #[test]
    fn link_uses_the_token_fragment_the_dashboard_reads() {
        assert_eq!(
            dashboard_link("0.0.0.0", 18789, "wft_s"),
            "http://localhost:18789/#token=wft_s"
        );
        assert_eq!(dashboard_link("gw.lan", 80, "t"), "http://gw.lan:80/#token=t");
        assert_eq!(dashboard_link("::1", 18789, "t"), "http://[::1]:18789/#token=t");
        assert_eq!(dashboard_link("[::1]", 18789, "t"), "http://[::1]:18789/#token=t");
        // The fragment is client-side only: nothing after `#` is a query.
        assert!(!dashboard_link("h", 1, "t").contains("?token"));
    }

    #[test]
    fn issue_params_shape() {
        assert_eq!(
            issue_params(60, "l", None),
            json!({"ttl_secs": 60, "label": "l"})
        );
        assert_eq!(issue_params(60, "l", Some("P"))["project"], "P");
    }
}
