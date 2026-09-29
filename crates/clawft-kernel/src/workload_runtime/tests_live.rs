//! Live adapter runs. Every test here returns immediately unless its env
//! flag is set, so `scripts/build.sh test` stays hermetic.
//!
//! - `WEFTOS_CONTAINER_LIVE=1`: anomaly-detect under each engine in
//!   `WEFTOS_CONTAINER_ENGINES` (default `docker,apple`). Needs
//!   `WEFTOS_COG_AARCH64_BIN` (the released aarch64 binary) and
//!   `WEFTOS_COG_BASE_IMAGE` (a digest-pinned base present locally).
//! - `WEFTOS_NATIVE_LIVE=1` (Linux only): anomaly-detect under the native
//!   adapter with the same binary env var.
//! - `COGNITUM_SEED_LIVE=1`: fall-detect on a real Seed at
//!   `COGNITUM_SEED_BASE` with `COGNITUM_SEED_TOKEN`. Never installs a new
//!   cog (fall-detect must already be installed; the install path is
//!   covered against the mock Seed in `tests_seed_host`); start, console
//!   (preempt, run, restore), stop.
//!
//! The chain is always an in-memory [`ChainManager`], never the operator's.

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::container::{ContainerRuntime, ContainerRuntimeConfig};
use super::container_cmd::{Engine, SystemRunner};
use super::host::{RUNTIME_CHAIN_SOURCE, WorkloadHost};
use super::host_contract::HostContract;
use super::native::{NativeConfig, NativeRuntime};
use super::seed::{SeedApiRuntime, SeedConfig, SeedPin};
use super::seed_http::HttpSeedTransport;
use super::test_support::{MemoryCredentials, signed_workload};
use super::types::*;
use crate::chain::ChainManager;
use crate::workload_governance::{
    NetworkPolicy, NodeTrustTier, PackageTrust, WorkloadGate, WorkloadPermitRule,
};

/// Upstream anomaly-detect `cog.toml` (identity, config surface, console).
const ANOMALY_TOML: &str = r#"[cog]
id = "anomaly-detect"
name = "Anomaly Detection"
version = "1.2.0"
category = "health"
binary = "cog-anomaly-detect-arm"

[config.interval]
type = "integer"
default = 10
cli_arg = "--interval"

[config.threshold]
type = "float"
default = 2.5
cli_arg = "--threshold"

[console]
allowed_commands = ["--once", "--once --threshold 1.5", "--once --threshold 3.0"]
max_runtime_secs = 15
output_limit_bytes = 65536
"#;

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

/// ADR-069 MAGIC_FEATURES packet: magic, 12 pad bytes, 8 LE f32.
fn feature_packet(tick: u32) -> Vec<u8> {
    let mut p = 0xC511_0003u32.to_le_bytes().to_vec();
    p.extend_from_slice(&[0u8; 12]);
    for i in 0..8 {
        let v = if tick % 40 == 39 {
            0.95f32
        } else {
            0.1 * ((tick as f32) / 5.0 + i as f32).sin()
        };
        p.extend_from_slice(&v.to_le_bytes());
    }
    p
}

/// Sends the reference feed to `target` at 50 Hz until dropped.
struct Feed(Arc<AtomicBool>);

