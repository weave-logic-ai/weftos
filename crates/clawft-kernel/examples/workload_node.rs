//! Two-node placement on real hardware (card mesh-placement-12): run a
//! `workload-host` on one node and the placement control plane on another,
//! over mesh TCP with Noise XX. `scripts/build.sh test-pi --placement`
//! drives it: `serve` on the Pi 5 (isolated scratch dir, port 9471, never
//! the system weaver), `place` on the Mac.
//!
//! ```text
//! workload_node keygen <key-file>                  # prints the public key
//! workload_node serve --listen ADDR --dir DIR --controller PUBHEX
//!     [--trust PUBHEX] [--noise] [--secs N] [--feed-port P]
//! workload_node daemon-files --controller PUBHEX --listen ADDR --out DIR
//! workload_node place --key FILE --peer ADDR --cog-toml FILE
//!     --binary ARCH=PATH [--binary ...] --dir DIR [--noise]
//!     [--interval N] [--run-secs N] [--pin-local] [--out FILE] [--release-url URL]
//! ```
//!
//! `daemon-files` writes the policy files a real `weaver` daemon needs to
//! serve its `workload-host` to this controller (`workload-host.json`,
//! `workload-trust.json`, `workload-permits.json`, for its runtime dir):
//! that is how the Pi lane places onto a WeftOS node. `serve` is the bare
//! host without a daemon.
//!
//! The controller's key signs the package (operator key) and the control
//! requests; the node pins it for both. Chains are in memory and are
//! written as JSON under `--dir` (never the operator's chain.rvf).
//! `--feed-port` makes the node send a synthetic ESP32 feature feed to
//! `127.0.0.1:P` (ADR-069 packets, as the native live test does).

use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::artifact_store::ArtifactStore;
use clawft_kernel::chain::ChainManager;
use clawft_kernel::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use clawft_kernel::node_facts::{DEFAULT_FACTS_TTL_SECS, ProbeConfig, SystemHost, probe_and_sign};
use clawft_kernel::workload_ctl::msg::method;
use clawft_kernel::workload_ctl::{
    CtlConfig, MeshConnector, PlaceOrder, PlacementControlPlane, WorkloadHostService, listen_tcp,
    serve_listener,
};
use clawft_kernel::workload_governance::{
    NetworkPolicy, NodeTrustTier, WorkloadGate, WorkloadPermitRule,
};
use clawft_kernel::workload_pkg::codec::{hex_decode_exact, hex_encode};
use clawft_kernel::workload_pkg::{
    CogPackInput, KeyOrigin, PackageSource, TrustAnchors, key_id_for, pack_cog, sign_envelope,
    write_manifest,
};
use clawft_kernel::workload_runtime::{NativeConfig, NativeRuntime, RunMode, WorkloadHost};
use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

type R<T> = Result<T, String>;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn args_all(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|w| w[0] == name)
        .map(|w| w[1].clone())
        .collect()
}

