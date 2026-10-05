//! Where the console points (host, registries, token) and how those settings are read: an env
//! var natively, a `?query` or `#fragment` param in the browser (the fragment wins; tokens read
//! from the URL are removed from the address bar).

#[derive(Clone)]
pub struct Settings {
    /// cog-host base, e.g. `http://127.0.0.1:9480` or `http://100.64.0.10:9480`.
    pub host: String,
    /// WeaveLogic signed registry.json URL (optional; empty = skip).
    pub our_registry: String,
    /// Cognitum app-registry.json URL (optional; empty = skip).
    pub cognitum_registry: String,
    /// Optional bearer override (`WEFTOS_HOST_TOKEN`, or `?token=` in the browser). On a tailnet
    /// or local connection the console registers its own mesh key and fills this in. A pasted
    /// value is that host's own token and is never sent to a different host.
    pub token: String,
    /// ADR-102 gateway base for the fleet snapshot (`WEFTOS_GATEWAY`, or `?gw=`); empty = the
    /// Network tab shows only what this cog-host sees.
    pub gateway: String,
    /// Gateway bearer token; a read-only one is enough (`weft token issue --read-only`).
    /// `WEFTOS_GATEWAY_TOKEN`, or `?gwtoken=` in the browser.
    pub gateway_token: String,
    /// Cognitum Seed agent base URLs to read (`WEFTOS_SEEDS` / `?seeds=`, comma-separated). The
    /// connected cog-host's own agent is probed as well, without being listed here.
    pub seeds: Vec<String>,
    /// WeftOS project ULID the console is scoped to (`WEFTOS_PROJECT`, or `?project=`); empty = no
    /// project, the unfiltered console. An invalid value is ignored, with `project_warning` set.
    pub project: String,
    pub project_warning: Option<String>,
}

/// Comma- or space-separated Seed URLs; a bare host gets `http://`.
pub fn parse_seeds(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(|x| if x.starts_with("http://") || x.starts_with("https://") { x.trim_end_matches('/').to_string() } else { format!("http://{}", x.trim_end_matches('/')) })
        .collect()
}

impl Default for Settings {
    fn default() -> Self {
        let (project, project_warning) = super::params::check_project(&setting("WEFTOS_PROJECT", "project", ""));
        Self {
            project,
            project_warning,
            // native: env; browser: ?host= / ?wl= / ?cog= query params; else the default.
            host: setting("WEFTOS_HOST", "host", crate::address_book::LOCAL_HOST),
            token: setting("WEFTOS_HOST_TOKEN", "token", ""),
            gateway: setting("WEFTOS_GATEWAY", "gw", ""),
            gateway_token: setting("WEFTOS_GATEWAY_TOKEN", "gwtoken", ""),
            seeds: parse_seeds(&setting("WEFTOS_SEEDS", "seeds", "")),
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
// Native reads an env var; the browser reads a `#<key>=` fragment param, else `?<key>=`; else the
// default.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn setting(env_key: &str, _query_key: &str, default: &str) -> String {
    std::env::var(env_key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn setting(_env_key: &str, query_key: &str, default: &str) -> String {
    let (query, fragment) = initial_url();
    super::params::lookup(query, fragment, query_key).unwrap_or_else(|| default.to_string())
}

/// The page's `(search, hash)` as first loaded. Captured once: the secrets are then removed from
/// the address bar, and later `Settings::default()` calls must still see them.
#[cfg(target_arch = "wasm32")]
fn initial_url() -> &'static (String, String) {
    static URL: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    URL.get_or_init(|| {
        let Some(w) = web_sys::window() else { return Default::default() };
        let loc = w.location();
        let (search, hash) = (loc.search().unwrap_or_default(), loc.hash().unwrap_or_default());
        scrub_address_bar(&w, &loc, &search);
        (search, hash)
    })
}

/// Drop the fragment and the token query params from the address bar (no history entry), so a
/// token does not linger in history or a Referer header.
#[cfg(target_arch = "wasm32")]
fn scrub_address_bar(w: &web_sys::Window, loc: &web_sys::Location, search: &str) {
    let (stripped, hash) = (super::params::strip_secret_query(search), loc.hash().unwrap_or_default());
    if stripped == search && hash.is_empty() {
        return;
    }
    let url = format!("{}{stripped}", loc.pathname().unwrap_or_default());
    if let Ok(h) = w.history() {
        let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
    }
}
