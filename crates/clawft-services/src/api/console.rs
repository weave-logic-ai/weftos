//! The project console on the gateway (dashboard project console, slice 5/6).
//!
//! Two independent pieces, both off unless configured:
//!
//! * **Static console** (`gateway.staticDir` / `--static-dir`): the built
//!   cog-manager web app served under `/console/` (never at the root), with a
//!   CSP whose `connect-src` is `'self'` plus `gateway.consoleConnectSrc`.
//! * **Tailnet-identity token mint** (`gateway.tailnetIdentity`):
//!   `POST /api/console/token` `{"project": "<ulid>"}` returns a short-lived
//!   read-only token confined to that project. The caller is identified by
//!   its TCP peer address (never `X-Forwarded-For`), which must be a tailnet
//!   address; `tailscale whois` names the login, which must be on the
//!   allowlist. The tailnet is the trust boundary: anyone who can reach the
//!   gateway over the tailnet as an allowlisted login can read that project.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{ConnectInfo, Extension, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};

use super::ApiState;
use super::daemon_facade::DaemonKernelFacade;

/// Longest console token lifetime, mirroring `clawft_kernel::token_authority::MAX_TTL`
/// (24 h). That module needs the `exochain` feature, which the `weft` build does not
/// enable; the daemon enforces the authoritative cap when it issues the token.
const MAX_TTL_SECS: u64 = 24 * 60 * 60;

/// Longest the `tailscale whois` subprocess may run.
const WHOIS_TIMEOUT: Duration = Duration::from_secs(3);

/// Console settings handed to the router. `Default` = everything off.
#[derive(Default)]
pub struct ConsoleOptions {
    /// Built console directory, served under `/console/`.
    pub static_dir: Option<String>,
    /// Extra `connect-src` origins for the console page.
    pub connect_src: Vec<String>,
    /// Tailnet-identity minting; `None` leaves the route unmounted (404).
    pub tailnet: Option<Arc<TailnetMint>>,
}

// ── CSP ─────────────────────────────────────────────────────────────

/// Whether `o` is a bare origin (`scheme://host[:port]`) that is safe to put
/// in a CSP source list: no whitespace, `;`, `,`, quotes, path or wildcard.
fn is_bare_origin(o: &str) -> bool {
    let Some((scheme, rest)) = o.split_once("://") else { return false };
    matches!(scheme, "http" | "https" | "ws" | "wss")
        && !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}

/// The console page's CSP. Errors on an origin that is not a bare origin, so
/// a config typo cannot widen or break the policy.
pub fn console_csp(connect_src: &[String]) -> Result<String, String> {
    for o in connect_src {
        if !is_bare_origin(o) {
            return Err(format!(
                "gateway.consoleConnectSrc: '{o}' is not a bare origin like http://host:port"
            ));
        }
    }
    let mut connect = String::from("'self'");
    for o in connect_src {
        connect.push(' ');
        connect.push_str(o);
    }
    Ok(format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src {connect}; \
base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    ))
}

/// `/console/` static routes for `dir`, with the console CSP on every
/// response. Unknown paths fall back to `index.html` (SPA routing).
pub fn console_static_router(dir: &str, csp: &str) -> Result<Router<ApiState>, String> {
    use tower_http::services::{ServeDir, ServeFile};
    let csp = HeaderValue::from_str(csp).map_err(|e| e.to_string())?;
    let index = std::path::Path::new(dir).join("index.html");
    let service = ServeDir::new(dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(index));
    let router = Router::new()
        .nest_service("/console", service)
        .layer(axum::middleware::from_fn(move |req: Request, next: axum::middleware::Next| {
            let csp = csp.clone();
            async move {
                let mut resp = next.run(req).await;
                resp.headers_mut().insert(header::CONTENT_SECURITY_POLICY, csp);
                resp
            }
        }));
    Ok(router)
}

// ── Tailnet identity ────────────────────────────────────────────────

/// Whether `ip` is a Tailscale address: `100.64.0.0/10` or `fd7a:115c:a1e0::/48`.
pub fn is_tailnet_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (o[1] & 0xc0) == 64
        }
        IpAddr::V6(v6) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

/// Why a whois lookup could not answer.
#[derive(Debug)]
pub struct WhoisError(pub String);

/// Resolves a tailnet address to the Tailscale login behind it.
#[async_trait]
pub trait TailnetWhois: Send + Sync {
    /// `Ok(Some(login))`, `Ok(None)` when Tailscale knows no user for the
    /// address (unknown peer, tagged node), `Err` when it cannot be asked.
    async fn login_of(&self, ip: IpAddr) -> Result<Option<String>, WhoisError>;
}

/// [`TailnetWhois`] over `tailscale whois --json <ip>` (no shell, bounded time).
pub struct TailscaleCli {
    bin: String,
}

impl TailscaleCli {
    /// Use `bin` (default `tailscale` from `PATH`).
    pub fn new(bin: Option<String>) -> Self {
        Self { bin: bin.unwrap_or_else(|| "tailscale".into()) }
    }
}