fn need(args: &[String], name: &str) -> R<String> {
    arg(args, name).ok_or_else(|| format!("{name} is required"))
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn now() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn load_key(path: &Path) -> R<SigningKey> {
    let hex = std::fs::read_to_string(path).map_err(|e| format!("read key: {e}"))?;
    let b = hex_decode_exact::<32>(hex.trim()).ok_or("key file must hold 64 hex")?;
    Ok(SigningKey::from_bytes(&b))
}

fn anchors(pk: &[u8; 32]) -> R<TrustAnchors> {
    let mut a = TrustAnchors::default();
    a.push_signer(&key_id_for(pk), &hex_encode(pk), KeyOrigin::Operator)?;
    Ok(a)
}

/// Operator permit for cog workloads on paired nodes; native cogs have
/// the host's network, so egress must be allowed explicitly.
fn gate(chain: &Arc<ChainManager>) -> R<Arc<WorkloadGate>> {
    let mut permit = WorkloadPermitRule::new("operator-cog", ["workload.*"], ["cog"]);
    permit.max_network = NetworkPolicy::Egress;
    Ok(Arc::new(
        WorkloadGate::new(0.95, false)
            .with_permit(permit)?
            .with_chain(chain.clone()),
    ))
}

fn exchange(id: &str, chain: &Arc<ChainManager>) -> R<Arc<ArtifactExchange>> {
    let mut ex = ArtifactExchange::new(
        id,
        Arc::new(ArtifactStore::new_memory()),
        ExchangeConfig::default(),
    )
    .map_err(|e| e.to_string())?;
    ex.set_chain_manager(chain.clone());
    Ok(Arc::new(ex))
}

/// A `workload-host` with the native adapter, facts probed on this machine.
fn host_service(
    key: &SigningKey,
    dir: &Path,
    controller: [u8; 32],
    trust: TrustAnchors,
    chain: &Arc<ChainManager>,
) -> R<WorkloadHostService> {
    let pk = key.verifying_key().to_bytes();
    let id = clawft_kernel::node_id_from_pubkey(&pk);
    let gate = gate(chain)?;
    let native = NativeRuntime::new(NativeConfig {
        root: dir.join("instances"),
        run_as: None,
        allow_interpreted: false,
    });
    let host = WorkloadHost::new(
        Arc::new(native),
        gate.clone(),
        id.clone(),
        NodeTrustTier::Paired,
    )
    .with_chain(chain.clone());
    let facts = probe_and_sign(
        &SystemHost::default(),
        &ProbeConfig::default(),
        key,
        now(),
        1,
        DEFAULT_FACTS_TTL_SECS,
    )
    .map_err(|e| e.to_string())?;
    let svc = WorkloadHostService::new(key.clone(), exchange(&id, chain)?, trust, gate)
        .with_route("native", Arc::new(host))
        .with_controllers(vec![controller])
        .with_chain(chain.clone());
    svc.set_facts(facts);
    Ok(svc)
}

fn dump_chain(chain: &ChainManager, path: &Path) -> R<usize> {
    let events: Vec<Value> = chain
        .tail(chain.len())
        .into_iter()
        .map(|e| {
            json!({ "seq": e.sequence, "source": e.source, "kind": e.kind,
                         "hash": hex_encode(&e.hash), "payload": e.payload })
        })
        .collect();
    std::fs::write(path, serde_json::to_vec_pretty(&events).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    Ok(events.len())
}

/// ADR-069 MAGIC_FEATURES packets at 50 Hz to `127.0.0.1:port`.
fn start_feed(port: u16) {
    std::thread::spawn(move || {
        let Ok(sock) = UdpSocket::bind("127.0.0.1:0") else {
            return;
        };
        for tick in 0u32.. {
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
            let _ = sock.send_to(&p, ("127.0.0.1", port));
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}

/// Policy files for a `weaver` daemon's runtime dir: serve `workload-host`
/// on `--listen` to this controller, pin it as the package signer, permit
/// cog workloads (with the host network a native cog has).
fn daemon_files(args: &[String]) -> R<()> {
    let hex = need(args, "--controller")?;
    let pk = hex_decode_exact::<32>(&hex).ok_or("--controller must be 64 hex")?;
    let out = PathBuf::from(need(args, "--out")?);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let host = json!({ "listen": need(args, "--listen")?, "noise": true, "controllers": [hex] });
    let trust = json!({ "schema": "weftos.workload-trust.v1",
        "operator_keys": [{ "key_id": key_id_for(&pk), "public_key": hex_encode(&pk) }] });
    let mut permit = WorkloadPermitRule::new("operator-cog", ["workload.*"], ["cog"]);
    permit.max_network = NetworkPolicy::Egress;
    for (name, v) in [
        ("workload-host.json", host),
        ("workload-trust.json", trust),
        ("workload-permits.json", json!([permit])),
    ] {
        std::fs::write(out.join(name), serde_json::to_vec_pretty(&v).unwrap_or_default())
            .map_err(|e| format!("{name}: {e}"))?;
    }
    // The files must load the way the daemon loads them.
    TrustAnchors::from_trust_json(&std::fs::read(out.join("workload-trust.json")).unwrap_or_default())?;
    Ok(())
}

async fn serve(args: &[String]) -> R<()> {
    let dir = PathBuf::from(need(args, "--dir")?);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let controller = hex_decode_exact::<32>(&need(args, "--controller")?)
        .ok_or("--controller must be 64 hex")?;
    let key = SigningKey::from_bytes(&rand::random());
    let chain = Arc::new(ChainManager::new(0, 10_000));
    let listen = need(args, "--listen")?;
    // Package signers: `--trust` (operator key), else the controller key.
    let trust = match arg(args, "--trust") {
        Some(t) => hex_decode_exact::<32>(&t).ok_or("--trust must be 64 hex")?,
        None => controller,
    };
    let svc = Arc::new(
        host_service(&key, &dir, controller, anchors(&trust)?, &chain)?
            .with_address(listen.clone()),
    );
    let listener = listen_tcp(&listen).await.map_err(|e| e.to_string())?;
    if let Some(p) = arg(args, "--feed-port") {
        start_feed(p.parse().map_err(|_| "--feed-port: not a port")?);
    }
    println!("NODE_ID {}", svc.node_id());
    println!("LISTENING {listen}");
    let secs: u64 = arg(args, "--secs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| e.to_string())?;
    tokio::select! {
        r = serve_listener(listener, svc, has(args, "--noise")) => { r.map_err(|e| e.to_string())?; }
        _ = term.recv() => println!("SIGTERM"),
        _ = tokio::signal::ctrl_c() => println!("SIGINT"),
        _ = tokio::time::sleep(Duration::from_secs(secs)) => println!("TIMEOUT"),
    }
    let n = dump_chain(&chain, &dir.join("node-chain.json"))?;
    println!("CHAIN {n} events");
    Ok(())
}

fn build_package(
    key: &SigningKey,
    dir: &Path,
    toml: &Path,
    binaries: &[String],
    release: String,
) -> R<PathBuf> {
    let src = dir.join("cog-src");
    std::fs::create_dir_all(&src).map_err(|e| e.to_string())?;
    std::fs::copy(toml, src.join("cog.toml")).map_err(|e| format!("cog.toml: {e}"))?;
    let bins = binaries
        .iter()
        .map(|b| {
            b.split_once('=')
                .map(|(a, p)| (a.to_string(), PathBuf::from(p)))
                .ok_or("--binary ARCH=PATH")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let input = CogPackInput {
        cog_dir: src,
        binaries: bins,
        source: PackageSource {
            repo: None,
            commit: None,
            release_url: Some(release),
        },
        cognitum_record: None,
    };
    let pkg = dir.join("pkg");
    let mut env = pack_cog(&input, &pkg).map_err(|e| e.to_string())?;
    let pk = key.verifying_key().to_bytes();
    sign_envelope(&mut env, key, &key_id_for(&pk)).map_err(|e| e.to_string())?;
    write_manifest(&pkg, &env).map_err(|e| e.to_string())?;
    Ok(pkg)
}

async fn place_cmd(args: &[String]) -> R<bool> {
    let key = load_key(Path::new(&need(args, "--key")?))?;
    let dir = PathBuf::from(need(args, "--dir")?);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let pk = key.verifying_key().to_bytes();
    let release = arg(args, "--release-url")
        .unwrap_or_else(|| "https://github.com/cognitum-one/cogs/releases".into());
    let pkg = build_package(
        &key,
        &dir,
        Path::new(&need(args, "--cog-toml")?),
        &args_all(args, "--binary"),
        release,
    )?;
    let chain = Arc::new(ChainManager::new(0, 10_000));
    let conn = Arc::new(MeshConnector::new(has(args, "--noise")));
    // This node (the Mac) is a candidate too, through its own host.
    let local = host_service(&key, &dir.join("local"), pk, anchors(&pk)?, &chain)?;
    let local_addr = conn.register_local("local", Arc::new(local));
    let plane = PlacementControlPlane::new(
        key.clone(),
        gate(&chain)?,
        chain.clone(),
        exchange("controller", &chain)?,
        anchors(&pk)?,
        conn,
    );
    let local_id = plane
        .add_target(&local_addr, TrustTier::Pinned)
        .await
        .map_err(|e| e.to_string())?;
    for peer in args_all(args, "--peer") {
        let id = plane
            .add_target(&peer, TrustTier::Paired)
            .await
            .map_err(|e| format!("peer: {e}"))?;
        println!("PEER {id} (paired)");
    }
    let interval: u32 = arg(args, "--interval")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let order = PlaceOrder {
        package_dir: pkg,
        config: CtlConfig {
            mode: RunMode::Interval { secs: interval },
            args: vec![],
            csi_port: arg(args, "--csi-port")
                .and_then(|s| s.parse().ok())
                .unwrap_or(15006),
        },
        pin: None,
        prefer: vec![],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: false,
        project_id: None,
    };
    let report = plane.place(&order).await.map_err(|e| e.to_string())?;
    println!("── place --explain\n{}", report.explain);
    let mut out = json!({ "controller": plane.node_id(), "local": local_id, "place": report });
    let mut ok = report
        .placed
        .as_ref()
        .is_some_and(|p| p.node_id != local_id);
    if let Some(p) = &report.placed {
        let run: u64 = arg(args, "--run-secs")
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);
        tokio::time::sleep(Duration::from_secs(run)).await;
        let status = plane
            .instance(method::STATUS, &p.instance_id)
            .await
            .map_err(|e| e.to_string())?;
        let stop = plane
            .instance(method::STOP, &p.instance_id)
            .await
            .map_err(|e| e.to_string())?;
        let logs = plane
            .instance(method::LOGS, &p.instance_id)
            .await
            .map_err(|e| e.to_string())?;
        let reports: Vec<Value> = logs["stdout"]
            .as_str()
            .unwrap_or("")
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .filter(|v| v.get("stats").is_some())
            .collect();
        let unload = plane
            .instance(method::UNLOAD, &p.instance_id)
            .await
            .map_err(|e| e.to_string())?;
        println!("STATUS {}", status["status"]["state"]);
        println!(
            "REPORTS {} (stdout lines with anomaly stats)",
            reports.len()
        );
        ok &= status["status"]["state"] == "running" && !reports.is_empty();
        out["run"] = json!({ "status": status, "stop": stop, "reports": reports.len(),
                             "first_report": reports.first(), "unload": unload });
    }
    if has(args, "--pin-local") {
        let pinned = plane
            .place(&PlaceOrder {
                pin: Some(local_id.clone()),
                ..order
            })
            .await
            .map_err(|e| e.to_string())?;
        println!("── place --pin <this Mac> --explain\n{}", pinned.explain);
        ok &= pinned.placed.is_none()
            && pinned.attempts.first().and_then(|a| a.code.as_deref()) == Some("admission");
        out["pin_local"] = json!(pinned);
    }
    out["chain"] = json!(chain.tail(chain.len()).into_iter().map(|e| json!({
        "seq": e.sequence, "source": e.source, "kind": e.kind, "hash": hex_encode(&e.hash), "payload": e.payload,
    })).collect::<Vec<_>>());
    if let Some(p) = arg(args, "--out") {
        std::fs::write(&p, serde_json::to_vec_pretty(&out).unwrap_or_default())
            .map_err(|e| e.to_string())?;
    }
    println!("RESULT {}", if ok { "ok" } else { "failed" });
    Ok(ok)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("keygen") => (|| {
            let path = args.get(1).ok_or("keygen <file>")?;
            let key = SigningKey::from_bytes(&rand::random());
            std::fs::write(path, hex_encode(&key.to_bytes())).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
            println!("{}", hex_encode(&key.verifying_key().to_bytes()));
            Ok(true)
        })(),
        Some("serve") => serve(&args[1..]).await.map(|_| true),
        Some("daemon-files") => daemon_files(&args[1..]).map(|_| true),
        Some("place") => place_cmd(&args[1..]).await,
        _ => Err("usage: workload_node keygen|serve|daemon-files|place (see the file header)".to_string()),
    };
    match r {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("workload_node: {e}");
            std::process::exit(2);
        }
    }
}
