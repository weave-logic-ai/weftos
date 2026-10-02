//! P3-U: `mesh_boot::prepare` for the user daemon, end to end with real peer
//! credentials (the in-process loopback server runs as this uid, which is
//! also the `service_uid` it advertises). One test per binary: it sets
//! `HOME`, `WEFTOS_MESH_STATE_DIR` and enters the user profile.
#![cfg(all(unix, feature = "mesh"))]

use std::sync::Arc;

use clawft_mesh_local::testing::{TestServer, TestServerConfig};
use clawft_mesh_local::{InjectedPeer, node_id_from_pubkey};
use clawft_types::config::{KernelConfig, MeshConfig, MeshServicePolicy};
use clawft_weave::{mesh_boot, mesh_state, user_daemon};
use ed25519_dalek::SigningKey;

#[tokio::test]
async fn user_daemon_enters_service_mode_without_touching_node_key() {
    let home = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let run = tempfile::tempdir().unwrap();
    let sock = state_dir.path().join("mesh.sock");
    let me = nix::unistd::geteuid().as_raw();

    let machine = SigningKey::from_bytes(&[100u8; 32]);
    let server = TestServer::start(
        &sock,
        TestServerConfig::new(machine, Arc::new(InjectedPeer::uid(me))),
    )
    .unwrap();
    // service.json as the real service writes it; the client pins from it.
    let record = server.service_record(me);
    std::fs::write(state_dir.path().join("service.json"), serde_json::to_vec(&record).unwrap()).unwrap();

    // SAFETY: this binary has a single test, so nothing reads the environment
    // concurrently.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var(clawft_weave::mesh_local_glue::STATE_DIR_ENV, state_dir.path());
    }
    user_daemon::enter();

    let kcfg = KernelConfig {
        mesh: Some(MeshConfig {
            enabled: true,
            service: MeshServicePolicy::Required,
            service_socket: Some(sock.display().to_string()),
            ..MeshConfig::default()
        }),
        ..KernelConfig::default()
    };
    let boot = mesh_boot::prepare(&kcfg, run.path()).await.expect("service mode boot");
    let link = boot.link.as_ref().expect("a registered link");
    assert_eq!(link.node_id(), node_id_from_pubkey(&server.machine_pubkey()));
    assert_eq!(boot.identity.node_id, link.node_id());
    assert!(boot.identity.is_service() && boot.identity.signing_key().is_err());
    assert!(!run.path().join("node.key").exists(), "service mode never reads or creates node.key");
    assert!(
        home.path().join(".weftos/user.key").exists(),
        "a fresh user key was created for the registration"
    );
    assert!(home.path().join(".weftos/mesh/machine.pub").exists(), "machine key pinned on first contact");
    assert_eq!(server.registrations().len(), 1);

    // The handshake now reports the user role only, and the mesh mode.
    mesh_state::global().set(clawft_rpc::handshake::MeshHandshake {
        mode: "service".into(),
        state: Some("connected".into()),
        ..Default::default()
    });
    let (profile, roles) = user_daemon::handshake_profile().unwrap();
    assert_eq!(profile, "user");
    assert_eq!(roles, ["user"]);

    // `required` without a service socket is a boot failure with the reason.
    drop(boot);
    drop(server);
    let e = mesh_boot::prepare(&kcfg, run.path()).await.err().expect("no service");
    assert!(e.to_string().contains("required"), "{e}");
    assert!(!run.path().join("node.key").exists(), "no fallback key was generated");
    user_daemon::leave();
}
