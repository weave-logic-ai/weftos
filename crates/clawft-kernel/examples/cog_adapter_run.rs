//! Conformance launcher: runs one cog through a `WorkloadRuntime` adapter
//! under `WorkloadHost`, so the card-08 harness (`scripts/cogs/harness.py`
//! with a plan `launcher`) exercises the real adapters instead of spawning
//! the binary itself.
//!
//! ```text
//! cog_adapter_run --runtime native|docker|apple|podman [--base-image REF]
//!     [--csi-port 5006] [--feed-port N] [--feed-publish-ip IP] [--network NAME]
//!     [--run-as UID:GID] [--timeout-secs 15] [--run-secs N] [--arch aarch64]
//!     -- <cog binary> <cog args...>
//! ```
//!
//! `--once` runs one governed console cycle; `--interval N` loads, starts,
//! runs until `--run-secs` or SIGTERM, then stops. The cog's stdout and
//! stderr are passed through; the exit code is the cog's (124 on a console
//! timeout, 125 when the adapter or governance refused). The package is
//! signed with a throwaway operator key pinned for this run only, and the
//! chain is in memory: nothing touches operator data. A summary line
//! prefixed `[adapter]` goes to stderr.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::chain::ChainManager;
use clawft_kernel::workload_governance::{
    NetworkPolicy, NodeTrustTier, WorkloadGate, WorkloadPermitRule,
};
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::{
    CogPackInput, DirSource, KeyOrigin, PackageSource, TrustAnchors, VerifyPolicy, key_id_for,
    pack_cog, sign_envelope, verify_dir, write_manifest,
};
use clawft_kernel::workload_runtime::{
    ContainerRuntime, ContainerRuntimeConfig, Engine, HostContract, NativeConfig, NativeRuntime,
    RUNTIME_CHAIN_SOURCE, RunEvidence, RunMode, SystemRunner, VerifiedWorkload, WorkloadConfig,
    WorkloadHost, WorkloadRuntime,
};
use ed25519_dalek::SigningKey;

#[derive(Default)]
struct Opts {
    runtime: String,
    base_image: Option<String>,
    csi_port: u16,
    feed_port: Option<u16>,
    feed_publish_ip: Option<std::net::IpAddr>,
    network: Option<String>,
    run_as: Option<(u32, u32)>,
    timeout_secs: u64,
    run_secs: Option<u64>,
    arch: String,
    binary: PathBuf,
    cog_args: Vec<String>,
}