impl Feed {
    fn start(target: String) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        std::thread::spawn(move || {
            let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
            let mut tick = 0;
            while !s.load(Ordering::Relaxed) {
                let _ = sock.send_to(&feature_packet(tick), &target);
                tick += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        Self(stop)
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn host_for(
    rt: Arc<dyn WorkloadRuntime>,
    permit: WorkloadPermitRule,
) -> (WorkloadHost, Arc<ChainManager>) {
    let chain = Arc::new(ChainManager::new(0, 1000));
    let gate = WorkloadGate::new(0.8, false)
        .with_chain(chain.clone())
        .with_permit(permit)
        .unwrap();
    (
        WorkloadHost::new(rt, Arc::new(gate), "live-test", NodeTrustTier::Paired)
            .with_chain(chain.clone()),
        chain,
    )
}

fn runtime_kinds(chain: &ChainManager) -> Vec<String> {
    chain
        .tail(0)
        .into_iter()
        .filter(|e| e.source == RUNTIME_CHAIN_SOURCE)
        .map(|e| e.kind)
        .collect()
}

/// Load, console `--once`, run in interval mode for a few cycles, stop,
/// unload; assert anomaly-detect produced its JSON report each time.
async fn anomaly_detect_cycle(rt: Arc<dyn WorkloadRuntime>, csi_bind: &str, feed_to: String) {
    let bin = std::env::var("WEFTOS_COG_AARCH64_BIN").expect("WEFTOS_COG_AARCH64_BIN");
    let fx = signed_workload(ANOMALY_TOML, &[("aarch64", &std::fs::read(bin).unwrap())]);
    // Native and container instances can reach the internet (nothing
    // restricts egress yet), so the operator permit has to allow it.
    let mut permit = WorkloadPermitRule::new("live", ["workload.*"], ["cog"]);
    permit.max_network = NetworkPolicy::Egress;
    let (host, chain) = host_for(rt.clone(), permit);
    let _feed = Feed::start(feed_to);
    let mut cfg = WorkloadConfig {
        mode: RunMode::Interval { secs: 1 },
        args: vec![],
        host: HostContract::new(csi_bind.parse().unwrap()),
        node_id: "live".into(),
    };
    cfg.args = vec!["--threshold".into(), "2.5".into()];
    let h = host.load(&fx.workload, &cfg).await.expect("load");

    let ev = host.console(&h, "--once").await.expect("console");
    eprintln!(
        "[{}] console: exit={:?} elapsed={}ms json={} stderr={:?}",
        rt.id(),
        ev.exit_code,
        ev.elapsed_ms,
        ev.json_lines().len(),
        ev.stderr.chars().take(300).collect::<String>()
    );
    assert!(ev.succeeded(), "{ev:?}");
    let report = ev
        .json_lines()
        .into_iter()
        .find(|l| l.get("stats").is_some())
        .expect("anomaly report");
    // A report exists only if the cog read the UDP feed (its HTTP fallback
    // has no server here), so this proves the host contract reached it.
    assert!(
        report["stats"]["num_channels"].as_u64().unwrap_or(0) > 0,
        "{report}"
    );
    assert!(report["anomalies"].is_array(), "{report}");

    host.start(&h).await.expect("start");
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(host.status(&h).await.state, InstanceState::Running);
    let ev = host.stop(&h, Duration::from_secs(3)).await.expect("stop");
    let reports = ev
        .json_lines()
        .into_iter()
        .filter(|l| l.get("stats").is_some())
        .count();
    eprintln!(
        "[{}] interval run: {} reports in {} ms",
        rt.id(),
        reports,
        ev.elapsed_ms
    );
    assert!(reports >= 1, "{ev:?}");
    host.unload(h).await.expect("unload");
    assert_eq!(
        runtime_kinds(&chain),
        [
            "workload.load",
            "workload.start",
            "workload.start",
            "workload.stop",
            "workload.unload"
        ]
    );
}

#[tokio::test]
async fn live_container_anomaly_detect() {
    if !flag("WEFTOS_CONTAINER_LIVE") {
        return;
    }
    let base = std::env::var("WEFTOS_COG_BASE_IMAGE").expect("WEFTOS_COG_BASE_IMAGE");
    let engines =
        std::env::var("WEFTOS_CONTAINER_ENGINES").unwrap_or_else(|_| "docker,apple".into());
    for name in engines.split(',') {
        let engine = match name.trim() {
            "docker" => Engine::Docker,
            "apple" => Engine::Apple,
            "podman" => Engine::Podman,
            other => panic!("unknown engine {other}"),
        };
        let work = tempfile::tempdir().unwrap();
        let port = free_udp_port();
        let mut c = ContainerRuntimeConfig::new(engine, base.clone(), work.path());
        c.feed_host_port = Some(port);
        let rt = Arc::new(ContainerRuntime::new(c, Arc::new(SystemRunner)));
        let caps = rt.provides();
        assert_eq!(caps[0].id.as_str(), engine.capability_id());
        anomaly_detect_cycle(rt, "0.0.0.0:5006", format!("127.0.0.1:{port}")).await;
    }
}

#[tokio::test]
async fn live_native_anomaly_detect() {
    if !flag("WEFTOS_NATIVE_LIVE") || std::env::consts::OS != "linux" {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let rt = Arc::new(NativeRuntime::new(NativeConfig {
        root: root.path().to_path_buf(),
        run_as: None,
        allow_interpreted: false,
    }));
    assert_eq!(rt.provides()[0].id.as_str(), "runtime.native");
    let port = std::env::var("WEFTOS_COG_FEED_PORT").unwrap_or_else(|_| "5006".into());
    anomaly_detect_cycle(rt, &format!("0.0.0.0:{port}"), format!("127.0.0.1:{port}")).await;
}

/// Live Seed run (COGNITUM_SEED_LIVE=1, COGNITUM_SEED_BASE,
/// COGNITUM_SEED_TOKEN). Every change to the Seed goes through
/// [`WorkloadHost`] (gated and chained); nothing is stopped or started
/// behind its back, and cogs end in the state they started in.
///
/// fall-detect already installed: load, start, one console cycle, stop,
/// unload; no install happens or is chained. fall-detect not installed:
/// the run refuses unless COGNITUM_SEED_LIVE_INSTALL=1 also authorizes the
/// install, in which case load installs it (chained) and unload removes it.
#[tokio::test]
async fn live_seed_fall_detect() {
    if !flag("COGNITUM_SEED_LIVE") {
        return;
    }
    let base = std::env::var("COGNITUM_SEED_BASE").expect("COGNITUM_SEED_BASE");
    let token = std::env::var("COGNITUM_SEED_TOKEN").expect("COGNITUM_SEED_TOKEN");
    let kinds = seed_fall_detect_cycle(&base, &token, flag("COGNITUM_SEED_LIVE_INSTALL")).await;
    eprintln!("[seed] chain: {kinds:?}");
}

/// The live Seed cycle, also run hermetically against the stateful mock
/// Seed (`tests_seed_host`). Returns the runtime chain kinds.
pub(super) async fn seed_fall_detect_cycle(
    base: &str,
    token: &str,
    allow_install: bool,
) -> Vec<String> {
    let creds = Arc::new(MemoryCredentials::with("seed-live", token));
    let seed = Arc::new(
        SeedApiRuntime::new(
            SeedConfig {
                node_id: "seed-live".into(),
                pins: vec![
                    SeedPin::new("fall-detect", "1.0.0"),
                    SeedPin::new("baby-cry", "1.0.0"),
                ],
                concurrency_cap: 3,
            },
            Arc::new(HttpSeedTransport::new(base, base.starts_with("https://")).unwrap()),
            creds,
        )
        .unwrap(),
    );
    // Read-only snapshot of what the operator has running.
    let before = seed.installed().await.expect("list apps");
    let was = |id: &str| before.iter().find(|c| c.id == id).map(|c| c.running);
    let preinstalled = was("fall-detect").is_some();
    assert!(
        preinstalled || allow_install,
        "fall-detect is not installed; set COGNITUM_SEED_LIVE_INSTALL=1 to authorize installing it"
    );
    let mut permit = WorkloadPermitRule::new("seed-live", ["workload.*"], ["cog"]);
    permit.min_package_trust = PackageTrust::OperatorAttested;
    permit.max_network = NetworkPolicy::Egress;
    let (host, chain) = host_for(seed.clone(), permit);
    let w = VerifiedWorkload::store_pin("cognitum", "fall-detect", "1.0.0", None).unwrap();
    let cfg = WorkloadConfig {
        mode: RunMode::Listener,
        args: vec![],
        host: HostContract::default_feed(),
        node_id: "seed-live".into(),
    };

    let h = host.load(&w, &cfg).await.expect("load");
    assert_eq!(h.store_installed, !preinstalled);
    host.start(&h).await.expect("start");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(host.status(&h).await.state, InstanceState::Running);
    let ev = host.console(&h, "--once").await.expect("console");
    eprintln!(
        "[seed] console: exit={:?} elapsed={}ms stopped={:?} out={}",
        ev.exit_code,
        ev.elapsed_ms,
        ev.stopped_for_console,
        ev.stdout.chars().take(400).collect::<String>()
    );
    assert!(ev.stopped_for_console.contains(&"fall-detect".to_string()));
    assert!(!ev.stdout.is_empty());
    // The host restarted what it preempted for the console run.
    assert_eq!(host.status(&h).await.state, InstanceState::Running);
    let stop = host.stop(&h, Duration::from_secs(3)).await.expect("stop");
    eprintln!("[seed] stop evidence: {} log bytes", stop.stdout_bytes);
    // Every other cog is back where the operator left it (the host resumed
    // what it preempted); fall-detect is stopped by the governed stop.
    let after = seed.installed().await.unwrap();
    for c in &after {
        match was(&c.id) {
            _ if c.id == "fall-detect" => assert!(!c.running, "fall-detect stopped"),
            Some(r) => assert_eq!(c.running, r, "{} changed state", c.id),
            None => panic!("{} appeared during the run", c.id),
        }
    }
    host.unload(h).await.expect("unload");
    let installed = seed.installed().await.unwrap();
    assert_eq!(
        installed.iter().any(|c| c.id == "fall-detect"),
        preinstalled,
        "unload removes only what this run installed"
    );
    // Restore fall-detect's own state through the host, if it was running.
    if was("fall-detect") == Some(true) {
        let h = host.load(&w, &cfg).await.expect("reload");
        host.start(&h).await.expect("restore fall-detect");
    }
    let kinds = runtime_kinds(&chain);
    assert_eq!(
        kinds.iter().any(|k| k == "workload.install"),
        !preinstalled,
        "an install is chained exactly when one happened"
    );
    for k in [
        "workload.load",
        "workload.start",
        "workload.stop",
        "workload.unload",
    ] {
        assert!(kinds.iter().any(|x| x == k), "missing {k}");
    }
    let dump = serde_json::to_string(&chain.tail(0).iter().map(|e| &e.payload).collect::<Vec<_>>())
        .unwrap();
    assert!(!dump.contains(token), "token in chain");
    kinds
}
