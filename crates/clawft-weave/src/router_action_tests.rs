//! The `route` action against temp dirs only: a manifest store, an overlays
//! directory, a project with its own `ports.yaml`, and a router handle built
//! on that source (no listener). Never `~/.weftos`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::router_cfg::RouterConfig;
use crate::router_sources::{Candidate, DirsSource, PORTS_FILE};

const A: &str = "01K00000000000000000000000";
const B: &str = "01K00000000000000000000001";
const OTHER: &str = "01K0000000000000000000000Z";

struct Lab {
    tmp: tempfile::TempDir,
    manifests: PathBuf,
    overlays: PathBuf,
}

fn manifest(dir: &Path, ulid: &str, name: &str, root: &Path) {
    let toml = format!(
        "schema = 1\nid = \"{ulid}\"\nname = \"{name}\"\nroot = {root:?}\nstate = \"active\"\n\
         created = \"2026-10-07T02:48:45Z\"\nlast_seen = \"2026-10-07T02:48:46Z\"\nproject_toml = \"present\"\n",
        root = root.display().to_string()
    );
    std::fs::write(dir.join(format!("{ulid}.toml")), toml).unwrap();
}

fn lab() -> Lab {
    let tmp = tempfile::tempdir().unwrap();
    let manifests = tmp.path().join("projects");
    let overlays = tmp.path().join("routes");
    std::fs::create_dir_all(&manifests).unwrap();
    let a = tmp.path().join("app");
    std::fs::create_dir_all(a.join("compose")).unwrap();
    std::fs::write(a.join(PORTS_FILE), "project: app\nroutes:\n  - { prefix: /app, port: 3000 }\n").unwrap();
    let b = tmp.path().join("beta");
    std::fs::create_dir_all(&b).unwrap();
    manifest(&manifests, A, "app", &a);
    manifest(&manifests, B, "beta", &b);
    Lab { tmp, manifests, overlays }
}

impl Lab {
    fn router(&self) -> Arc<RouterHandle> {
        let source = DirsSource {
            candidates: vec![
                Candidate { name: "app".into(), dir: self.tmp.path().join("app"), ulid: Some(A.into()) },
                Candidate { name: "beta".into(), dir: self.tmp.path().join("beta"), ulid: Some(B.into()) },
            ],
            overlays: Some(self.overlays.clone()),
        };
        let cfg = RouterConfig { enabled: true, ..Default::default() };
        Arc::new(RouterHandle::new(cfg, Box::new(source), "127.0.0.1:18000".parse().unwrap()))
    }

    fn env(&self, router: RouterRef) -> RouteEnv {
        RouteEnv { manifests_dir: self.manifests.clone(), overlays_dir: self.overlays.clone(), router }
    }
}

fn set(ulid: &str, prefix: &str, port: u64) -> Value {
    json!({ "op": "set", "project_ulid": ulid, "prefix": prefix, "port": port })
}

fn remove(ulid: &str, prefix: &str) -> Value {
    json!({ "op": "remove", "project_ulid": ulid, "prefix": prefix })
}

fn served(h: &RouterHandle) -> Vec<(String, u16, Source)> {
    h.table().routes.iter().map(|r| (r.prefix.clone(), r.port, r.source)).collect()
}

#[test]
fn set_writes_a_private_overlay_the_router_serves_and_replace_changes_it() {
    let lab = lab();
    let h = lab.router();
    let env = lab.env(RouterRef::Handle(h.clone()));
    let out = apply(&env, json!({ "op": "set", "project_ulid": A, "prefix": "/app-admin/", "port": 3001, "health": "/healthz", "allow": ["Alice@Example.com"] }));
    assert_eq!(out.status, "succeeded", "{:?}", out.result);
    assert_eq!(out.result, json!({ "op": "set", "prefix": "/app-admin", "applied": true }));
    let file = router_overlay::overlay_path(&lab.overlays, A);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let r = h.table().routes.iter().find(|r| r.prefix == "/app-admin").cloned().unwrap();
    assert_eq!((r.port, r.source, r.allow.clone(), r.health.clone()), (3001, Source::Dashboard, vec!["alice@example.com".into()], Some("/healthz".into())));
    assert_eq!(r.project, "app", "the overlay route belongs to the project's ports.yaml slug");

    let out = apply(&env, set(A, "/app-admin", 3002));
    assert_eq!(out.result["applied"], true);
    let r = h.table().routes.iter().find(|r| r.prefix == "/app-admin").cloned().unwrap();
    assert_eq!((r.port, r.allow.len()), (3002, 0), "replace, not merge");
    assert!(!std::fs::read_to_string(&file).unwrap().contains("3001"));
}

#[test]
fn remove_deletes_only_an_overlay_route() {
    let lab = lab();
    let h = lab.router();
    let env = lab.env(RouterRef::Handle(h.clone()));
    assert_eq!(apply(&env, set(A, "/app-admin", 3001)).result["applied"], true);
    let out = apply(&env, remove(A, "/app-admin/"));
    assert_eq!(out.result, json!({ "op": "remove", "prefix": "/app-admin", "applied": true }));
    assert_eq!(served(&h), [("/app".to_owned(), 3000, Source::Repo)]);
    let out = apply(&env, remove(A, "/app"));
    assert_eq!(out.status, "failed");
    assert!(out.result["reason"].as_str().unwrap().contains("no dashboard route /app"), "{}", out.result);
    assert_eq!(out.result["applied"], false);
    assert_eq!(served(&h), [("/app".to_owned(), 3000, Source::Repo)], "the repository route is untouched");
    assert!(std::fs::read_to_string(lab.tmp.path().join("app").join(PORTS_FILE)).unwrap().contains("port: 3000"));
}

