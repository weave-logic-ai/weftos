//! Hardening of the managed-role inputs and the lifecycle edges the daemon
//! must notice. Fakes only.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use clawft_kernel::workload_governance::NetworkPolicy;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::infer_managed_tests::{lab, managed, permit, state_of};
use crate::infer_wire::*;
use crate::infer_wire_tests::{fake_on, free_port, parts, write_cfg};

#[tokio::test]
async fn a_launcher_others_can_replace_is_not_run() {
    let lab = lab();
    let cfg = || serde_json::json!({"roles": [managed("hermes", "Model-A", 1.0, free_port(), None)], "serve_programs": lab.serve()});
    let prog = lab.path().join("bin/serve-llamacpp");
    for (mode, why) in [(0o775, "writable by group or others"), (0o757, "writable by group or others")] {
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(mode)).unwrap();
        write_cfg(lab.path(), cfg());
        let (st, _) = build(lab.parts(Some(lab.gate(Some(permit(NetworkPolicy::None)))))).await.unwrap().unwrap();
        let (state, reason) = state_of(&st, "hermes");
        assert_eq!(state, "unavailable", "{mode:o}");
        assert!(reason.unwrap().contains(why), "{mode:o}");
    }
    // Owned by the daemon's user and not writable by others: fine.
    std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
    write_cfg(lab.path(), cfg());
    let (st, _) = build(lab.parts(Some(lab.gate(Some(permit(NetworkPolicy::None)))))).await.unwrap().unwrap();
    assert_eq!(state_of(&st, "hermes").0, "stopped");
}

#[tokio::test]
async fn the_roster_must_be_a_regular_file_within_the_cap() {
    let lab = lab();
    let try_roster = |path: String| {
        write_cfg(
            lab.path(),
            serde_json::json!({"roster": {"file": path}, "roles": [{"role": "x", "flavor": "llamacpp", "instance_port": 9}]}),
        );
    };
    // A directory.
    try_roster(lab.path().display().to_string());
    let e = build(lab.parts(None)).await.err().unwrap();
    assert!(e.contains("not a regular file"), "{e}");
    // A FIFO is refused without being opened (opening would block).
    let fifo = lab.path().join("queue.fifo");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    try_roster(fifo.display().to_string());
    let e = tokio::time::timeout(std::time::Duration::from_secs(3), build(lab.parts(None))).await.expect("did not block").err().unwrap();
    assert!(e.contains("not a regular file"), "{e}");
    // Over the cap.
    let big = lab.path().join("big.yaml");
    std::fs::write(&big, vec![b'#'; 1024 * 1024 + 10]).unwrap();
    try_roster(big.display().to_string());
    let e = build(lab.parts(None)).await.err().unwrap();
    assert!(e.contains("bytes"), "{e}");
    // A small valid one is read.
    let ok = lab.path().join("ok.yaml");
    std::fs::write(&ok, "roster: []\n").unwrap();
    try_roster(ok.display().to_string());
    assert!(build(lab.parts(None)).await.is_ok());
}

#[tokio::test]
async fn native_api_roles_resolve_only_to_servers_on_this_node() {
    // The voice TTS speaks Ollama's native API, which the mesh does not
    // carry to peers: its role resolves locally or not at all, while a
    // provider role may go through the proxy.
    let (up, up2) = (fake_on(0).await, fake_on(0).await);
    let dir = tempfile::tempdir().unwrap();
    write_cfg(
        dir.path(),
        serde_json::json!({"roles": [
            {"role": "hermes", "flavor": "llamacpp", "instance_port": up.addr.port(), "provider": "local"},
            {"role": "orpheus-tts", "flavor": "llamacpp", "instance_port": up2.addr.port()}]}),
    );
    let (st, _) = build(parts(dir.path(), None, false, None)).await.unwrap().unwrap();
    let providers: std::collections::HashSet<String> = ["hermes".to_string()].into();
    let resolve = role_resolver(st.table.clone(), providers);
    assert!(resolve("orpheus-tts").is_some(), "served here: resolves");
    assert!(resolve("hermes").is_some());
    st.table.deregister_local("orpheus-tts");
    assert!(resolve("orpheus-tts").is_none(), "not served here: the consumer keeps its own endpoint");
}

