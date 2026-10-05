//! Request helpers shared by the client modules.

use super::*;

/// The epoch of the current host connection (see [`Shared::epoch`]).
/// True when a 404 body is the host's generic "not found": the route does not exist on this host.
/// (`/cogs/<id>/last` answers 404 with `no output yet` / `no such cog` when the route exists.)
pub(super) fn route_missing(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body).ok().and_then(|v| v["error"].as_str().map(|e| e == "not found")).unwrap_or(false)
}

pub(super) fn epoch_of(shared: &Arc<Mutex<Shared>>) -> u64 {
    shared.lock().unwrap().epoch
}

/// Run `f` on the shared state only if the console is still on the connection the fetch started in.
pub(super) fn apply(shared: &Arc<Mutex<Shared>>, epoch: u64, f: impl FnOnce(&mut Shared)) -> bool {
    let mut sh = shared.lock().unwrap();
    if sh.epoch != epoch {
        return false;
    }
    f(&mut sh);
    true
}

/// [`set`], but only while the console is still on the connection `epoch` started in.
pub(super) fn set_in(shared: &Arc<Mutex<Shared>>, epoch: u64, ctx: &eframe::egui::Context, msg: String) {
    if apply(shared, epoch, |sh| sh.last_action = Some(msg)) {
        ctx.request_repaint();
    }
}

/// The directory a `registry.json` URL lives in, used as the base for relative artifact paths.
pub(super) fn registry_base(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    match u.rsplit_once('/') {
        Some((base, _file)) => base.to_string(),
        None => u.to_string(),
    }
}

pub(super) fn base(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    if h.starts_with("http://") || h.starts_with("https://") {
        h.to_string()
    } else {
        format!("http://{h}")
    }
}

/// GET with the bearer token (the `/hw/*` reads require it).
pub(super) fn get_req(url: String, token: &str) -> ehttp::Request {
    let mut req = ehttp::Request::get(url);
    if !token.trim().is_empty() {
        req.headers.insert("authorization", format!("Bearer {}", token.trim()));
    }
    req
}

/// Headers every host POST needs: JSON content type, the `X-Weft-Host` CSRF marker and, when set,
/// the bearer token.
pub(super) fn post_headers(req: &mut ehttp::Request, token: &str) {
    req.headers.insert("content-type", "application/json");
    req.headers.insert("x-weft-host", "1");
    if !token.trim().is_empty() {
        req.headers.insert("authorization", format!("Bearer {}", token.trim()));
    }
}

/// A readable failure for a non-2xx response; 401 says where the token lives.
pub(super) fn http_err(r: &ehttp::Response) -> String {
    if r.status == 401 {
        return "this request needs a mesh key; the console registers one on a tailnet or local connection".into();
    }
    format!("HTTP {} {}", r.status, r.status_text)
}

pub(super) fn parse_json<T: for<'de> Deserialize<'de>>(res: &ehttp::Result<ehttp::Response>) -> Result<T, String> {
    match res {
        Ok(r) if r.ok => serde_json::from_slice::<T>(&r.bytes).map_err(|e| e.to_string()),
        Ok(r) => Err(http_err(r)),
        Err(e) => Err(e.clone()),
    }
}

#[cfg(test)]
mod epoch_tests {
    use super::*;

    #[test]
    fn a_reply_from_the_old_host_is_dropped_after_a_switch() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let started = epoch_of(&shared);
        assert!(apply(&shared, started, |sh| sh.last_action = Some("fresh".into())));
        // the console switches host: the epoch moves on and state is reset
        {
            let mut sh = shared.lock().unwrap();
            let epoch = sh.epoch + 1;
            *sh = Shared { epoch, ..Shared::default() };
        }
        assert!(!apply(&shared, started, |sh| sh.last_action = Some("stale".into())), "stale reply must not apply");
        assert!(shared.lock().unwrap().last_action.is_none());
        assert!(apply(&shared, epoch_of(&shared), |sh| sh.last_action = Some("new".into())));
    }

    #[test]
    fn reconnect_bumps_the_epoch() {
        let mut c = Client::new(Settings::default());
        let e0 = c.snapshot().epoch;
        c.reconnect(Settings::default());
        assert_eq!(c.snapshot().epoch, e0 + 1);
    }

    #[test]
    fn only_the_generic_not_found_means_the_route_is_missing() {
        assert!(route_missing(br#"{"ok":false,"error":"not found"}"#));
        assert!(!route_missing(br#"{"ok":false,"error":"no output yet"}"#));
        assert!(!route_missing(b"garbage"));
    }
}
