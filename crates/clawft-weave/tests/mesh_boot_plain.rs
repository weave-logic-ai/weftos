//! P3-U: `mesh_boot::prepare` for daemons that keep their own mesh (not the
//! user profile). One process, no profile entered.
#![cfg(all(unix, feature = "mesh"))]

use clawft_types::config::{KernelConfig, MeshConfig, MeshServicePolicy};
use clawft_weave::{mesh_boot, mesh_state};

#[tokio::test]
async fn non_user_daemons_keep_their_own_node_key_and_mesh() {
    let run = tempfile::tempdir().unwrap();
    let boot = mesh_boot::prepare(&KernelConfig::default(), run.path()).await.unwrap();
    assert!(!boot.identity.is_service() && boot.link.is_none());
    assert!(run.path().join("node.key").exists());
    assert_eq!(mesh_state::global().get().unwrap().mode, "off");

    let enabled = KernelConfig {
        mesh: Some(MeshConfig { enabled: true, ..MeshConfig::default() }),
        ..KernelConfig::default()
    };
    mesh_boot::prepare(&enabled, run.path()).await.unwrap();
    assert_eq!(mesh_state::global().get().unwrap().mode, "collapsed");

    // `required` is meaningless for a daemon that cannot link a service.
    let required = KernelConfig {
        mesh: Some(MeshConfig {
            enabled: true,
            service: MeshServicePolicy::Required,
            ..MeshConfig::default()
        }),
        ..KernelConfig::default()
    };
    let e = mesh_boot::prepare(&required, run.path()).await.err().unwrap();
    assert!(e.to_string().contains("only supported by the user daemon"), "{e}");
}
