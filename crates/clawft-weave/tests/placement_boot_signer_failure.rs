//! ADR-106: a service-mode daemon whose placement signer cannot be loaded
//! still has its licence role set to "unknown" before boot returns, so the
//! licence role gate refuses the licence verbs instead of reading the daemon
//! as collapsed (not applicable).
//!
//! One process per test file: the module-wide licence state is this test's.
#![cfg(all(unix, feature = "placement"))]

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use clawft_kernel::Kernel;
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig};
use clawft_weave::licence_boot::{self, HolderState};
use clawft_weave::node_identity::DaemonIdentity;
use clawft_weave::{licence_role_gate, placement_boot, workload_place_rpc};
use ed25519_dalek::SigningKey;
use serde_json::json;
use tokio::sync::RwLock;

#[tokio::test]
async fn a_signer_failure_in_service_mode_leaves_the_role_unknown_and_the_verbs_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = tmp.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    // A malformed control key: the signer cannot load it.
    let bad = runtime.join(placement_boot::CONTROL_KEY_FILE);
    std::fs::write(&bad, b"short").unwrap();
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o600)).unwrap();

    let machine = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
    let id = clawft_kernel::node_id_from_pubkey(&machine);
    let identity = DaemonIdentity::for_service(id.clone(), machine).unwrap();
    assert!(placement_boot::signer(&identity, &runtime).is_err(), "the signer really fails");
    let kcfg = KernelConfig { chain: Some(ChainConfig::isolated_in(&tmp.path().join("chain"))), ..KernelConfig::default() };
    let kernel = Kernel::boot_in_service_mode(Config::default(), kcfg, Arc::new(NativePlatform::new()), id)
        .await
        .expect("kernel boots in service mode");
    let kernel = Arc::new(RwLock::new(kernel));

    placement_boot::start(&kernel, &identity, &runtime).await;
    assert_eq!(licence_boot::holder_state(), HolderState::Unknown, "not left as not-applicable");
    assert!(workload_place_rpc::runtime_dir().is_none(), "placement stayed off");
    let caller = clawft_weave::rpc_ext::CallerCtx::from_auth(Some("admin".into()));
    let caps = clawft_weave::capability::CallerCapabilities::from_scopes(["admin"]);
    let r = clawft_weave::rpc_ext::authorize(&caller, &caps, "workload.cog.checkout", &json!({}), &kernel).await;
    let resp = r.expect_err("the role gate refuses while the role is unknown");
    assert_eq!(resp.error_kind.as_deref(), Some(licence_role_gate::NOT_HERE_KIND));
    assert!(resp.error.unwrap_or_default().contains("holder status unknown"));
}
