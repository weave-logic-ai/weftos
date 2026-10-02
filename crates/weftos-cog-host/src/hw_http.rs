//! The `/hw/*` routes (USB inventory, baseline, agent identify, Hardware Dex), split out of the
//! binary's `http.rs` so they are unit-testable. CSRF checks for every POST run in `http.rs`; the
//! routes here (reads included: they describe attached hardware) require the host bearer token.
//!
//!   GET  /hw/usb            (token) read-only inventory vs baseline + id-table labels (serials redacted)
//!   POST /hw/usb/scan       same report, but records sightings in the dex (catches, times_seen)
//!   POST /hw/usb/baseline   accept the current scan (or body {keys:[..]}) as known
//!   POST /hw/usb/identify   body {key}; ask the configured agent what the device is
//!   GET  /hw/dex            (token) Hardware Dex: caught, unseen_catches, wild, totals, badges, types
//!   POST /hw/dex/catch      body {key, catalog_id|"wild", name?, answer?, force?}
//!   POST /hw/dex/ack        body {refs?}; acknowledge NEW CATCH banners

use crate::auth::{Headers, Policy};
use crate::{dex, network, usb, usb_identify};
use serde_json::{json, Value};
use std::path::Path;

/// Body cap for `/hw/*` requests (the generic cap is far larger, for cog uploads).
pub const HW_BODY_CAP: usize = 64 * 1024;

pub struct Req<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub headers: &'a Headers,
    pub body: &'a [u8],
}

