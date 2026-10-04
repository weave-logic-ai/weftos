//! Where the console points (host, registries, token) and how those settings are read: an env
//! var natively, a `?query` param in the browser.

#[derive(Clone)]
pub struct Settings {
    /// cog-host base, e.g. `http://127.0.0.1:9480` or `http://100.64.0.10:9480`.
    pub host: String,
    /// WeaveLogic signed registry.json URL (optional; empty = skip).
    pub our_registry: String,
    /// Cognitum app-registry.json URL (optional; empty = skip).
    pub cognitum_registry: String,
    /// cog-host bearer token for the `/hw/*` mutating routes (`WEFTOS_HOST_TOKEN`, or `?token=` in
    /// the browser). Found in `<host root>/host.token`.
    pub token: String,
    /// ADR-102 gateway base for the fleet snapshot (`WEFTOS_GATEWAY`, or `?gw=`); empty = the
    /// Network tab shows only what this cog-host sees.
    pub gateway: String,
    /// Gateway bearer token; a read-only one is enough (`weft token issue --read-only`).
    /// `WEFTOS_GATEWAY_TOKEN`, or `?gwtoken=` in the browser.
    pub gateway_token: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // native: env; browser: ?host= / ?wl= / ?cog= query params; else the default.
            host: setting("WEFTOS_HOST", "host", "http://127.0.0.1:9480"),
            token: setting("WEFTOS_HOST_TOKEN", "token", ""),
            gateway: setting("WEFTOS_GATEWAY", "gw", ""),
            gateway_token: setting("WEFTOS_GATEWAY_TOKEN", "gwtoken", ""),
            our_registry: setting("WEFTOS_WL_REGISTRY", "wl", ""),
            cognitum_registry: setting(
                "WEFTOS_COGNITUM_REGISTRY",
                "cog",
                "https://storage.googleapis.com/cognitum-apps/app-registry.json",
            ),
        }
    }
}

// ---- setting resolution ---------------------------------------------------
// Native reads an env var; the browser reads a `?<query>=` param; else the default.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn setting(env_key: &str, _query_key: &str, default: &str) -> String {
    std::env::var(env_key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn setting(_env_key: &str, query_key: &str, default: &str) -> String {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|q| {
            let prefix = format!("{query_key}=");
            q.trim_start_matches('?').split('&').find_map(|kv| {
                kv.strip_prefix(&prefix).map(|v| js_decode(v))
            })
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Minimal percent-decode for query values (handles %3A %2F etc. in URLs passed as params).
#[cfg(target_arch = "wasm32")]
fn js_decode(s: &str) -> String {
    let bytes = s.replace('+', " ");
    let mut out = Vec::new();
    let mut it = bytes.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let h = (it.next(), it.next());
            if let (Some(a), Some(c)) = h {
                if let (Some(x), Some(y)) = (hexval(a), hexval(c)) {
                    out.push(x * 16 + y);
                    continue;
                }
            }
        } else {
            out.push(b);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_arch = "wasm32")]
fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
