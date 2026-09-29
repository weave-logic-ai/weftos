//! The Seed adapter under [`WorkloadHost`], against a stateful mock Seed
//! (wiremock, real reqwest transport): installs, starts, console runs and
//! stops change the mock's state the way the firmware does, so the tests
//! check what ends up running on the device and what the chain recorded.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::host::{RUNTIME_CHAIN_SOURCE, WorkloadHost};
use super::host_contract::HostContract;
use super::seed::{SEED_CONCURRENCY_CAP, SeedApiRuntime, SeedConfig, SeedPin};
use super::seed_http::HttpSeedTransport;
use super::seed_tls::SeedTls;
use super::test_support::MemoryCredentials;
use super::types::{RunMode, RuntimeError, VerifiedWorkload, WorkloadConfig};
use crate::chain::ChainManager;
use crate::workload_governance::{
    NetworkPolicy, NodeTrustTier, PackageTrust, WorkloadGate, WorkloadPermitRule,
};

const TOKEN: &str = "seed-token-0123456789abcdef";
const NODE: &str = "seed-01";

/// What the mock Seed has installed, and every mutating call it received.
#[derive(Default)]
struct SeedState {
    apps: Mutex<Vec<(String, bool)>>,
    calls: Mutex<Vec<String>>,
    console_fails: bool,
    /// Operations (`stop`, `uninstall`, ...) that answer 500.
    failing: Mutex<Vec<&'static str>>,
}

impl SeedState {
    fn with(apps: &[(&str, bool)], console_fails: bool) -> Arc<Self> {
        Arc::new(Self {
            apps: Mutex::new(apps.iter().map(|(a, r)| (a.to_string(), *r)).collect()),
            calls: Mutex::new(Vec::new()),
            console_fails,
            failing: Mutex::new(Vec::new()),
        })
    }
    fn fail(&self, ops: &[&'static str]) {
        *self.failing.lock().unwrap() = ops.to_vec();
    }
    fn running(&self) -> Vec<String> {
        let apps = self.apps.lock().unwrap();
        apps.iter().filter(|a| a.1).map(|a| a.0.clone()).collect()
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn set(&self, id: &str, running: bool) {
        for a in self.apps.lock().unwrap().iter_mut().filter(|a| a.0 == id) {
            a.1 = running;
        }
    }
}

fn cog_of(r: &Request) -> String {
    r.url.path().split('/').nth(4).unwrap_or("").to_string()
}

struct Handler(Arc<SeedState>, &'static str);

impl Respond for Handler {
    fn respond(&self, r: &Request) -> ResponseTemplate {
        let st = &self.0;
        let ok = ResponseTemplate::new(200).set_body_json(json!({"ok": true}));
        if st.failing.lock().unwrap().contains(&self.1) {
            st.calls
                .lock()
                .unwrap()
                .push(format!("failed-{}:{}", self.1, cog_of(r)));
            return ResponseTemplate::new(500);
        }
        if self.1 != "list" && self.1 != "install" {
            st.calls
                .lock()
                .unwrap()
                .push(format!("{}:{}", self.1, cog_of(r)));
        }
        match self.1 {
            "list" => {
                let apps = st.apps.lock().unwrap();
                let installed: Vec<Value> = apps
                    .iter()
                    .map(|(id, run)| json!({"id": id, "version": "1.0.0", "running": run}))
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({"installed": installed}))
            }
            "install" => {
                let body: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
                let id = body["id"].as_str().unwrap_or("").to_string();
                st.calls.lock().unwrap().push(format!("installed:{id}"));
                // Installed cogs auto-start on the Seed.
                st.apps.lock().unwrap().push((id, true));
                ok
            }
            "uninstall" => {
                let id = cog_of(r);
                st.apps.lock().unwrap().retain(|a| a.0 != id);
                ok
            }
            "stop" => {
                st.set(&cog_of(r), false);
                ok
            }
            "start" => {
                st.set(&cog_of(r), true);
                ok
            }
            _ if st.console_fails => ResponseTemplate::new(500),
            _ if !st.running().is_empty() => {
                ResponseTemplate::new(409).set_body_json(json!({"error": "UDP 5006 in use"}))
            }
            _ => ResponseTemplate::new(200).set_body_json(json!({
                "output": ["{\"status\":\"quiet\",\"z_impact\":0.7}"], "exit_code": 0})),
        }
    }
}

async fn mock_seed(st: &Arc<SeedState>) -> MockServer {
    let s = MockServer::start().await;
    let h = |k| Handler(st.clone(), k);
    Mock::given(method("GET"))
        .and(path("/api/v1/apps"))
        .respond_with(h("list"))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/apps/available"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"cogs": [
            {"id": "fall-detect", "version": "1.0.0"}, {"id": "baby-cry", "version": "1.0.0"}]})))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/v1/apps/[a-z0-9-]+/logs$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"output": []})))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/apps/install"))
        .respond_with(h("install"))
        .mount(&s)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(r"^/api/v1/apps/[a-z0-9-]+$"))
        .respond_with(h("uninstall"))
        .mount(&s)
        .await;
    for op in ["stop", "start", "console"] {
        Mock::given(method("POST"))
            .and(path_regex(format!(r"^/api/v1/apps/[a-z0-9-]+/{op}$")))
            .respond_with(h(op))
            .mount(&s)
            .await;
    }
    s
}

