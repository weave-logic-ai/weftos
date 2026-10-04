//! Installing a marketplace cog on the connected host.

use super::*;
use base64::Engine as _;

impl Client {
    /// Install a catalog item onto the host: fetch the binary (TLS here, in the browser/native),
    /// then upload it to the host's `/install`, which verifies (Ed25519 for signed, sha256 for
    /// Cognitum) before writing. Progress/result lands in `last_action`.
    pub fn install(&self, id: &str, source: Source, version: String, ctx: &eframe::egui::Context) {
        // Resolve the binary URL + integrity material from the raw registries.
        let detail: Option<(String, String, Option<String>, bool)> = {
            let sh = self.shared.lock().unwrap();
            match source {
                Source::WeaveLogic => {
                    let b = registry_base(&self.s.our_registry);
                    sh.our_reg
                        .as_ref()
                        .and_then(|r| r.as_ref().ok())
                        .and_then(|r| r.cogs.iter().find(|c| c.id == id).cloned())
                        .and_then(|c| c.artifacts.get("arm").or_else(|| c.artifacts.values().next()).cloned())
                        .map(|a| (format!("{b}/{}", a.path), a.sha256, Some(a.sig), true))
                }
                Source::Cognitum => sh.cognitum_reg.as_ref().and_then(|r| r.as_ref().ok()).and_then(|r| {
                    let url = r.arm_binary_url(id)?;
                    let sha = r.cogs.iter().find(|c| c.id == id).and_then(|c| c.sha256.clone())?;
                    Some((url, sha, None, false))
                }),
            }
        };
        let Some((url, sha, sig, signed)) = detail else {
            self.set_action(format!("install {id}: no binary URL (is the registry configured/loaded?)"), ctx);
            return;
        };

        self.set_action(format!("fetching {id}…"), ctx);
        let epoch = epoch_of(&self.shared);
        let shared = Arc::clone(&self.shared);
        let host = base(&self.s.host);
        let token = self.s.token.clone();
        let busy = Arc::clone(&self.status_busy);
        let id = id.to_string();
        let ctx = ctx.clone();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            let bytes = match &res {
                Ok(r) if r.ok => r.bytes.clone(),
                Ok(r) => return set_in(&shared, epoch, &ctx, format!("fetch {id}: HTTP {}", r.status)),
                Err(e) => return set_in(&shared, epoch, &ctx, format!("fetch {id}: {e} (CORS? use the native console for this source)")),
            };
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let body = serde_json::json!({
                "id": id, "version": version,
                "source": if signed { "weavelogic" } else { "cognitum" },
                "signed": signed, "sha256": sha, "sig": sig,
                "binary_b64": b64, "enable": false,
            })
            .to_string();
            let mut req = ehttp::Request::post(format!("{host}/install"), body.into_bytes());
            post_headers(&mut req, &token);
            let shared2 = Arc::clone(&shared);
            let busy2 = Arc::clone(&busy);
            let ctx2 = ctx.clone();
            let id2 = id.clone();
            ehttp::fetch(req, move |r2| {
                let msg = match &r2 {
                    Ok(x) if x.ok => format!("installed {id2} ✓ ({} KB, verified)", bytes.len() / 1024),
                    Ok(x) => format!("install {id2}: {}", String::from_utf8_lossy(&x.bytes).chars().take(160).collect::<String>()),
                    Err(e) => format!("install {id2}: {e}"),
                };
                busy2.store(false, Ordering::Release); // prompt a status refresh so it shows up
                set_in(&shared2, epoch, &ctx2, msg);
            });
        });
    }
}
