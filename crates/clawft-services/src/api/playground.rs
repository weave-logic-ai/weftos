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
            resp
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            "playground is not built: run `scripts/build.sh ui` and point the gateway at clawft-ui/dist",
        )
            .into_response(),
    }
}