fn seed_rt(server: &MockServer) -> Arc<SeedApiRuntime> {
    Arc::new(
        SeedApiRuntime::new(
            SeedConfig {
                node_id: NODE.into(),
                pins: vec![
                    SeedPin::new("fall-detect", "1.0.0"),
                    SeedPin::new("baby-cry", "1.0.0"),
                ],
                concurrency_cap: SEED_CONCURRENCY_CAP,
            },
            Arc::new(HttpSeedTransport::new(&server.uri(), SeedTls::WebPki).unwrap()),
            Arc::new(MemoryCredentials::with(NODE, TOKEN)),
        )
        .unwrap(),
    )
}

fn rule(actions: &[&str]) -> WorkloadPermitRule {
    let mut r = WorkloadPermitRule::new("seed", actions.iter().copied(), ["cog"]);
    r.min_package_trust = PackageTrust::OperatorAttested;
    r.max_network = NetworkPolicy::Egress;
    r
}

fn host(rt: Arc<SeedApiRuntime>, r: WorkloadPermitRule) -> (WorkloadHost, Arc<ChainManager>) {
    let chain = Arc::new(ChainManager::new(0, 1000));
    let gate = WorkloadGate::new(0.8, false)
        .with_chain(chain.clone())
        .with_permit(r)
        .unwrap();
    let h = WorkloadHost::new(rt, Arc::new(gate), "operator", NodeTrustTier::Paired)
        .with_chain(chain.clone());
    (h, chain)
}

fn runtime_events(cm: &ChainManager) -> Vec<(String, String, String)> {
    cm.tail(0)
        .into_iter()
        .filter(|e| e.source == RUNTIME_CHAIN_SOURCE)
        .map(|e| {
            let p = e.payload.unwrap_or(Value::Null);
            let s = |k: &str| p[k].as_str().unwrap_or("").to_string();
            (e.kind, s("workload_id"), s("phase"))
        })
        .collect()
}

fn cfg() -> WorkloadConfig {
    WorkloadConfig {
        mode: RunMode::Listener,
        args: vec![],
        host: HostContract::default_feed(),
        node_id: NODE.into(),
    }
}

fn fall_detect() -> VerifiedWorkload {
    VerifiedWorkload::store_pin("cognitum", "fall-detect", "1.0.0", None).unwrap()
}

#[tokio::test]
async fn console_without_a_stop_permit_leaves_the_operators_cogs_running() {
    // baby-cry was installed and started by the operator, not WeftOS.
    let st = SeedState::with(&[("fall-detect", false), ("baby-cry", true)], false);
    let server = mock_seed(&st).await;
    let (h, chain) = host(
        seed_rt(&server),
        rule(&["workload.install", "workload.load", "workload.start"]),
    );
    let inst = h.load(&fall_detect(), &cfg()).await.unwrap();
    let e = h.console(&inst, "--once").await.unwrap_err();
    assert!(matches!(e, RuntimeError::Governance(_)), "{e}");
    assert_eq!(st.running(), ["baby-cry"], "baby-cry must keep running");
    assert!(
        st.calls().is_empty(),
        "no stop, no console: {:?}",
        st.calls()
    );
    let gate = chain.tail(0);
    let denial = gate.iter().rev().find(|e| e.source == "workload").unwrap();
    assert_eq!(denial.kind, "workload.stop");
    assert_eq!(denial.payload.as_ref().unwrap()["decision"], "deny");
    assert!(
        !runtime_events(&chain)
            .iter()
            .any(|e| e.0 == "workload.stop"),
        "no stop chained"
    );
}