type Resp = (&'static str, String);

fn resp(code: &'static str, v: Value) -> Resp {
    (code, v.to_string())
}

fn err(code: &'static str, msg: impl Into<String>) -> Resp {
    resp(code, json!({ "ok": false, "error": msg.into() }))
}

fn parse_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Resp> {
    if body.len() > HW_BODY_CAP {
        return Err(err("413 Payload Too Large", "body too large"));
    }
    serde_json::from_slice(body).map_err(|e| err("400 Bad Request", format!("bad request body: {e}")))
}

/// An optional JSON body: empty means "defaults".
fn parse_opt<T: serde::de::DeserializeOwned + Default>(body: &[u8]) -> Result<T, Resp> {
    if body.iter().all(u8::is_ascii_whitespace) {
        Ok(T::default())
    } else {
        parse_body(body)
    }
}

/// `Some(response)` for a `/hw/*` route, `None` when the path is not one of ours.
pub fn handle(req: &Req, root: &Path, policy: &Policy) -> Option<Resp> {
    let parts: Vec<&str> = req.path.split('?').next().unwrap_or("").trim_matches('/').split('/').filter(|p| !p.is_empty()).collect();
    Some(match (req.method, parts.as_slice()) {
        ("GET", ["hw", "usb"]) => guarded(req, policy, || usb_get(root)),
        ("GET", ["hw", "dex"]) => guarded(req, policy, || dex_get(root)),
        ("POST", ["hw", "usb", "scan"]) => guarded(req, policy, || usb_scan(root)),
        ("POST", ["hw", "usb", "baseline"]) => guarded(req, policy, || baseline(req.body, root)),
        ("POST", ["hw", "usb", "identify"]) => guarded(req, policy, || identify(req.body, root)),
        ("POST", ["hw", "dex", "catch"]) => guarded(req, policy, || dex_catch(req.body, root)),
        ("POST", ["hw", "dex", "ack"]) => guarded(req, policy, || dex_ack(req.body, root)),
        _ => return None,
    })
}

fn guarded(req: &Req, policy: &Policy, f: impl FnOnce() -> Resp) -> Resp {
    match policy.check_token(req.headers) {
        Ok(()) => f(),
        Err(r) => r,
    }
}

fn node_now() -> (String, u64) {
    (network::node_name(), usb::now_secs())
}

fn usb_get(root: &Path) -> Resp {
    let (node, now) = node_now();
    let scan = usb::scan(&usb::load_salt(root));
    let d = dex::read_dex(root);
    let table = dex::id_table().with_user_rows(d.user_rows(dex::catalog()));
    let mut rep = usb::report(root, &scan, &table, &node, now);
    rep["unseen_catches"] = Value::Array(d.unseen_json(dex::catalog()));
    resp("200 OK", rep)
}

fn usb_scan(root: &Path) -> Resp {
    let (node, now) = node_now();
    let scan = usb::scan(&usb::load_salt(root));
    let res = dex::with_dex(root, |d| {
        let table = dex::id_table().with_user_rows(d.user_rows(dex::catalog()));
        d.record_scan(&scan.devices, &table, dex::catalog(), &node, now);
        (table, d.unseen_json(dex::catalog()))
    });
    match res {
        Ok((table, unseen)) => {
            let mut rep = usb::report(root, &scan, &table, &node, now);
            rep["unseen_catches"] = Value::Array(unseen);
            resp("200 OK", rep)
        }
        Err(e) => err("500 Internal Server Error", format!("dex: {e}")),
    }
}

fn baseline(body: &[u8], root: &Path) -> Resp {
    #[derive(serde::Deserialize, Default)]
    struct R {
        keys: Option<Vec<String>>,
    }
    let req: R = match parse_opt(body) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let scan = usb::scan(&usb::load_salt(root));
    match usb::save_baseline(root, &scan, req.keys.as_deref(), usb::now_secs()) {
        Ok(n) => resp("200 OK", json!({ "ok": true, "devices": n })),
        Err(e) => err("500 Internal Server Error", format!("write baseline: {e}")),
    }
}

fn identify(body: &[u8], root: &Path) -> Resp {
    #[derive(serde::Deserialize)]
    struct R {
        key: String,
    }
    let req: R = match parse_body(body) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let Some(dev) = usb::scan(&usb::load_salt(root)).devices.into_iter().find(|d| d.key == req.key) else {
        return err("404 Not Found", "device not attached (rescan)");
    };
    let (ok, v) = usb_identify::identify(&dev, dex::id_table());
    resp(if ok { "200 OK" } else { "503 Service Unavailable" }, v)
}

fn dex_get(root: &Path) -> Resp {
    let node = network::node_name();
    resp("200 OK", dex::read_dex(root).report(dex::catalog(), dex::id_table(), &node))
}

fn dex_catch(body: &[u8], root: &Path) -> Resp {
    #[derive(serde::Deserialize)]
    struct R {
        key: String,
        catalog_id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        answer: String,
        #[serde(default)]
        force: bool,
    }
    let req: R = match parse_body(body) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let Some(dev) = usb::scan(&usb::load_salt(root)).devices.into_iter().find(|d| d.key == req.key) else {
        return err("404 Not Found", "device not attached (rescan)");
    };
    let (node, now) = node_now();
    let rr = dex::RegisterReq { catalog_id: &req.catalog_id, name: &req.name, answer: &req.answer, force: req.force };
    match dex::with_dex(root, |d| d.register(&dev, &rr, dex::id_table(), dex::catalog(), &node, now)) {
        Ok(Ok(v)) => resp("200 OK", json!({ "ok": true, "catch": v })),
        Ok(Err(e)) => err("400 Bad Request", e),
        Err(e) => err("500 Internal Server Error", format!("dex: {e}")),
    }
}

fn dex_ack(body: &[u8], root: &Path) -> Resp {
    #[derive(serde::Deserialize, Default)]
    struct R {
        refs: Option<Vec<String>>,
    }
    let req: R = match parse_opt(body) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match dex::with_dex(root, |d| d.ack(req.refs.as_deref())) {
        Ok(()) => resp("200 OK", json!({ "ok": true })),
        Err(e) => err("500 Internal Server Error", format!("dex: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol() -> Policy {
        Policy::new("tok".into(), vec![])
    }
    fn hdr(auth: bool) -> Headers {
        let mut h = Headers::new();
        if auth {
            h.insert("authorization".into(), "Bearer tok".into());
        }
        h
    }
    fn call(method: &str, path: &str, body: &str, auth: bool, root: &Path) -> Resp {
        let h = hdr(auth);
        handle(&Req { method, path, headers: &h, body: body.as_bytes() }, root, &pol()).expect("a /hw route")
    }

    #[test]
    fn every_mutating_route_refuses_without_the_token() {
        let d = tempfile::tempdir().unwrap();
        for path in ["/hw/usb/scan", "/hw/usb/baseline", "/hw/usb/identify", "/hw/dex/catch", "/hw/dex/ack"] {
            let (code, body) = call("POST", path, "{}", false, d.path());
            assert_eq!(code, "401 Unauthorized", "{path}");
            assert!(body.contains("host token required"));
        }
        assert!(!dex::path(d.path()).exists() && !usb::baseline_path(d.path()).exists());
    }

    #[test]
    fn reads_need_the_token_and_are_read_only() {
        let d = tempfile::tempdir().unwrap();
        for p in ["/hw/usb", "/hw/dex"] {
            assert_eq!(call("GET", p, "", false, d.path()).0, "401 Unauthorized", "{p}");
            let (code, body) = call("GET", p, "", true, d.path());
            assert_eq!(code, "200 OK");
            assert!(body.contains("\"ok\":true"));
        }
        assert!(!dex::path(d.path()).exists(), "GET must not create or mutate the dex");
        assert!(!usb::baseline_path(d.path()).exists());
    }

    #[test]
    fn scan_post_records_and_ack_clears() {
        let d = tempfile::tempdir().unwrap();
        let (code, body) = call("POST", "/hw/usb/scan", "", true, d.path());
        assert_eq!(code, "200 OK");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["unseen_catches"].is_array());
        assert!(dex::path(d.path()).exists());
        dex::with_dex(d.path(), |x| x.unseen.push("module:ky-038".into())).unwrap();
        let g: Value = serde_json::from_str(&call("GET", "/hw/dex", "", true, d.path()).1).unwrap();
        assert!(g["unseen_catches"].as_array().unwrap().iter().any(|c| c["ref"] == "module:ky-038"));
        assert_eq!(call("POST", "/hw/dex/ack", r#"{"refs":["module:ky-038"]}"#, true, d.path()).0, "200 OK");
        assert!(!dex::read_dex(d.path()).unseen.contains(&"module:ky-038".to_string()));
    }

    #[test]
    fn bad_bodies_and_unknown_devices_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(call("POST", "/hw/usb/identify", "not json", true, d.path()).0, "400 Bad Request");
        assert_eq!(call("POST", "/hw/usb/identify", r#"{"key":"nope"}"#, true, d.path()).0, "404 Not Found");
        assert_eq!(call("POST", "/hw/dex/catch", r#"{"key":"nope","catalog_id":"wild"}"#, true, d.path()).0, "404 Not Found");
        let big = format!(r#"{{"key":"{}"}}"#, "k".repeat(HW_BODY_CAP));
        assert_eq!(call("POST", "/hw/usb/identify", &big, true, d.path()).0, "413 Payload Too Large");
    }

    #[test]
    fn non_hw_paths_are_not_ours() {
        let d = tempfile::tempdir().unwrap();
        let h = hdr(true);
        assert!(handle(&Req { method: "GET", path: "/status", headers: &h, body: b"" }, d.path(), &pol()).is_none());
        assert!(handle(&Req { method: "GET", path: "/hw/nope", headers: &h, body: b"" }, d.path(), &pol()).is_none());
    }
}