fn parse(mut it: impl Iterator<Item = String>) -> Result<Opts, String> {
    let mut o = Opts {
        csi_port: 5006,
        timeout_secs: 15,
        arch: "aarch64".into(),
        ..Opts::default()
    };
    let num = |v: Option<String>, what: &str| -> Result<u64, String> {
        v.ok_or(format!("{what} needs a value"))?
            .parse()
            .map_err(|_| format!("{what}: not a number"))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--runtime" => o.runtime = it.next().ok_or("--runtime needs a value")?,
            "--base-image" => o.base_image = it.next(),
            "--csi-port" => o.csi_port = num(it.next(), "--csi-port")? as u16,
            "--feed-port" => o.feed_port = Some(num(it.next(), "--feed-port")? as u16),
            "--feed-publish-ip" => {
                o.feed_publish_ip = Some(
                    it.next()
                        .and_then(|v| v.parse().ok())
                        .ok_or("--feed-publish-ip needs an IP")?,
                )
            }
            "--network" => o.network = it.next(),
            "--run-as" => {
                let v = it.next().ok_or("--run-as needs UID:GID")?;
                let (u, g) = v.split_once(':').ok_or("--run-as needs UID:GID")?;
                o.run_as = Some((
                    u.parse().map_err(|_| "bad uid")?,
                    g.parse().map_err(|_| "bad gid")?,
                ));
            }
            "--timeout-secs" => o.timeout_secs = num(it.next(), "--timeout-secs")?,
            "--run-secs" => o.run_secs = Some(num(it.next(), "--run-secs")?),
            "--arch" => o.arch = it.next().ok_or("--arch needs a value")?,
            "--" => {
                o.binary = it.next().ok_or("missing cog binary after --")?.into();
                o.cog_args = it.collect();
                break;
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    if o.runtime.is_empty() || o.binary.as_os_str().is_empty() {
        return Err("--runtime and -- <binary> are required".into());
    }
    if o.csi_port == 0 || !(1..=600).contains(&o.timeout_secs) {
        return Err("--csi-port must be non-zero and --timeout-secs 1..=600".into());
    }
    Ok(o)
}

/// `cog-<id>-<arch>` -> `<id>`.
fn cog_id(binary: &Path) -> String {
    let name = binary
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("cog");
    let name = name.strip_prefix("cog-").unwrap_or(name);
    ["-aarch64", "-arm", "-armv7"]
        .iter()
        .find_map(|s| name.strip_suffix(s))
        .unwrap_or(name)
        .to_string()
}

/// `cog.toml` whose `[config]` surface and `[console]` command match the
/// harness's argv exactly (nothing wider).
fn cog_toml(id: &str, args: &[String], timeout: u64) -> String {
    let mut t = format!(
        "[cog]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"0.0.0-conformance\"\n\n"
    );
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a.starts_with("--") && a != "--once" && a != "--interval" {
            let key = a.trim_start_matches('-').replace('-', "_");
            let boolean = args.get(i + 1).is_none_or(|n| n.starts_with("--"));
            let ty = if boolean { "boolean" } else { "string" };
            t.push_str(&format!(
                "[config.{key}]\ntype = \"{ty}\"\ncli_arg = \"{a}\"\n\n"
            ));
        }
        i += 1;
    }
    // Only a `--once` run is a console command; interval runs are placed.
    let cmds = if args.iter().any(|a| a == "--once") {
        format!("\"{}\"", args.join(" ").replace('"', ""))
    } else {
        String::new()
    };
    t.push_str(&format!(
        "[console]\nallowed_commands = [{cmds}]\nmax_runtime_secs = {timeout}\n\
         output_limit_bytes = 1048576\n"
    ));
    t
}

/// Pack, sign with a throwaway operator key, verify, load.
fn signed(dir: &Path, o: &Opts, id: &str) -> Result<VerifiedWorkload, String> {
    let cog_dir = dir.join("src");
    std::fs::create_dir_all(&cog_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        cog_dir.join("cog.toml"),
        cog_toml(id, &o.cog_args, o.timeout_secs),
    )
    .map_err(|e| e.to_string())?;
    let input = CogPackInput {
        cog_dir,
        binaries: vec![(o.arch.clone(), o.binary.clone())],
        source: PackageSource {
            repo: Some("conformance".into()),
            // The released binary under test (card-08 harness download).
            commit: None,
            release_url: Some("https://storage.googleapis.com/cognitum-apps/cogs".into()),
        },
        cognitum_record: None,
    };
    let pkg = dir.join("pkg");
    let mut env = pack_cog(&input, &pkg).map_err(|e| e.to_string())?;
    let mut seed = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
    let key = SigningKey::from_bytes(&seed);
    let pk = key.verifying_key().to_bytes();
    sign_envelope(&mut env, &key, &key_id_for(&pk)).map_err(|e| e.to_string())?;
    write_manifest(&pkg, &env).map_err(|e| e.to_string())?;
    let mut anchors = TrustAnchors::default();
    anchors
        .push_signer(&key_id_for(&pk), &hex_encode(&pk), KeyOrigin::Operator)
        .map_err(|e| e.to_string())?;
    let verified =
        verify_dir(&pkg, &anchors, &VerifyPolicy::default()).map_err(|e| e.to_string())?;
    VerifiedWorkload::from_package(&verified, &DirSource::new(&pkg)).map_err(|e| e.to_string())
}

fn runtime(o: &Opts, work: &Path) -> Result<Arc<dyn WorkloadRuntime>, String> {
    let engine = match o.runtime.as_str() {
        "native" => {
            return Ok(Arc::new(NativeRuntime::new(NativeConfig {
                root: work.join("native"),
                run_as: o.run_as,
                allow_interpreted: false,
            })));
        }
        "docker" => Engine::Docker,
        "apple" | "apple-container" => Engine::Apple,
        "podman" => Engine::Podman,
        other => return Err(format!("unknown runtime {other}")),
    };
    let base = o
        .base_image
        .clone()
        .ok_or("container runtimes need --base-image name@sha256:...")?;
    let mut c = ContainerRuntimeConfig::new(engine, base, work.join("ctr"));
    c.feed_host_port = o.feed_port;
    if let Some(ip) = o.feed_publish_ip {
        c.feed_publish_ip = ip;
    }
    c.network = o.network.clone();
    Ok(Arc::new(ContainerRuntime::new(c, Arc::new(SystemRunner))))
}

fn emit(ev: &RunEvidence) {
    let _ = std::io::stdout().write_all(ev.stdout.as_bytes());
    let _ = std::io::stderr().write_all(ev.stderr.as_bytes());
    let _ = std::io::stdout().flush();
}

async fn run(o: Opts) -> Result<i32, String> {
    let work = tempfile::tempdir().map_err(|e| e.to_string())?;
    if o.run_as.is_some() {
        // The unprivileged run_as user must reach the staged instance dir.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    let id = cog_id(&o.binary);
    let w = signed(work.path(), &o, &id)?;
    let rt = runtime(&o, work.path())?;
    let chain = Arc::new(ChainManager::new(0, 10_000));
    let mut permit = WorkloadPermitRule::new("conformance", ["workload.*"], ["cog"]);
    permit.max_network = NetworkPolicy::Egress;
    let gate = WorkloadGate::new(0.8, false)
        .with_chain(chain.clone())
        .with_permit(permit)
        .map_err(|e| e.to_string())?;
    let host = WorkloadHost::new(rt.clone(), Arc::new(gate), "conformance", NodeTrustTier::Paired)
        .with_chain(chain.clone());
    let csi = std::net::SocketAddr::from(([0, 0, 0, 0], o.csi_port));
    let once = o.cog_args.iter().any(|a| a == "--once");
    let (mode, args) = match o.cog_args.iter().position(|a| a == "--interval") {
        Some(i) if !once => {
            let secs: u32 = o
                .cog_args
                .get(i + 1)
                .and_then(|s| s.parse().ok())
                .ok_or("--interval needs seconds")?;
            let mut rest = o.cog_args.clone();
            rest.drain(i..=i + 1);
            (RunMode::Interval { secs }, rest)
        }
        _ => (RunMode::Once, Vec::new()),
    };
    let cfg = WorkloadConfig {
        mode: mode.clone(),
        args,
        host: HostContract::new(csi),
        node_id: "conformance".into(),
    };
    let h = host.load(&w, &cfg).await.map_err(|e| e.to_string())?;
    let code = if once {
        let ev = host
            .console(&h, &o.cog_args.join(" "))
            .await
            .map_err(|e| e.to_string())?;
        emit(&ev);
        if ev.killed_for_timeout {
            124
        } else {
            ev.exit_code.unwrap_or(125)
        }
    } else {
        host.start(&h).await.map_err(|e| e.to_string())?;
        let limit = Duration::from_secs(o.run_secs.unwrap_or(o.timeout_secs));
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| e.to_string())?;
        tokio::select! {
            _ = tokio::time::sleep(limit) => {}
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        let ev = host
            .stop(&h, Duration::from_secs(2))
            .await
            .map_err(|e| e.to_string())?;
        emit(&ev);
        0
    };
    host.unload(h).await.map_err(|e| e.to_string())?;
    let kinds: Vec<String> = chain
        .tail(0)
        .into_iter()
        .filter(|e| e.source == RUNTIME_CHAIN_SOURCE)
        .map(|e| e.kind)
        .collect();
    let caps: Vec<String> = rt.provides().iter().map(|c| c.id.to_string()).collect();
    eprintln!(
        "[adapter] runtime={} provides={caps:?} network={:?} chain={kinds:?}",
        rt.id(),
        rt.network_exposure()
    );
    Ok(code)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let o = match parse(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[adapter] usage error: {e}");
            std::process::exit(2);
        }
    };
    match run(o).await {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("[adapter] refused: {e}");
            std::process::exit(125);
        }
    }
}