#[tokio::test]
async fn pinned_cog_installs_starts_runs_one_console_cycle_stops_and_uninstalls() {
    let st = SeedState::with(&[("baby-cry", true)], false);
    let server = mock_seed(&st).await;
    let (h, chain) = host(seed_rt(&server), rule(&["workload.*"]));
    let inst = h.load(&fall_detect(), &cfg()).await.unwrap();
    assert!(inst.store_installed);
    assert_eq!(st.running(), ["baby-cry"], "auto-start was stopped");
    h.start(&inst).await.unwrap();
    let ev = h.console(&inst, "--once").await.unwrap();
    assert_eq!(ev.exit_code, Some(0));
    assert_eq!(ev.json_lines()[0]["status"], "quiet");
    assert_eq!(ev.stopped_for_console, ["baby-cry", "fall-detect"]);
    let mut running = st.running();
    running.sort();
    assert_eq!(running, ["baby-cry", "fall-detect"], "both restored");
    h.stop(&inst, Duration::from_secs(1)).await.unwrap();
    assert_eq!(st.running(), ["baby-cry"]);
    h.unload(inst).await.unwrap();
    assert_eq!(
        st.calls(),
        [
            "installed:fall-detect",
            "stop:fall-detect",
            "start:fall-detect",
            "stop:baby-cry",
            "stop:fall-detect",
            "console:fall-detect",
            "start:baby-cry",
            "start:fall-detect",
            "stop:fall-detect",
            "uninstall:fall-detect"
        ]
    );
    let t = |k: &str, id: &str, ph: &str| (k.to_string(), id.to_string(), ph.to_string());
    assert_eq!(
        runtime_events(&chain),
        [
            t("workload.install", "fall-detect", ""),
            t("workload.load", "fall-detect", ""),
            t("workload.start", "fall-detect", ""),
            t("workload.stop", "baby-cry", "preempt-for-console"),
            t("workload.stop", "fall-detect", "preempt-for-console"),
            t("workload.start", "baby-cry", "resume-after-console"),
            t("workload.start", "fall-detect", "resume-after-console"),
            t("workload.start", "fall-detect", "console"),
            t("workload.stop", "fall-detect", ""),
            t("workload.unload", "fall-detect", ""),
        ]
    );
    let dump = serde_json::to_string(&chain.tail(0).iter().map(|e| &e.payload).collect::<Vec<_>>())
        .unwrap();
    assert!(!dump.contains(TOKEN), "Seed token in chain");
}

#[tokio::test]
async fn a_failed_console_call_still_chains_its_stops_and_restores_the_cogs() {
    let st = SeedState::with(&[("fall-detect", true), ("baby-cry", true)], true);
    let server = mock_seed(&st).await;
    let (h, chain) = host(seed_rt(&server), rule(&["workload.*"]));
    let inst = h.load(&fall_detect(), &cfg()).await.unwrap();
    assert!(!inst.store_installed);
    assert!(h.console(&inst, "--once").await.is_err());
    let mut running = st.running();
    running.sort();
    assert_eq!(running, ["baby-cry", "fall-detect"]);
    let ev = runtime_events(&chain);
    let kinds: Vec<(&str, &str)> = ev.iter().map(|e| (e.0.as_str(), e.1.as_str())).collect();
    assert_eq!(
        kinds,
        [
            ("workload.load", "fall-detect"),
            ("workload.stop", "fall-detect"),
            ("workload.stop", "baby-cry"),
            ("workload.start", "fall-detect"),
            ("workload.start", "baby-cry"),
            ("workload.refuse", "fall-detect"),
        ],
        "no workload.install: it was already installed"
    );
}

