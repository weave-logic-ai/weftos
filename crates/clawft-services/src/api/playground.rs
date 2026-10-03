//! `GET /playground`: the API playground page (ADR-102 D2).
//!
//! The page is the `playground.html` entry of the built `clawft-ui` bundle,
//! served from the same static directory as the dashboard. The HTML itself
//! is public and carries no data and no token: the token arrives in the URL
//! fragment, which the browser never sends, and every data call the page
//! makes (`/api/*`, `/mcp`) goes through the bearer middleware like any
//! other client.

use std::path::PathBuf;

use axum::{
    Router,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};

use super::ApiState;

/// Entry file name inside the static directory.
pub const PAGE_FILE: &str = "playground.html";

/// CSP for the playground page. The gateway-wide policy also allows `ws:` and
/// `wss:` to any host, which a page holding a full-scope bearer must not
/// inherit: it only ever talks to its own origin, from same-origin scripts.
pub const PLAYGROUND_CSP: &str = "default-src 'self'; script-src 'self'; \
style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; \
base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// `/playground` routes for the built UI in `static_dir`.
///
/// Both `/playground` and `/playground/` serve the page; the router's
/// fallback would otherwise answer with a 404 for them.
pub fn playground_routes(static_dir: &str) -> Router<ApiState> {
    let path = PathBuf::from(static_dir).join(PAGE_FILE);
    let handler = move || {
        let path = path.clone();
        async move { page(path).await }
    };
    Router::new()
        .route("/playground", get(handler.clone()))
        .route("/playground/", get(handler))
}

async fn page(path: PathBuf) -> Response {
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mut resp = bytes.into_response();
            let h = resp.headers_mut();
            h.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            // The page is tiny and versioned with the binary; never let a
            // proxy or the browser keep a stale copy of a security page.
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            h.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(PLAYGROUND_CSP),
            );
            resp
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            "playground is not built: run `scripts/build.sh ui` and point the gateway at clawft-ui/dist",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::PLAYGROUND_CSP;

    #[test]
    fn csp_is_same_origin_only() {
        for bad in ["ws:", "wss:", "*", "http:", "https:", "unsafe-eval", "wasm-unsafe-eval"] {
            assert!(!PLAYGROUND_CSP.contains(bad), "{bad} in {PLAYGROUND_CSP}");
        }
        let script = PLAYGROUND_CSP.split("; ").find(|d| d.starts_with("script-src")).unwrap();
        assert_eq!(script, "script-src 'self'");
        assert!(PLAYGROUND_CSP.contains("connect-src 'self';"));
        assert!(PLAYGROUND_CSP.contains("base-uri 'none'"));
        assert!(PLAYGROUND_CSP.contains("frame-ancestors 'none'"));
    }

    /// When the UI has been built, its playground entry must load only
    /// same-origin script files: no inline script body, no external URL.
    #[test]
    fn built_page_has_no_inline_script_or_external_urls() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../clawft-ui/dist")
            .join(super::PAGE_FILE);
        let Ok(html) = std::fs::read_to_string(&path) else {
            eprintln!("skipping: {} not built", path.display());
            return;
        };
        let script = regex::Regex::new(r"(?is)<script\b([^>]*)>(.*?)</script>").unwrap();
        let mut n = 0;
        for m in script.captures_iter(&html) {
            n += 1;
            assert!(m[1].contains("src=\"/"), "script without a same-origin src: {}", &m[0]);
            assert!(m[2].trim().is_empty(), "inline script body: {}", &m[0]);
        }
        assert!(n >= 1, "no script tag found");
        let ext = regex::Regex::new(r#"(?i)(https?:)?//[a-z0-9.-]+\.[a-z]{2,}"#).unwrap();
        assert!(!ext.is_match(&html), "external URL in playground.html");
    }
}
