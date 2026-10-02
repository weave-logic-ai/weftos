//! Review S2: under `auto`, a user daemon on a machine that has used the mesh
//! service (machine key pinned) and has no local node.key refuses to collapse
//! with a fresh node id when the service is down. One test per binary: it sets
//! `HOME` and enters the user profile.
#![cfg(all(unix, feature = "mesh"))]

use clawft_types::config::{KernelConfig, MeshConfig, MeshServicePolicy};
use clawft_weave::{mesh_boot, user_daemon};

#[tokio::test]
async fn auto_refuses_a_fresh_node_id_after_the_service_was_used() {
    let home = tempfile::tempdir().unwrap();
    let run = tempfile::tempdir().unwrap();
    // SAFETY: this binary has a single test, so nothing reads the environment
    // concurrently.
    unsafe { std::env::set_var("HOME", home.path()) };
    user_daemon::enter();
    let cfg = |policy| KernelConfig {
        mesh: Some(MeshConfig {
            enabled: true,
            service: policy,
            service_socket: Some(home.path().join("no-service.sock").display().to_string()),
            ..MeshConfig::default()
        }),
        ..KernelConfig::default()
    };
    std::fs::create_dir_all(home.path().join(".weftos/mesh")).unwrap();
    std::fs::write(home.path().join(".weftos/mesh/machine.pub"), "00".repeat(32)).unwrap();

    let e = mesh_boot::prepare(&cfg(MeshServicePolicy::Auto), run.path()).await.err().expect("refused");
    assert!(e.to_string().contains("NEW node id"), "{e}");
    assert!(!run.path().join("node.key").exists(), "no key was generated");

    // `off` is the deliberate way to collapse with a new key.
    let boot = mesh_boot::prepare(&cfg(MeshServicePolicy::Off), run.path()).await.expect("off collapses");
    assert!(boot.link.is_none() && run.path().join("node.key").exists());
    // With a key present (rollback), auto collapses on it as before.
    mesh_boot::prepare(&cfg(MeshServicePolicy::Auto), run.path()).await.expect("auto with a node.key");
    user_daemon::leave();
}