#[tokio::test]
async fn an_install_whose_stop_fails_is_rolled_back_and_chained() {
    let st = SeedState::with(&[], false);
    st.fail(&["stop"]);
    let server = mock_seed(&st).await;
    let (h, chain) = host(seed_rt(&server), rule(&["workload.*"]));
    let e = h.load(&fall_detect(), &cfg()).await.unwrap_err();
    assert!(
        matches!(
            e,
            RuntimeError::StrandedInstall {
                rolled_back: true,
                ..
            }
        ),
        "{e}"
    );
    assert!(
        st.apps.lock().unwrap().is_empty(),
        "install undone on the device"
    );
    assert_eq!(
        st.calls(),
        [
            "installed:fall-detect",
            "failed-stop:fall-detect",
            "uninstall:fall-detect"
        ]
    );
    let events: Vec<(String, String)> = chain
        .tail(0)
        .into_iter()
        .filter(|e| e.source == RUNTIME_CHAIN_SOURCE)
        .map(|e| {
            (
                e.kind,
                e.payload.unwrap()["outcome"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let ev = |k: &str, o: &str| (k.to_string(), o.to_string());
    assert_eq!(
        events,
        [
            ev("workload.install", "rolled-back"),
            ev("workload.refuse", "error")
        ]
    );
}

#[tokio::test]
async fn an_install_that_cannot_be_undone_stays_recorded_and_unloadable() {
    let st = SeedState::with(&[], false);
    st.fail(&["stop", "uninstall"]);
    let server = mock_seed(&st).await;
    let (h, chain) = host(seed_rt(&server), rule(&["workload.*"]));
    let e = h.load(&fall_detect(), &cfg()).await.unwrap_err();
    let RuntimeError::StrandedInstall {
        handle,
        rolled_back: false,
        ..
    } = e
    else {
        panic!("expected a stranded install, got {e}");
    };
    assert_eq!(st.running(), ["fall-detect"], "still installed and running");
    let install = chain
        .tail(0)
        .into_iter()
        .find(|e| e.source == RUNTIME_CHAIN_SOURCE && e.kind == "workload.install")
        .expect("the install is on the chain");
    assert_eq!(install.payload.unwrap()["outcome"], "stranded");
    // While the Seed still refuses, unload fails and the handle survives.
    assert!(h.unload((*handle).clone()).await.is_err());
    assert_eq!(st.running(), ["fall-detect"]);
    // Once the Seed answers again, the operator removes it through the host.
    st.fail(&[]);
    h.unload(*handle).await.unwrap();
    assert!(st.apps.lock().unwrap().is_empty(), "uninstalled by unload");
    assert_eq!(
        runtime_events(&chain).last().unwrap().0,
        "workload.unload",
        "unload is gated and chained"
    );
}

#[tokio::test]
async fn the_live_cycle_leaves_the_operators_cogs_as_it_found_them() {
    // Operator state: fall-detect installed but stopped, baby-cry running.
    let st = SeedState::with(&[("fall-detect", false), ("baby-cry", true)], false);
    let server = mock_seed(&st).await;
    let kinds =
        super::tests_live::seed_fall_detect_cycle(&server.uri(), TOKEN, SeedTls::WebPki, false)
            .await;
    assert!(!kinds.iter().any(|k| k == "workload.install"));
    // baby-cry was only stopped for the console run and resumed by the
    // host; nothing else touched it.
    let baby: Vec<String> = st
        .calls()
        .into_iter()
        .filter(|c| c.ends_with(":baby-cry"))
        .collect();
    assert_eq!(baby, ["stop:baby-cry", "start:baby-cry"]);
    let mut apps = st.apps.lock().unwrap().clone();
    apps.sort();
    assert_eq!(
        apps,
        [
            ("baby-cry".to_string(), true),
            ("fall-detect".to_string(), false)
        ]
    );
}

#[tokio::test]
async fn the_live_cycle_installs_only_when_authorized_and_removes_what_it_installed() {
    let st = SeedState::with(&[("baby-cry", false)], false);
    let server = mock_seed(&st).await;
    let uri = server.uri();
    let refused = tokio::spawn(async move {
        super::tests_live::seed_fall_detect_cycle(&uri, TOKEN, SeedTls::WebPki, false).await
    })
    .await;
    assert!(refused.is_err(), "no install without authorization");
    assert!(
        st.calls().is_empty(),
        "nothing was changed: {:?}",
        st.calls()
    );
    let kinds =
        super::tests_live::seed_fall_detect_cycle(&server.uri(), TOKEN, SeedTls::WebPki, true)
            .await;
    assert_eq!(kinds.first().map(String::as_str), Some("workload.install"));
    assert_eq!(
        st.calls().first().map(String::as_str),
        Some("installed:fall-detect")
    );
    assert_eq!(
        st.calls().last().map(String::as_str),
        Some("uninstall:fall-detect")
    );
    assert_eq!(*st.apps.lock().unwrap(), [("baby-cry".to_string(), false)]);
}
