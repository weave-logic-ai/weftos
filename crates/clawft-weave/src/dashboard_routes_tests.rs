//! `report.routes` with canned `tailscale` JSON, a router handle on temp dirs
//! and the fake dashboard. Never the real `tailscale`, router or `~/.weftos`.

use std::sync::Arc;

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use super::*;
use crate::dashboard_test_support::*;
use crate::router_cfg::RouterConfig;
use crate::router_sources::{Candidate, DirsSource, PORTS_FILE};
use crate::router_state::RouterHandle;

const STATUS: &str = r#"{"Version":"1.80.0","Self":{"DNSName":"machine.example.ts.net.","HostName":"machine"}}"#;
const SERVE_ROUTER: &str = r#"{"TCP":{"443":{"HTTPS":true}},"Web":{"machine.example.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:18000"}}}}}"#;
const SERVE_OTHER: &str = r#"{"TCP":{"443":{"HTTPS":true}},"Web":{"machine.example.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:18120"}}}}}"#;

struct Canned(Result<String, String>, Result<String, String>);

impl TailscaleFacts for Canned {
    fn status_json(&self) -> Result<String, String> {
        self.0.clone()
    }
    fn serve_status_json(&self) -> Result<String, String> {
        self.1.clone()
    }
}

#[test]
fn base_url_and_served_come_from_the_canned_documents() {
    assert_eq!(base_url(STATUS).as_deref(), Some("https://machine.example.ts.net"));
    assert_eq!(base_url(r#"{"Self":{"DNSName":""}}"#), None);
    assert_eq!(base_url(r#"{"BackendState":"NeedsLogin"}"#), None);
    assert_eq!(base_url("not json"), None);
    assert_eq!(base_url(r#"{"Self":{"DNSName":"bad name.example.ts.net."}}"#), None);
    assert!(served(SERVE_ROUTER, 18000));
    assert!(!served(SERVE_ROUTER, 18001));
    assert!(!served(SERVE_OTHER, 18000));
    assert!(!served("", 18000) && !served("{}", 18000) && !served("nope", 18000));
}

/// A one-shot HTTP 200 upstream for a health probe.
async fn ok_upstream() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
            let _ = s.shutdown().await;
        }
    });
    port
}

fn handle(dir: &std::path::Path, yaml: &str) -> Arc<RouterHandle> {
    let proj = dir.join("app");
    std::fs::create_dir_all(proj.join("compose")).unwrap();
    std::fs::write(proj.join(PORTS_FILE), yaml).unwrap();
    let source = DirsSource::new(vec![Candidate { name: "app".into(), dir: proj, ulid: Some(ULID.into()) }]);
    let cfg = RouterConfig { enabled: true, health_timeout_ms: 500, ..Default::default() };
    Arc::new(RouterHandle::new(cfg, Box::new(source), "127.0.0.1:18000".parse().unwrap()))
}

async fn beat_report(src: Arc<dyn RouteReportSource>) -> Value {
    let fake = FakeDash::start().await;
    fake.answer("/api/nodes/heartbeat", &[(200, "{}")]);
    let (_d, path) = token_dir(&token('a'));
    let dash = dashboard(config(&fake.url, &path));
    dash.set_route_source(src);
    dash.heartbeat_once().await;
    serde_json::from_str::<Value>(&fake.requests("/api/nodes/heartbeat")[0].body).unwrap()["report"].clone()
}

#[tokio::test]
async fn the_heartbeat_carries_routes_with_health_restriction_source_and_ulid() {
    let up = ok_upstream().await;
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let tmp = tempfile::tempdir().unwrap();
    let mut yaml = format!("project: app\nroutes:\n  - {{ prefix: /app, port: {up}, health: /health, default: true, allow: [alice@example.com] }}\n");
    yaml += &format!("  - {{ prefix: /app/dead, port: {dead}, health: /health }}\n");
    yaml += &format!("  - {{ prefix: /app/quiet, port: {up} }}\n");
    yaml += "  - { prefix: /API/x, port: 3000 }\n";
    let h = handle(tmp.path(), &yaml);
    let src = Arc::new(RouterRoutes::new(RouterRef::Handle(h), Arc::new(Canned(Ok(STATUS.into()), Ok(SERVE_ROUTER.into())))));
    let report = beat_report(src).await;
    let routes = &report["routes"];
    assert_eq!(routes["base_url"], "https://machine.example.ts.net");
    assert_eq!(routes["served"], true);
    let items = routes["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{items:?}");
    let by = |p: &str| items.iter().find(|i| i["prefix"] == p).cloned().unwrap();
    let app = by("/app");
    assert_eq!(app["project"], "app");
    assert_eq!(app["project_ulid"], ULID);
    assert_eq!(app["port"], up);
    assert_eq!((app["default"].clone(), app["healthy"].clone(), app["restricted"].clone(), app["source"].clone()), (true.into(), true.into(), true.into(), "repo".into()));
    assert!(app.get("allow").is_none(), "the list never leaves the node: {app}");
    assert!(!routes.to_string().contains("alice"), "{routes}");
    assert_eq!(by("/app/dead")["healthy"], false);
    assert_eq!(by("/app/quiet")["healthy"], Value::Null, "no health path declared");
    assert_eq!(by("/app/quiet")["restricted"], false);
    let refused = routes["refused"].as_array().unwrap();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["prefix"], "/API/x");
    assert_eq!(refused[0]["source"], "repo");
    assert!(refused[0]["reason"].as_str().unwrap().contains("[a-z0-9-]"));
}

#[tokio::test]
async fn tailscale_unavailable_drops_base_url_and_the_router_off_drops_the_key() {
    let tmp = tempfile::tempdir().unwrap();
    let h = handle(tmp.path(), "project: app\nroutes:\n  - { prefix: /app, port: 3000 }\n");
    let src = Arc::new(RouterRoutes::new(RouterRef::Handle(h), Arc::new(Canned(Err("tailscale: not found".into()), Err("tailscale: not found".into())))));
    let report = beat_report(src).await;
    assert!(report["routes"].get("base_url").is_none(), "{}", report["routes"]);
    assert_eq!(report["routes"]["served"], false);
    assert_eq!(report["routes"]["items"][0]["prefix"], "/app");
    let off = Arc::new(RouterRoutes::new(RouterRef::Off, Arc::new(Canned(Ok(STATUS.into()), Ok(SERVE_ROUTER.into())))));
    let report = beat_report(off).await;
    assert!(report.get("routes").is_none(), "{report}");
}

#[tokio::test]
async fn items_and_refusals_are_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let mut yaml = String::from("project: app\nroutes:\n");
    for i in 0..MAX_ROUTES {
        yaml += &format!("  - {{ prefix: /r{i}, port: {} }}\n", 3000 + i);
    }
    // A second project supplies more routes and the refusals (32 entries in all,
    // so none is dropped by the per-project cap).
    let other = tmp.path().join("other");
    std::fs::create_dir_all(other.join("compose")).unwrap();
    let mut y2 = String::from("project: other\nroutes:\n");
    let bad = MAX_REFUSED + 4;
    for i in 0..(MAX_ROUTES - bad) {
        y2 += &format!("  - {{ prefix: /o{i}, port: {} }}\n", 4000 + i);
    }
    for i in 0..bad {
        y2 += &format!("  - {{ prefix: /Bad{i}, port: {} }}\n", 5000 + i);
    }
    std::fs::write(other.join(PORTS_FILE), y2).unwrap();
    let proj = tmp.path().join("app");
    std::fs::create_dir_all(proj.join("compose")).unwrap();
    std::fs::write(proj.join(PORTS_FILE), yaml).unwrap();
    let source = DirsSource::new(vec![
        Candidate { name: "app".into(), dir: proj, ulid: None },
        Candidate { name: "other".into(), dir: other, ulid: None },
    ]);
    let cfg = RouterConfig { enabled: true, health_timeout_ms: 100, ..Default::default() };
    let h = Arc::new(RouterHandle::new(cfg, Box::new(source), "127.0.0.1:18000".parse().unwrap()));
    assert!(h.table().routes.len() > MAX_ITEMS && h.table().refused.len() > MAX_REFUSED);
    let src = RouterRoutes::new(RouterRef::Handle(h), Arc::new(Canned(Ok(STATUS.into()), Ok(SERVE_ROUTER.into()))));
    let doc = src.routes().await.unwrap();
    assert_eq!(doc["items"].as_array().unwrap().len(), MAX_ITEMS);
    assert_eq!(doc["refused"].as_array().unwrap().len(), MAX_REFUSED);
    assert_eq!(doc["items"][0]["project_ulid"], Value::Null, "a candidate without a manifest has no ULID");
}

const MAX_ROUTES: usize = crate::router_routes::MAX_ROUTES_PER_PROJECT;
