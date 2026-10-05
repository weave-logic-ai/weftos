//! Register and keep a mesh session with the connected cog-host.
//!
//! Native only. The browser build has no operator key. A LAN or named host is not enrolled:
//! the mesh registers a key for a tailnet address or for this machine.

use super::*;

/// What a challenge response means. An old cog-host has no open enroll route: `GET /mesh/*`
/// is token-guarded, so the challenge comes back 401. A host that registers keys returns the
/// nonce, or 403 when the peer is not a tailnet or local address. 404 is the same old host
/// once the route exists and the key store is absent.
fn challenge_reply(status: u16) -> ChallengeReply {
    match status {
        200..=299 => ChallengeReply::Sign,
        401 | 404 => ChallengeReply::Unsupported,
        403 => ChallengeReply::Forbidden,
        _ => ChallengeReply::Other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChallengeReply {
    Sign,
    Unsupported,
    Forbidden,
    Other,
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct MeshAuto {
    pub busy: Arc<AtomicBool>,
    pub at: Option<Instant>,
    pub maintain_at: Option<Instant>,
    pub loaded_for: String,
    pub operator: Option<crate::mesh_key::Operator>,
}

#[cfg(not(target_arch = "wasm32"))]
impl MeshAuto {
    pub fn new() -> Self {
        Self { busy: Arc::new(AtomicBool::new(false)), at: None, maintain_at: None, loaded_for: String::new(), operator: None }
    }
}

impl Client {
    pub fn ensure_mesh_session(&mut self, ctx: &eframe::egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        self.ensure_mesh_session_native(ctx);
        #[cfg(target_arch = "wasm32")]
        {
            let _ = ctx;
        }
    }

    /// Session bearer the host just issued. The frame loop copies it onto the settings token.
    pub fn take_issued_token(&self) -> Option<String> {
        let mut sh = self.shared.lock().unwrap();
        if sh.mesh_token.is_empty() { None } else { Some(std::mem::take(&mut sh.mesh_token)) }
    }

    /// The host refused the bearer we sent, so the saved session for this host is no longer good.
    pub fn take_mesh_clear(&self) -> bool {
        let mut sh = self.shared.lock().unwrap();
        let clear = sh.mesh_clear;
        sh.mesh_clear = false;
        clear
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Client {
    fn ensure_mesh_session_native(&mut self, ctx: &eframe::egui::Context) {
        let host = base(&self.s.host);
        if !matches!(crate::address_book::classify(&host), crate::address_book::Reach::ThisMachine | crate::address_book::Reach::Tailnet) {
            self.set_mesh_note("the mesh registers a key on a tailnet or local connection");
            return;
        }
        let rejected = {
            let mut sh = self.shared.lock().unwrap();
            let rejected = sh.mesh_rejected;
            sh.mesh_rejected = false;
            rejected
        };
        if rejected {
            self.s.token.clear();
            crate::mesh_key::forget_session(&host);
            self.mesh_auto.at = None;
            self.mesh_auto.loaded_for.clear();
            self.shared.lock().unwrap().mesh_clear = true;
            self.set_mesh_note("the host refused the mesh key; registering again");
        }
        if self.s.token.trim().is_empty() && self.mesh_auto.loaded_for != host {
            self.mesh_auto.loaded_for = host.clone();
            if let Some(tok) = crate::mesh_key::token_for(&host) {
                self.s.token = tok.clone();
                self.shared.lock().unwrap().mesh_token = tok;
                self.set_mesh_note("");
            }
        }
        if self.s.token.trim().is_empty() {
            self.begin_enroll(ctx, &host);
        } else {
            self.maybe_maintain(ctx, &host);
        }
    }

    fn set_mesh_note(&self, note: &str) {
        self.shared.lock().unwrap().mesh_note = note.to_string();
    }

    fn begin_enroll(&mut self, ctx: &eframe::egui::Context, host: &str) {
        if self.shared.lock().unwrap().mesh_unsupported {
            return;
        }
        let busy = self.mesh_auto.busy.load(Ordering::Acquire);
        let recent = self.mesh_auto.at.is_some_and(|t| t.elapsed() < Duration::from_secs(8));
        let hung = self.mesh_auto.at.is_some_and(|t| t.elapsed() >= Duration::from_secs(15));
        if busy && !hung {
            return;
        }
        if recent && !hung {
            return;
        }
        let op = match self.mesh_auto.operator.clone() {
            Some(op) => op,
            None => match crate::mesh_key::load_or_create() {
                Ok(op) => {
                    self.mesh_auto.operator = Some(op.clone());
                    op
                }
                Err(e) => {
                    self.set_mesh_note(&e);
                    return;
                }
            },
        };
        self.mesh_auto.at = Some(Instant::now());
        self.mesh_auto.busy.store(true, Ordering::Release);
        self.set_mesh_note("registering a mesh key");
        let url = format!("{host}/mesh/enroll");
        let shared = Arc::clone(&self.shared);
        let epoch = epoch_of(&shared);
        let busy = Arc::clone(&self.mesh_auto.busy);
        let ctx = ctx.clone();
        let host = host.to_string();
        ehttp::fetch(ehttp::Request::get(url), move |res| {
            let fail = |shared: &Arc<Mutex<Shared>>, epoch, busy: &Arc<AtomicBool>, ctx: &eframe::egui::Context, note: String, unsupported: bool| {
                apply(shared, epoch, |sh| {
                    sh.mesh_note = note;
                    if unsupported {
                        sh.mesh_unsupported = true;
                        sh.node_facts = Some(Err(sh.mesh_note.clone()));
                    }
                });
                busy.store(false, Ordering::Release);
                ctx.request_repaint();
            };
            let nonce = match &res {
                Ok(r) => match challenge_reply(r.status) {
                    ChallengeReply::Sign => serde_json::from_slice::<serde_json::Value>(&r.bytes).ok().and_then(|v| v["nonce"].as_str().map(str::to_string)),
                    ChallengeReply::Unsupported => {
                        fail(&shared, epoch, &busy, &ctx, "this host does not register mesh keys yet".into(), true);
                        return;
                    }
                    ChallengeReply::Forbidden => {
                        let why = serde_json::from_slice::<serde_json::Value>(&r.bytes).ok().and_then(|v| v["error"].as_str().map(str::to_string));
                        fail(&shared, epoch, &busy, &ctx, why.unwrap_or_else(|| "the mesh registers a key on a tailnet or local connection".into()), false);
                        return;
                    }
                    ChallengeReply::Other => {
                        fail(&shared, epoch, &busy, &ctx, format!("mesh enroll: HTTP {}", r.status), false);
                        return;
                    }
                },
                Err(e) => {
                    fail(&shared, epoch, &busy, &ctx, format!("mesh enroll: {e}"), false);
                    return;
                }
            };
            let Some(nonce) = nonce else {
                fail(&shared, epoch, &busy, &ctx, "mesh enroll: no challenge".into(), false);
                return;
            };
            let body = serde_json::json!({"pub": op.pub_hex(), "nonce": nonce, "sig": op.sign_nonce(&nonce)}).to_string();
            let mut req = ehttp::Request::post(format!("{host}/mesh/enroll"), body.into_bytes());
            post_headers(&mut req, "");
            let shared2 = Arc::clone(&shared);
            let busy2 = Arc::clone(&busy);
            let ctx2 = ctx.clone();
            let host2 = host.clone();
            ehttp::fetch(req, move |res| {
                match &res {
                    Ok(r) if r.ok => {
                        let token = serde_json::from_slice::<serde_json::Value>(&r.bytes).ok().and_then(|v| v["token"].as_str().map(str::to_string));
                        if let Some(token) = token {
                            let _ = crate::mesh_key::remember_session(&host2, &token);
                            apply(&shared2, epoch, |sh| {
                                sh.mesh_token = token;
                                sh.mesh_note.clear();
                            });
                        } else {
                            apply(&shared2, epoch, |sh| sh.mesh_note = "mesh enroll: no session".into());
                        }
                    }
                    Ok(r) => {
                        let why = serde_json::from_slice::<serde_json::Value>(&r.bytes).ok().and_then(|v| v["error"].as_str().map(str::to_string));
                        apply(&shared2, epoch, |sh| sh.mesh_note = why.unwrap_or_else(|| format!("mesh enroll: HTTP {}", r.status)));
                    }
                    Err(e) => {
                        apply(&shared2, epoch, |sh| sh.mesh_note = format!("mesh enroll: {e}"));
                    }
                }
                busy2.store(false, Ordering::Release);
                ctx2.request_repaint();
            });
        });
    }

    fn maybe_maintain(&mut self, ctx: &eframe::egui::Context, host: &str) {
        let due = self.mesh_auto.maintain_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(60));
        if !due || self.mesh_auto.busy.load(Ordering::Acquire) {
            return;
        }
        self.mesh_auto.maintain_at = Some(Instant::now());
        let mut req = ehttp::Request::post(format!("{host}/mesh/maintain"), b"{}".to_vec());
        post_headers(&mut req, &self.s.token);
        let shared = Arc::clone(&self.shared);
        let epoch = epoch_of(&shared);
        let ctx = ctx.clone();
        ehttp::fetch(req, move |res| {
            if let Ok(r) = &res
                && r.status == 401
            {
                apply(&shared, epoch, |sh| {
                    sh.mesh_rejected = true;
                    sh.mesh_note = "the host refused the mesh key; registering again".into();
                });
            }
            ctx.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::challenge_reply;
    use super::ChallengeReply;

    #[test]
    fn an_old_host_token_guard_is_not_a_retry() {
        assert_eq!(challenge_reply(401), ChallengeReply::Unsupported);
        assert_eq!(challenge_reply(404), ChallengeReply::Unsupported);
        assert_eq!(challenge_reply(200), ChallengeReply::Sign);
        assert_eq!(challenge_reply(403), ChallengeReply::Forbidden);
        assert_eq!(challenge_reply(500), ChallengeReply::Other);
    }
}