/// A fake Ollama whose model load always fails.
async fn failing_ollama() -> (u16, tokio::task::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let t = tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let line = head.lines().next().unwrap_or("").to_string();
                let (code, body) = if line.starts_with("POST /api/generate") {
                    (500, "out of memory".to_string())
                } else if line.contains("/api/version") {
                    (200, r#"{"version":"0"}"#.to_string())
                } else if line.contains("/api/tags") {
                    (200, r#"{"models":[{"name":"Model-O:latest"}]}"#.to_string())
                } else {
                    (200, r#"{"models":[]}"#.to_string())
                };
                let resp = format!(
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    (port, t)
}

#[tokio::test]
async fn a_managed_role_whose_ollama_load_failed_stops_claiming_to_run() {
    use clawft_kernel::model_manifest::{AdoptInput, ModelFormat, ModelRegistry, ModelSource, ModelTrust, scan_file};
    use clawft_kernel::workload_pkg::{KeyOrigin, TrustAnchors, codec::hex_encode, key_id_for};
    let lab = lab();
    // An Ollama-format model in the lab's registry.
    let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    let pk = key.verifying_key().to_bytes();
    let id = key_id_for(&pk);
    let mut anchors = TrustAnchors::default();
    anchors.push_signer(&id, &hex_encode(&pk), KeyOrigin::Operator).unwrap();
    let reg = ModelRegistry::open(lab.path().join("models/registry.json")).unwrap();
    reg.set_trust(ModelTrust::new(anchors.clone()));
    let f = lab.path().join("Model-O.gguf");
    std::fs::write(&f, vec![3u8; 2048]).unwrap();
    let scanned = scan_file(
        &f,
        AdoptInput {
            name: "Model-O".into(),
            format: ModelFormat::Ollama,
            source: ModelSource { ollama_tag: Some("Model-O:latest".into()), ..ModelSource::default() },
            redistributable: false,
        },
    )
    .unwrap();
    reg.adopt(scanned, &key, &id, &anchors, false).unwrap();
    let (port, _server) = failing_ollama().await;
    write_cfg(
        lab.path(),
        serde_json::json!({"roles": [{"role": "tts", "mode": "managed", "flavor": "ollama", "model": "Model-O",
            "memory_gb": 4, "instance_port": port}], "budget_gb": 10}),
    );
    let (st, _) = build(lab.parts(Some(lab.gate(Some(permit(NetworkPolicy::None)))))).await.unwrap().unwrap();
    st.start_role("tts").await.unwrap();
    assert_eq!(st.ledger.used(), 4_000_000_000, "reserved while the load runs");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    // The next pass sees the failed load: not running, memory back, reason shown.
    st.sync_once().await;
    let (state, reason) = state_of(&st, "tts");
    assert_eq!(state, "refused");
    assert!(reason.unwrap().contains("load failed"));
    assert_eq!(st.ledger.used(), 0);
    assert!(st.table.resolve("tts").is_none());
    let _ = Arc::strong_count(&st);
}

async fn wait_for(p: &std::path::Path) -> bool {
    crate::infer_managed_tests::wait_for(p).await
}

#[tokio::test]
async fn a_terminal_outcome_stays_down_until_an_explicit_start() {
    use clawft_kernel::workload_runtime::infer::Reconcile;
    for exposed in [false, true] {
        let lab = lab();
        write_cfg(
            lab.path(),
            serde_json::json!({"roles": [managed("hermes", "Model-A", 1.0, free_port(), None)], "serve_programs": lab.serve()}),
        );
        let (st, _) = build(lab.parts(Some(lab.gate(Some(permit(NetworkPolicy::None)))))).await.unwrap().unwrap();
        st.start_role("hermes").await.unwrap();
        let pid_file = lab.path().join("bin/pid.txt");
        assert!(wait_for(&pid_file).await);
        if !exposed {
            // A give-up means the process is dead: make it so.
            let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGKILL).unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        std::fs::remove_file(&pid_file).unwrap();
        let r = st.find("hermes").unwrap();
        let outcome = if exposed {
            Reconcile::StoppedExposed { reachable_on: vec!["192.0.2.1".parse().unwrap()] }
        } else {
            Reconcile::GaveUp { attempts: 5 }
        };
        st.on_reconcile(r, Some(outcome)).await;
        let (state, reason) = state_of(&st, "hermes");
        assert_eq!(state, "refused", "exposed={exposed}");
        assert!(reason.unwrap().contains("explicit infer.start"), "exposed={exposed}");
        assert!(!r.run.lock().unwrap().wanted, "no longer wanted");
        assert_eq!(r.handle.lock().await.is_some(), !exposed, "a stopped-exposed instance is dropped, not reused");
        // Many passes later: nothing was launched, loaded or exposed again.
        for _ in 0..4 {
            st.sync_once().await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(!pid_file.exists(), "exposed={exposed}: it started again by itself");
        assert_eq!(r.handle.lock().await.is_some(), !exposed);
        assert_eq!(state_of(&st, "hermes").0, "refused");
        // The operator's start brings it back.
        st.start_role("hermes").await.unwrap();
        assert!(wait_for(&pid_file).await, "exposed={exposed}: infer.start did not start it");
        st.shutdown().await;
    }
}