#[test]
fn an_unregistered_ulid_a_bad_payload_and_bad_values_are_refused_before_anything_is_written() {
    let lab = lab();
    let h = lab.router();
    let env = lab.env(RouterRef::Handle(h.clone()));
    let gen0 = h.generation();
    let cases: Vec<(Value, &str)> = vec![
        (set(OTHER, "/x", 3001), "not registered"),
        (set("not-a-ulid", "/x", 3001), "not a ULID"),
        (json!({ "op": "set", "project_ulid": A, "prefix": "/x", "port": 3001, "extra": 1 }), "unknown field"),
        (json!({ "op": "rename", "project_ulid": A, "prefix": "/x" }), "op must be"),
        (set(A, "/Bad Prefix", 3001), "[a-z0-9-]"),
        (set(A, "/api/x", 3001), "reserved"),
        (set(A, "/x", 80), "1024..=65535"),
        (json!({ "op": "set", "project_ulid": A, "prefix": "/x" }), "needs a port"),
        (json!({ "op": "set", "project_ulid": A, "prefix": "/x", "port": 3001, "health": "nope" }), "health"),
        (json!({ "op": "set", "project_ulid": A, "prefix": "/x", "port": 3001, "allow": ["nobody"] }), "not a login"),
    ];
    for (payload, needle) in cases {
        let out = apply(&env, payload.clone());
        assert_eq!(out.status, "failed", "{payload}");
        assert_eq!(out.result["applied"], false);
        assert_eq!(out.result["kind"], "route");
        let reason = out.result["reason"].as_str().unwrap();
        assert!(reason.contains(needle), "{payload}: {reason}");
    }
    assert!(!lab.overlays.exists(), "nothing was written");
    assert_eq!(h.generation(), gen0, "nothing was reloaded");
}

#[test]
fn the_repository_wins_on_its_own_prefix_and_overlays_conflict_across_projects() {
    let lab = lab();
    let h = lab.router();
    let env = lab.env(RouterRef::Handle(h.clone()));
    let out = apply(&env, set(A, "/app", 3009));
    assert_eq!(out.status, "succeeded", "the overlay is written; the router just does not serve it");
    assert_eq!(out.result["applied"], false);
    assert!(out.result["reason"].as_str().unwrap().contains("the repository wins"), "{}", out.result);
    assert_eq!(served(&h), [("/app".to_owned(), 3000, Source::Repo)]);
    let refused = &h.table().refused;
    assert_eq!(refused.len(), 1);
    assert_eq!((refused[0].source, refused[0].project.as_str(), refused[0].port), (Source::Dashboard, "app", 3009));

    assert_eq!(apply(&env, set(A, "/shared", 3010)).result["applied"], true);
    let out = apply(&env, set(B, "/shared", 3011));
    assert_eq!(out.result["applied"], false);
    assert_eq!(out.result["reason"], "prefix /shared is already routed by project app");
    let out = apply(&env, set(B, "/beta-only", 3010));
    assert_eq!(out.result["applied"], false, "a port another project routes is refused too");
    assert!(out.result["reason"].as_str().unwrap().contains("port 3010 is already routed by project app"), "{}", out.result);
    assert_eq!(apply(&env, set(B, "/beta-only", 3012)).result["applied"], true);
    assert_eq!(h.table().routes.iter().find(|r| r.prefix == "/beta-only").unwrap().project, "beta", "the manifest name is the slug without a ports.yaml");
}

#[test]
fn with_the_router_off_the_overlay_is_saved_and_the_result_says_so() {
    let lab = lab();
    let env = lab.env(RouterRef::Off);
    let out = apply(&env, set(A, "/later", 3001));
    assert_eq!(out.status, "succeeded");
    assert_eq!(out.result["applied"], false);
    assert!(out.result["reason"].as_str().unwrap().contains("router is off"), "{}", out.result);
    assert!(router_overlay::overlay_path(&lab.overlays, A).is_file());
}

#[tokio::test]
async fn the_handler_answers_the_dashboard_with_the_contract_result() {
    use crate::dashboard_test_support::*;
    let lab = lab();
    let h = lab.router();
    let fake = FakeDash::start().await;
    let action = json!({ "id": "rt1", "kind": "route", "payload": set(A, "/from-dash", 3001) });
    fake.answer("/api/nodes/heartbeat", &[(200, &json!({ "ok": true, "actions": [action] }).to_string())]);
    fake.answer("/api/nodes/actions/rt1/result", &[(200, "{}")]);
    let (_d, path) = token_dir(&token('a'));
    let dash = dashboard(config(&fake.url, &path));
    dash.set_action_handler(Arc::new(RouteHandler::new(lab.env(RouterRef::Handle(h.clone())))));
    dash.heartbeat_once().await;
    let posted: Vec<Value> = fake.requests("/api/nodes/actions/rt1/result").iter().map(|r| serde_json::from_str(&r.body).unwrap()).collect();
    assert_eq!(posted[0], json!({ "status": "running" }));
    assert_eq!(posted[1], json!({ "status": "succeeded", "result": { "op": "set", "prefix": "/from-dash", "applied": true } }));
    assert!(served(&h).iter().any(|(p, port, s)| p == "/from-dash" && *port == 3001 && *s == Source::Dashboard));
}