/// The `UserProfile.LoginName` of a `tailscale whois --json` document.
pub fn login_from_whois_json(out: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(out).ok()?;
    v["UserProfile"]["LoginName"]
        .as_str()
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
}

#[async_trait]
impl TailnetWhois for TailscaleCli {
    async fn login_of(&self, ip: IpAddr) -> Result<Option<String>, WhoisError> {
        let mut cmd = tokio::process::Command::new(&self.bin);
        cmd.arg("whois").arg("--json").arg(ip.to_string()).kill_on_drop(true);
        cmd.stdin(std::process::Stdio::null());
        let out = tokio::time::timeout(WHOIS_TIMEOUT, cmd.output())
            .await
            .map_err(|_| WhoisError("tailscale whois timed out".into()))?
            .map_err(|e| WhoisError(format!("cannot run tailscale: {e}")))?;
        if out.status.success() {
            return Ok(login_from_whois_json(&out.stdout));
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("no match") || err.contains("not found") {
            return Ok(None);
        }
        Err(WhoisError("tailscale whois failed".into()))
    }
}

/// A freshly minted token.
pub struct Minted {
    /// The secret (shown once).
    pub token: String,
    /// RFC 3339 expiry.
    pub expires_at: String,
}

/// Why minting failed.
#[derive(Debug, PartialEq, Eq)]
pub enum MintError {
    /// The daemon could not be reached.
    Unavailable,
    /// The daemon refused or returned nothing usable.
    Refused,
}

/// Issues a read-only project-confined token (the daemon, in production).
#[async_trait]
pub trait TokenMinter: Send + Sync {
    /// Mint a `read` token bound to `project`, valid for `ttl_secs`.
    async fn mint_read_for_project(
        &self,
        project: &str,
        label: &str,
        ttl_secs: u64,
    ) -> Result<Minted, MintError>;
}

/// [`TokenMinter`] over the daemon's `auth.token.issue`. The gateway is the
/// local owner's process; this asks for `admin` to issue, but only ever with
/// `scope: read` and a project, hard-coded here.
pub struct DaemonTokenMinter(pub Arc<DaemonKernelFacade>);

#[async_trait]
impl TokenMinter for DaemonTokenMinter {
    async fn mint_read_for_project(
        &self,
        project: &str,
        label: &str,
        ttl_secs: u64,
    ) -> Result<Minted, MintError> {
        let params = json!({
            "scope": "read", "project": project, "label": label, "ttl_secs": ttl_secs,
        });
        let resp = self
            .0
            .auth_call("auth.token.issue", params, "admin")
            .await
            .map_err(|_| MintError::Unavailable)?;
        if !resp.ok {
            tracing::warn!(kind = ?resp.error_kind, "daemon refused auth.token.issue");
            return Err(MintError::Refused);
        }
        let r = resp.result.unwrap_or(Value::Null);
        match (r["secret"].as_str(), r["expires_at"].as_str()) {
            (Some(t), Some(e)) => Ok(Minted { token: t.into(), expires_at: e.into() }),
            _ => Err(MintError::Refused),
        }
    }
}

/// Everything `POST /api/console/token` needs.
pub struct TailnetMint {
    allowed: Vec<String>,
    ttl_secs: u64,
    whois: Arc<dyn TailnetWhois>,
    minter: Arc<dyn TokenMinter>,
    /// Extra browser origins (the gateway's CORS list) that may call the route.
    origins: Vec<String>,
}

impl TailnetMint {
    /// Build from config values. Logins compare case-insensitively; the TTL
    /// is clamped to `[1, authority max]`.
    pub fn new(
        allowed_logins: &[String],
        ttl_secs: u64,
        origins: &[String],
        whois: Arc<dyn TailnetWhois>,
        minter: Arc<dyn TokenMinter>,
    ) -> Self {
        Self {
            allowed: allowed_logins.iter().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect(),
            ttl_secs: ttl_secs.clamp(1, MAX_TTL_SECS),
            whois,
            minter,
            origins: origins.to_vec(),
        }
    }

    fn allows(&self, login: &str) -> bool {
        let l = login.trim().to_lowercase();
        self.allowed.contains(&l)
    }
}

/// Route `POST /api/console/token`. Not behind the bearer middleware: the
/// tailnet identity is the credential.
pub fn token_route(mint: Arc<TailnetMint>) -> Router<ApiState> {
    let route = Router::new().route("/api/console/token", post(console_token));
    route.layer(Extension(mint))
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

/// Browser guard: a request that names an `Origin` must come from the page's
/// own origin or an origin on the CORS list, so another site cannot make a
/// tailnet user's browser mint a token. Non-browser clients send no Origin.
fn origin_ok(headers: &HeaderMap, mint: &TailnetMint) -> bool {
    if headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !matches!(v, "same-origin" | "none"))
    {
        // `same-site`/`cross-site`: only allowed when the origin is listed.
        return headers
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|o| mint.origins.iter().any(|a| a == o));
    }
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    if mint.origins.iter().any(|a| a == origin) {
        return true;
    }
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    match (origin.split_once("://"), host) {
        (Some((_, authority)), Some(h)) => authority.eq_ignore_ascii_case(h),
        _ => false,
    }
}

