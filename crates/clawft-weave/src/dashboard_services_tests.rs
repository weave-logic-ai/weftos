//! `report.services` against a canned process-compose `/processes` answer on
//! loopback, temp project directories and the fake dashboard. Never a real
//! process-compose or `~/.weftos`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use super::*;
use crate::dashboard_test_support::*;
use crate::router_sources::{Candidate, DirsSource, PORTS_FILE};

/// A fake process-compose API answering `body` to every request.
async fn pc_api(body: String) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = s.write_all(resp.as_bytes()).await;
            let _ = s.shutdown().await;
        }
    });
    port
}

fn canned() -> String {
    // process-compose shape, plus fields the report must never copy.
    json!({"data": [
        {"name": "shasta-field", "namespace": "default", "status": "Running", "is_ready": "Ready", "restarts": 2, "pid": 4242,
         "command": "node server.js --token=do-not-send", "env": {"API_SECRET": "do-not-send"}},
        {"name": "Shasta_API", "status": "Running", "restarts": 0},
        {"name": "worker", "status": "Completed"}
    ]})
    .to_string()
}

fn project(dir: &std::path::Path, name: &str, yaml: &str, ulid: Option<&str>) -> Candidate {
    let p = dir.join(name);
    std::fs::create_dir_all(p.join("compose")).unwrap();
    std::fs::write(p.join(PORTS_FILE), yaml).unwrap();
    Candidate { name: name.into(), dir: p, ulid: ulid.map(str::to_owned) }
}

#[tokio::test]
async fn processes_are_listed_with_claimed_ports_and_nothing_sensitive() {
    let pc = pc_api(canned()).await;
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let tmp = tempfile::tempdir().unwrap();
    let mut shasta = format!("project: shasta\nclaims:\n  - {{ port: {pc}, use: process-compose-http }}\n");
    shasta += "  - { port: 18120, use: shasta-field }\n  - { port: 18121, use: shasta-field }\n  - { port: 18113, use: shasta-api }\n";
    shasta += "routes:\n  - { prefix: /shastaos, port: 18120 }\n";
    let candidates = vec![
        project(tmp.path(), "shasta", &shasta, Some(ULID)),
        project(tmp.path(), "sleepy", &format!("project: sleepy\nclaims:\n  - {{ port: {dead}, use: process-compose-http }}\n"), None),
        project(tmp.path(), "quiet", "project: quiet\nroutes:\n  - { prefix: /quiet, port: 3000 }\n", None),
    ];
    let src = ProjectServices::new(Arc::new(DirsSource::new(candidates)), Duration::from_millis(500));
    let list = src.services().await;
    assert_eq!(list.len(), 2, "a project without a process-compose claim is not listed: {list:?}");
    let s = &list[0];
    assert_eq!(s["project"], "shasta");
    assert_eq!(s["project_ulid"], ULID);
    assert_eq!(s["pc_port"], pc);
    assert_eq!(s["state"], "ok");
    let procs = s["processes"].as_array().unwrap();
    assert_eq!(procs.len(), 3);
    assert_eq!(procs[0], json!({"name": "shasta-field", "status": "Running", "ports": [18120, 18121], "restarts": 2}));
    assert_eq!(procs[1]["ports"], json!([18113]), "use matches the process name loosely (case, _ vs -)");
    assert_eq!(procs[2], json!({"name": "worker", "status": "Completed", "ports": [], "restarts": 0}));
    let text = s.to_string();
    assert!(!text.contains("do-not-send") && !text.contains("command") && !text.contains("env") && !text.contains("4242"), "{text}");
    let u = &list[1];
    assert_eq!((u["project"].clone(), u["state"].clone(), u["project_ulid"].clone()), (json!("sleepy"), json!("unreachable"), Value::Null));
    assert_eq!(u["processes"], json!([]));
}

#[tokio::test]
async fn projects_and_processes_are_bounded() {
    let many: Vec<Value> = (0..(MAX_PROCESSES + 8)).map(|i| json!({"name": format!("p{i}"), "status": "Running"})).collect();
    let pc = pc_api(json!({"data": many}).to_string()).await;
    let tmp = tempfile::tempdir().unwrap();
    let candidates: Vec<Candidate> = (0..(MAX_PROJECTS + 3))
        .map(|i| project(tmp.path(), &format!("proj{i}"), &format!("project: proj{i}\nclaims:\n  - {{ port: {pc}, use: process-compose-http }}\n"), None))
        .collect();
    let src = ProjectServices::new(Arc::new(DirsSource::new(candidates)), Duration::from_millis(500));
    let list = src.services().await;
    assert_eq!(list.len(), MAX_PROJECTS);
    assert_eq!(list[0]["project"], "proj0");
    assert_eq!(list[0]["processes"].as_array().unwrap().len(), MAX_PROCESSES);
}

#[tokio::test]
async fn the_heartbeat_carries_services_only_when_there_is_something_to_report() {
    let pc = pc_api(canned()).await;
    let tmp = tempfile::tempdir().unwrap();
    let with = vec![project(tmp.path(), "app", &format!("project: app\nclaims:\n  - {{ port: {pc}, use: process-compose-http }}\n"), Some(ULID))];
    let without = vec![project(tmp.path(), "bare", "project: bare\n", None)];
    for (candidates, expect) in [(with, true), (without, false)] {
        let fake = FakeDash::start().await;
        fake.answer("/api/nodes/heartbeat", &[(200, "{}")]);
        let (_d, path) = token_dir(&token('a'));
        let dash = dashboard(config(&fake.url, &path));
        dash.set_service_source(Arc::new(ProjectServices::new(Arc::new(DirsSource::new(candidates)), Duration::from_millis(500))));
        dash.heartbeat_once().await;
        let report = serde_json::from_str::<Value>(&fake.requests("/api/nodes/heartbeat")[0].body).unwrap()["report"].clone();
        assert_eq!(report.get("services").is_some(), expect, "{report}");
        if expect {
            assert_eq!(report["services"][0]["processes"][0]["name"], "shasta-field");
        }
    }
}
