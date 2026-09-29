//! The in-container ingest relay: how the image and run line change, which
//! host contracts are refused, and (with `python3` on the test host) the
//! relay script itself forwarding a cog's ingest post and its exit status.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use super::container::{ContainerRuntime, ContainerRuntimeConfig};
use super::container_cmd::Engine;
use super::container_relay::{RELAY_FILE, RELAY_SCRIPT};
use super::host_contract::HostContract;
use super::native::{NativeConfig, NativeRuntime};
use super::test_support::signed_workload;
use super::tests_container::{BASE, Recorder, TOML, aarch64_elf, cfg, rt};
use super::types::{RuntimeError, WorkloadRuntime};

const GATEWAY: &str = "192.0.2.10:18080";

fn upstream_cfg(addr: &str) -> super::types::WorkloadConfig {
    let mut c = cfg();
    c.host = HostContract::default_feed().with_ingest_upstream(addr.parse().unwrap());
    c
}

#[tokio::test]
async fn an_ingest_upstream_fronts_the_cog_with_the_relay_and_only_its_caps() {
    let root = tempfile::tempdir().unwrap();
    let rec = Arc::new(Recorder::default());
    let r = rt(Engine::Apple, root.path(), rec.clone());
    let fx = signed_workload(TOML, &[("aarch64", &aarch64_elf())]);
    let c = upstream_cfg(GATEWAY);
    let h = r.load(&fx.workload, &c).await.unwrap();
    let ctx = root.path().join(&h.instance_id).join("ctx");
    let df = std::fs::read_to_string(ctx.join("Dockerfile")).unwrap();
    assert!(df.starts_with(&format!("FROM {BASE}\n")));
    assert!(df.contains("USER 0:0"), "{df}");
    assert!(
        df.contains(&format!(
            "\"--listen\", \"127.0.0.1:80\", \"--upstream\", \"{GATEWAY}\", \"--uid\", \"65534\", \"--gid\", \"65534\", \"--\", \"/cog\"]"
        )),
        "{df}"
    );
    assert!(
        df.contains("RUN [\"python3\"") && df.contains("import ctypes"),
        "the build checks for python3 and ctypes (no_new_privs)"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.join(RELAY_FILE)).unwrap(),
        RELAY_SCRIPT
    );
    assert_eq!(c.host.audit()["ingest_upstream"], GATEWAY);

    r.start(&h).await.unwrap();
    let run = rec.find("run").join(" ");
    for want in [
        "--cap-drop ALL",
        "--cap-add CAP_NET_BIND_SERVICE",
        "--cap-add CAP_SETUID",
        "--cap-add CAP_SETGID",
        "--read-only",
    ] {
        assert!(run.contains(want), "missing {want}: {run}");
    }
    assert_eq!(run.matches("--cap-add").count(), 3, "{run}");
}

#[tokio::test]
async fn without_an_upstream_the_image_has_no_relay_and_no_added_caps() {
    let root = tempfile::tempdir().unwrap();
    let rec = Arc::new(Recorder::default());
    let r = rt(Engine::Docker, root.path(), rec.clone());
    let fx = signed_workload(TOML, &[("aarch64", &aarch64_elf())]);
    let h = r.load(&fx.workload, &cfg()).await.unwrap();
    let ctx = root.path().join(&h.instance_id).join("ctx");
    let df = std::fs::read_to_string(ctx.join("Dockerfile")).unwrap();
    assert!(
        df.contains("USER 65534:65534") && !df.contains("relay"),
        "{df}"
    );
    assert!(!ctx.join(RELAY_FILE).exists());
    r.start(&h).await.unwrap();
    assert!(!rec.find("run").join(" ").contains("--cap-add"));
}

