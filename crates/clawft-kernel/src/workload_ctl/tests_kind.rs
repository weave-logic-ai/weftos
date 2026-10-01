//! P2-B: the workload-kind registry on the placement path. The cog path
//! through the registry matches the pre-registry results.

use std::sync::Arc;

use ed25519_dalek::SigningKey;

use super::host_service::CtlConfig;
use super::plane::PlaneError;
use super::plane_place::PlaceOrder;
use super::test_support::*;
use super::transport::MeshConnector;
use crate::chain;
use crate::workload_kind::{CogKind, KindRegistry, WorkloadKind, validate_envelope};
use crate::workload_pkg::{
    DirSource, KIND_COG, MANIFEST_FILE, ManifestEnvelope, VerifyPolicy, verify_dir,
};
use crate::workload_runtime::{RunMode, VerifiedWorkload};

fn order(pkg: &std::path::Path) -> PlaceOrder {
    PlaceOrder {
        package_dir: pkg.to_path_buf(),
        config: CtlConfig {
            mode: RunMode::Listener,
            args: vec![],
            csi_port: 15027,
        },
        pin: None,
        prefer: vec![],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: true,
    }
}

#[test]
fn cog_round_trip_through_the_registry_matches_direct_calls() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "kind-cog", "#!/bin/sh\nexit 0\n", &[arch()]);
    let anchors = anchors();
    let env =
        ManifestEnvelope::from_bytes(&std::fs::read(pkg.join(MANIFEST_FILE)).unwrap()).unwrap();

    // Validation: same outcome as the old direct `cog_body` call.
    let reg = KindRegistry::builtin();
    assert!(validate_envelope(&reg, &env).is_ok() && env.cog_body().is_ok());

    // Load + spec: identical to the pre-registry sequence.
    let verified = verify_dir(&pkg, &anchors, &VerifyPolicy::default()).unwrap();
    let direct = VerifiedWorkload::from_package(&verified, &DirSource::new(&pkg)).unwrap();
    let direct_spec = super::cog_workload_spec(&direct).unwrap();
    let kind = reg.require(KIND_COG).unwrap();
    let via = kind.load(&pkg, &anchors).unwrap();
    assert_eq!(
        (&via.id, &via.kind, &via.version),
        (&direct.id, &direct.kind, &direct.version)
    );
    assert_eq!(
        serde_json::to_value(kind.spec(&via).unwrap()).unwrap(),
        serde_json::to_value(direct_spec).unwrap()
    );
    assert_eq!(CogKind.adapters(), kind.adapters());
}

#[tokio::test]
async fn unregistered_kind_is_refused_and_chained() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "kind-unknown", "#!/bin/sh\nexit 0\n", &[arch()]);
    let text = std::fs::read_to_string(pkg.join(MANIFEST_FILE)).unwrap();
    std::fs::write(
        pkg.join(MANIFEST_FILE),
        text.replace("\"kind\": \"cog\"", "\"kind\": \"model\""),
    )
    .unwrap();

    let (plane, chain_mgr) = controller(
        &SigningKey::from_bytes(&[41; 32]),
        Arc::new(MeshConnector::new(false)),
    );
    let err = plane.place(&order(&pkg)).await.unwrap_err();
    assert!(
        matches!(&err, PlaneError::UnknownKind(k) if k.0 == "model"),
        "{err:?}"
    );
    let refused = events(&chain_mgr, chain::EVENT_KIND_WORKLOAD_REFUSE);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].1["phase"], "kind");
}

#[tokio::test]
async fn a_registry_without_cog_refuses_cog_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "kind-nocog", "#!/bin/sh\nexit 0\n", &[arch()]);
    let (plane, _c) = controller(
        &SigningKey::from_bytes(&[42; 32]),
        Arc::new(MeshConnector::new(false)),
    );
    let plane = plane.with_kind_registry(KindRegistry::new());
    let err = plane.place(&order(&pkg)).await.unwrap_err();
    assert!(matches!(err, PlaneError::UnknownKind(_)));
}
