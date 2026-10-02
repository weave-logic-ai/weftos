//! `weaver mesh ...` against a live service on tempdirs, as the current user
//! (the real peer credential; the user is the admin).
#![cfg(all(unix, feature = "mesh"))]

use clawft_mesh_local::client::{ClientConfig, MeshLocalClient, RegisterParams};
use clawft_mesh_local::peer::own_uid;
use clawft_mesh_local::proto::ServiceRecord;
use clawft_mesh_service::{start, MeshServiceConfig};
use clawft_weave::commands::mesh_cmd::{execute, BindCmd, ConnArgs, JournalCmd, MeshCmd, PeerCmd, TrustArgs};
use ed25519_dalek::SigningKey;

async fn run(cmd: MeshCmd) -> anyhow::Result<String> {
    let mut out = Vec::new();
    execute(cmd, &mut out).await?;
    Ok(String::from_utf8(out)?)
}

#[tokio::test]
async fn weaver_mesh_verbs_work_against_a_live_service() {
    let euid = own_uid().await.unwrap();
    let dir = tempfile::Builder::new().prefix("w").tempdir().unwrap();
    let sock = dir.path().join("r").join("s");
    let cfg = MeshServiceConfig {
        state_dir: dir.path().join("st"),
        socket: sock.clone(),
        listen: "127.0.0.1:0".into(),
        health_listen: Some("127.0.0.1:0".into()),
        probe_facts: false,
        admin_uids: vec![euid],
        ..MeshServiceConfig::default()
    };
    let svc = start(cfg).await.expect("service starts");
    let pin = dir.path().join("pin");
    let conn = ConnArgs { socket: Some(sock.clone()), pin: Some(pin.clone()), json: false };

    let out = run(MeshCmd::Status(conn.clone())).await.unwrap();
    assert!(out.contains(&svc.node_id) && out.contains("admission observe"), "{out}");

    // `trust` shows the fingerprint and pins the key; later verbs compare against the pin.
    let out = run(MeshCmd::Trust(TrustArgs { conn: conn.clone(), replace: false })).await.unwrap();
    assert!(out.contains("fingerprint") && out.contains("pinned in"), "{out}");
    assert_eq!(std::fs::read_to_string(&pin).unwrap().trim().len(), 64);
    run(MeshCmd::Trust(TrustArgs { conn: conn.clone(), replace: false })).await.expect("trust is idempotent");
    run(MeshCmd::Status(conn.clone())).await.expect("status passes the pin check");

    // A different pinned key is a hard error naming the remedy.
    let other = dir.path().join("other-pin");
    std::fs::write(&other, format!("{}\n", "ab".repeat(32))).unwrap();
    let bad = ConnArgs { pin: Some(other), ..conn.clone() };
    let err = run(MeshCmd::Status(bad)).await.unwrap_err().to_string();
    assert!(err.contains("machine_key_changed"), "{err}");

    // Register a user, then list, verify, revoke.
    let record = ServiceRecord::load(&dir.path().join("r").join("service.json")).unwrap();
    let client = MeshLocalClient::connect_and_register(
        &ClientConfig::new(&sock, record),
        &SigningKey::from_bytes(&[5; 32]),
        &RegisterParams::default(),
    )
    .await
    .unwrap();
    let out = run(MeshCmd::Bindings(conn.clone())).await.unwrap();
    assert!(out.contains(&format!("bound    uid {euid}")) && out.contains("(registered)"), "{out}");
    let out = run(MeshCmd::Journal { cmd: JournalCmd::Verify { accept_truncate: false, seq: None, floor: None, conn: conn.clone() } })
        .await
        .unwrap();
    assert!(out.contains("journal verifies"), "{out}");
    let err = run(MeshCmd::Journal { cmd: JournalCmd::Verify { accept_truncate: true, seq: None, floor: None, conn: conn.clone() } })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("nothing is pending"), "accepting with no quarantine is refused: {err}");

    let out = run(MeshCmd::Bind { cmd: BindCmd::Revoke { uid: euid, reason: "test".into(), conn: conn.clone() } }).await.unwrap();
    assert!(out.contains("revoked"), "{out}");
    let out = run(MeshCmd::Bindings(conn.clone())).await.unwrap();
    assert!(out.contains(&format!("revoked  uid {euid}")), "{out}");
    drop(client);

    run(MeshCmd::Peer { cmd: PeerCmd::Revoke { node_id: "d".repeat(32), reason: "x".into(), conn: conn.clone() } }).await.unwrap();
    run(MeshCmd::Peer { cmd: PeerCmd::Unrevoke { node_id: "d".repeat(32), conn: conn.clone() } }).await.unwrap();

    assert!(run(MeshCmd::Bind { cmd: BindCmd::Rebind { uid: euid, pubkey: Some("zz".into()), conn: conn.clone() } })
        .await
        .is_err());
    let json = run(MeshCmd::Status(ConnArgs { json: true, ..conn.clone() })).await.unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&json).unwrap()["node_id"], svc.node_id.as_str());

    svc.shutdown().await;
    let err = run(MeshCmd::Status(conn)).await.unwrap_err().to_string();
    assert!(err.contains("service.json") || err.contains("running"), "{err}");
}