#[tokio::test]
async fn upstreams_the_container_cannot_use_are_refused_before_any_build() {
    let fx = signed_workload(TOML, &[("aarch64", &aarch64_elf())]);
    for (network, up) in [
        (Some("host"), GATEWAY),
        (Some("none"), GATEWAY),
        (None, "127.0.0.1:80"),
        (None, "0.0.0.0:8080"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let rec = Arc::new(Recorder::default());
        let mut c = ContainerRuntimeConfig::new(Engine::Docker, BASE, root.path());
        c.network = network.map(str::to_string);
        let r = ContainerRuntime::new(c, rec.clone());
        let e = r.load(&fx.workload, &upstream_cfg(up)).await.unwrap_err();
        assert!(
            matches!(e, RuntimeError::InvalidConfig(_)),
            "{network:?} {up}: {e}"
        );
        assert!(rec.calls().is_empty(), "no build for {network:?} {up}");
    }
    // Native cogs post to the node's own loopback: only 127.0.0.1:80 works.
    let root = tempfile::tempdir().unwrap();
    let native = NativeRuntime::new(NativeConfig {
        root: root.path().to_path_buf(),
        run_as: None,
        allow_interpreted: false,
    });
    let e = native.load(&fx.workload, &upstream_cfg(GATEWAY)).await;
    assert!(matches!(e, Err(RuntimeError::InvalidConfig(_))), "{e:?}");
}

fn python3() -> bool {
    Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn relay(listen: u16, upstream: SocketAddr, child: &str) -> Command {
    let mut c = Command::new("python3");
    c.args(["-B", "-c", RELAY_SCRIPT, "--listen"])
        .arg(format!("127.0.0.1:{listen}"))
        .arg("--upstream")
        .arg(upstream.to_string())
        .args(["--", "/usr/bin/env", "python3", "-c", child])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

#[test]
fn the_relay_forwards_the_cogs_ingest_post_and_returns_its_exit_status() {
    if !python3() {
        eprintln!("python3 not found: relay script not exercised");
        return;
    }
    let up = TcpListener::bind("127.0.0.1:0").unwrap();
    let up_addr = up.local_addr().unwrap();
    let bridge = std::thread::spawn(move || {
        let (mut s, _) = up.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut buf = vec![0u8; 4096];
        let n = s.read(&mut buf).unwrap();
        s.write_all(b"HTTP/1.0 200 OK\r\n\r\nstored").unwrap();
        String::from_utf8_lossy(&buf[..n]).to_string()
    });
    let listen = free_port();
    // The "cog": posts one ingest request to the fixed loopback address
    // (here the relay's test port), prints the answer, exits 7.
    let cog = format!(
        "import socket,sys\n\
         s=socket.create_connection(('127.0.0.1',{listen}),timeout=10)\n\
         s.sendall(b'POST /api/v1/store/ingest HTTP/1.0\\r\\nContent-Length: 2\\r\\n\\r\\n{{}}')\n\
         print(s.recv(4096).decode())\n\
         sys.exit(7)\n"
    );
    let out = relay(listen, up_addr, &cog).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(7),
        "stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("200 OK") && stdout.contains("stored"),
        "{stdout}"
    );
    let got = bridge.join().unwrap();
    assert!(
        got.starts_with("POST /api/v1/store/ingest HTTP/1.0"),
        "{got}"
    );
}

#[test]
fn the_relay_forwards_sigterm_to_the_cog() {
    if !python3() {
        return;
    }
    let dead = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let mut child = relay(free_port(), dead, "import time\ntime.sleep(60)\n")
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(800));
    // SAFETY: plain kill(2) on a child we own.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let t0 = std::time::Instant::now();
    let status = child.wait().unwrap();
    assert!(t0.elapsed() < Duration::from_secs(10), "cog did not stop");
    assert_eq!(status.code(), Some(128 + libc::SIGTERM), "{status:?}");
}

/// What the relayed cog sees: its uid and NoNewPrivs, from a child that
/// reads `/proc/self/status` (Linux only).
const PRIV_PROBE: &str = "import os\n\
    st = open('/proc/self/status').read()\n\
    nnp = st.split('NoNewPrivs:')[1].split()[0]\n\
    print('uid=%d nnp=%s' % (os.getuid(), nnp))\n";

/// On a Linux test host the relay sets no_new_privs for the cog even when
/// it is not root (no uid drop happens then). Elsewhere prctl does not
/// exist and the live container test below covers it.
#[test]
fn the_relay_sets_no_new_privs_for_the_cog() {
    if !cfg!(target_os = "linux") || !python3() {
        eprintln!("not Linux with python3: covered by live_relay_drops_privileges_in_a_container");
        return;
    }
    let dead = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let out = relay(free_port(), dead, PRIV_PROBE).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("nnp=1"),
        "{stdout} {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Live (WEFTOS_CONTAINER_LIVE=1, WEFTOS_COG_BASE_IMAGE, engines from
/// WEFTOS_CONTAINER_ENGINES): run the relay as the image entrypoint would,
/// as root with only the relay capabilities and the adapter's task cap, and
/// check the cog runs as `nobody` with no_new_privs set.
#[test]
fn live_relay_drops_privileges_in_a_container() {
    if std::env::var("WEFTOS_CONTAINER_LIVE").as_deref() != Ok("1") {
        return;
    }
    let base = std::env::var("WEFTOS_COG_BASE_IMAGE").expect("WEFTOS_COG_BASE_IMAGE");
    let engines =
        std::env::var("WEFTOS_CONTAINER_ENGINES").unwrap_or_else(|_| "docker,apple".into());
    for engine in engines.split(',') {
        let (bin, extra): (&str, Vec<String>) = match engine {
            "apple" => ("container", vec!["--ulimit".into(), "nproc=64:64".into()]),
            other => (other, vec!["--pids-limit".into(), "64".into()]),
        };
        let mut c = Command::new(bin);
        c.args(["run", "--rm", "--read-only", "--cap-drop", "ALL"]);
        for cap in super::container_relay::RELAY_CAPS {
            c.args(["--cap-add", cap]);
        }
        c.args(&extra)
            .args(["--user", "0:0", &base, "python3", "-B", "-c", RELAY_SCRIPT])
            .args(["--listen", "127.0.0.1:80", "--upstream", "192.0.2.1:9"])
            .args([
                "--uid",
                "65534",
                "--gid",
                "65534",
                "--",
                "/usr/bin/env",
                "python3",
                "-c",
                PRIV_PROBE,
            ]);
        let out = c.output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        eprintln!("[relay-privs] {engine}: {}", stdout.trim());
        assert!(
            stdout.contains("uid=65534 nnp=1"),
            "{engine}: {stdout} {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
