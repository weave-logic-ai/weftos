//! Process-wide inference role resolution (card mesh-placement-20;
//! ADR-101 section 5).
//!
//! The daemon installs a resolver that answers "where is inference role `R`
//! served right now?" and the consumers that must not depend on the kernel
//! (the voice TTS, the LLM service client) ask it. Nothing is installed by
//! default, so [`resolve`] answers `None` and every consumer keeps its own
//! configured address: placement is opt-in and an explicit endpoint always
//! wins because consumers only ask when they were not given one.
//!
//! The answer is the server's root URL (`http://127.0.0.1:PORT`, no `/v1`).
//! A role served on this node resolves to the instance itself, so the
//! latency path has no extra hop; a role served elsewhere resolves to this
//! node's loopback proxy.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Role name to base URL (`.../v1` or root; [`resolve`] normalises).
pub type RoleResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

struct Installed {
    resolve: RoleResolver,
    providers: HashMap<String, String>,
}

static HOOK: RwLock<Option<Installed>> = RwLock::new(None);

/// Install the resolver and the provider-to-role map (replaces any earlier).
pub fn install(resolve: RoleResolver, providers: HashMap<String, String>) {
    if let Ok(mut g) = HOOK.write() {
        *g = Some(Installed { resolve, providers });
    }
}

/// Remove the resolver (tests, shutdown).
pub fn clear() {
    if let Ok(mut g) = HOOK.write() {
        *g = None;
    }
}

fn root(url: &str) -> String {
    let u = url.trim_end_matches('/');
    u.strip_suffix("/v1").unwrap_or(u).to_string()
}

/// Root URL serving `role` now, or `None` (no resolver, unknown role,
/// nothing healthy): the caller keeps its own address.
pub fn resolve(role: &str) -> Option<String> {
    let g = HOOK.read().ok()?;
    let h = g.as_ref()?;
    (h.resolve)(role).map(|u| root(&u))
}

/// [`resolve`] for the role a provider name (`local`) follows.
pub fn resolve_for_provider(provider: &str) -> Option<String> {
    let role = {
        let g = HOOK.read().ok()?;
        g.as_ref()?.providers.get(provider)?.clone()
    };
    resolve(&role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_resolves_until_installed_and_urls_are_normalised() {
        // One body: the hook is process-wide.
        assert_eq!(resolve("hermes"), None);
        let mut providers = HashMap::new();
        providers.insert("local".to_string(), "hermes".to_string());
        install(
            Arc::new(|r| match r {
                "hermes" => Some("http://127.0.0.1:18090/v1".into()),
                "orpheus-tts" => Some("http://127.0.0.1:11434".into()),
                _ => None,
            }),
            providers,
        );
        assert_eq!(resolve("hermes").as_deref(), Some("http://127.0.0.1:18090"));
        assert_eq!(resolve("orpheus-tts").as_deref(), Some("http://127.0.0.1:11434"));
        assert_eq!(resolve("other"), None);
        assert_eq!(resolve_for_provider("local").as_deref(), Some("http://127.0.0.1:18090"));
        assert_eq!(resolve_for_provider("openai"), None);
        clear();
        assert_eq!(resolve("hermes"), None);
    }
}