async fn console_token(
    State(state): State<ApiState>,
    Extension(mint): Extension<Arc<TailnetMint>>,
    request: Request,
) -> Response {
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
    let (parts, body) = request.into_parts();
    // Only the TCP peer counts; X-Forwarded-For is never read.
    let Some(ip) = peer.filter(|ip| is_tailnet_ip(*ip)) else {
        return err(StatusCode::FORBIDDEN, "not a tailnet peer");
    };
    if !origin_ok(&parts.headers, &mint) {
        return err(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let json_ct = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().to_ascii_lowercase().starts_with("application/json"));
    if !json_ct {
        return err(StatusCode::UNSUPPORTED_MEDIA_TYPE, "content-type must be application/json");
    }
    let Ok(bytes) = axum::body::to_bytes(body, 4096).await else {
        return err(StatusCode::BAD_REQUEST, "body too large");
    };
    let project = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v["project"].as_str().map(str::to_owned))
        .filter(|p| clawft_types::project::validate_id(p).is_ok());
    let Some(project) = project else {
        return err(StatusCode::BAD_REQUEST, "body must be {\"project\": \"<ulid>\"}");
    };

    let login = match mint.whois.login_of(ip).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            tracing::warn!(%ip, "console token refused: tailscale knows no user for this peer");
            return err(StatusCode::FORBIDDEN, "tailnet identity not recognised");
        }
        Err(WhoisError(why)) => {
            tracing::warn!(%ip, why, "console token: whois unavailable");
            return err(StatusCode::SERVICE_UNAVAILABLE, "tailnet identity lookup unavailable");
        }
    };
    if !mint.allows(&login) {
        tracing::warn!(%login, %project, "console token refused: login not allowlisted");
        return err(StatusCode::FORBIDDEN, "login not allowed");
    }

    // The project must exist.
    let shown = state.kernel_facade.call_rpc("project.show", json!({ "id": project })).await;
    if shown.status != 200 {
        let status = StatusCode::from_u16(shown.status).unwrap_or(StatusCode::BAD_GATEWAY);
        return (status, Json(shown.body)).into_response();
    }

    let label: String = format!("console:{login}").chars().filter(|c| !c.is_control()).take(64).collect();
    match mint.minter.mint_read_for_project(&project, &label, mint.ttl_secs).await {
        Ok(m) => {
            tracing::info!(%login, %project, ttl_secs = mint.ttl_secs, "console token minted");
            let mut resp = Json(json!({
                "token": m.token, "expires_at": m.expires_at, "project": project,
            }))
            .into_response();
            resp.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            resp
        }
        Err(MintError::Unavailable) => err(StatusCode::SERVICE_UNAVAILABLE, "daemon unavailable"),
        Err(MintError::Refused) => err(StatusCode::BAD_GATEWAY, "token authority refused"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn tailnet_ranges() {
        for ok in ["100.64.0.0", "100.101.102.103", "100.127.255.255", "fd7a:115c:a1e0::1", "fd7a:115c:a1e0:ffff::9", "::ffff:100.64.1.1"] {
            assert!(is_tailnet_ip(ip(ok)), "{ok}");
        }
        for bad in ["100.63.255.255", "100.128.0.0", "10.0.0.1", "127.0.0.1", "192.168.1.1", "::1", "fd7a:115c:a1e1::1", "fd7b:115c:a1e0::1", "2001:db8::1"] {
            assert!(!is_tailnet_ip(ip(bad)), "{bad}");
        }
    }

    #[test]
    fn whois_json_login() {
        let j = br#"{"Node":{"Name":"a.ts.net."},"UserProfile":{"ID":1,"LoginName":"a@b.c","DisplayName":"A"}}"#;
        assert_eq!(login_from_whois_json(j).as_deref(), Some("a@b.c"));
        // A tagged node has no user profile login.
        assert_eq!(login_from_whois_json(br#"{"Node":{"Tags":["tag:x"]},"UserProfile":{"LoginName":""}}"#), None);
        assert_eq!(login_from_whois_json(b"garbage"), None);
    }

    #[test]
    fn csp_lists_only_valid_origins() {
        let c = console_csp(&["http://h:1".into()]).unwrap();
        assert!(c.contains("connect-src 'self' http://h:1;"));
        assert!(console_csp(&[]).unwrap().contains("connect-src 'self';"));
        assert!(console_csp(&["https://*.x".into()]).is_err());
    }

    #[tokio::test]
    async fn a_missing_tailscale_binary_is_a_whois_error() {
        let w = TailscaleCli::new(Some("/nonexistent/tailscale-bin".into()));
        assert!(w.login_of(ip("100.64.0.1")).await.is_err());
    }
}
