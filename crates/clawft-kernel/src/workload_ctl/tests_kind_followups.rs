//! P2-B follow-ups: the registry reaches the fetch path, prepare refuses a
//! package swapped under it, the Cognitum verifier is cog-only, and manifest
//! text echoed into errors is bounded.

use std::path::Path;
use std::sync::Arc;

use ed25519_dalek::SigningKey;

use super::plane::PlaneError;
use super::test_support::*;
use super::tests_kind::{Echo, echo_package, order};
use super::transport::MeshConnector;
use crate::workload_kind::{CogKind, KindRegistry, WorkloadKind};
use crate::workload_pkg::{
    MANIFEST_FILE, ManifestEnvelope, TrustAnchors, VerifyError, VerifyPolicy, verify_dir_in,
};
use crate::workload_runtime::VerifiedWorkload;

fn echo_registry() -> KindRegistry {
    let mut reg = KindRegistry::builtin();
    reg.register(Arc::new(Echo)).unwrap();
    reg
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

/// A `cog` kind whose `load` runs `swap` on the package directory first, as
/// a racing writer could between prepare's read and the kind's.
struct SwapDuringLoad {
    swap: Box<dyn Fn(&Path) + Send + Sync>,
}

impl WorkloadKind for SwapDuringLoad {
    fn id(&self) -> &'static str {
        "cog"
    }
    fn validate(&self, env: &ManifestEnvelope) -> Result<(), crate::workload_pkg::ManifestError> {
        env.cog_body().map(|_| ())
    }
    fn load(
        &self,
        dir: &Path,
        anchors: &TrustAnchors,
        kinds: &KindRegistry,
    ) -> Result<VerifiedWorkload, PlaneError> {
        (self.swap)(dir);
        CogKind.load(dir, anchors, kinds)
    }
    fn spec(
        &self,
        w: &VerifiedWorkload,
    ) -> Result<clawft_types::placement::engine::WorkloadSpec, String> {
        CogKind.spec(w)
    }
    fn adapters(&self) -> &'static [&'static str] {
        &["native"]
    }
}

fn prepare_with_swap(pkg: &Path, swap: impl Fn(&Path) + Send + Sync + 'static) -> PlaneError {
    let mut reg = KindRegistry::new();
    reg.register(Arc::new(SwapDuringLoad {
        swap: Box::new(swap),
    }))
    .unwrap();
    let (plane, _c) = controller(
        &SigningKey::from_bytes(&[45; 32]),
        Arc::new(MeshConnector::new(false)),
    );
    plane
        .with_kind_registry(reg)
        .prepare(&order(pkg))
        .unwrap_err()
}

#[test]
fn prepare_refuses_a_manifest_swapped_after_it_was_read() {
    use crate::workload_pkg::{key_id_for, sign_envelope};
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "kind-first", "#!/bin/sh\nexit 0\n", &[arch()]);
    // Same files, re-signed with another version: valid on its own, but not
    // the package prepare read first.
    let mut env =
        ManifestEnvelope::from_bytes(&std::fs::read(pkg.join(MANIFEST_FILE)).unwrap()).unwrap();
    env.body["version"] = "0.2.0".into();
    env.signatures.clear();
    let k = signer();
    sign_envelope(&mut env, &k, &key_id_for(&k.verifying_key().to_bytes())).unwrap();
    let swapped = env.to_pretty_json().unwrap();
    let err = prepare_with_swap(&pkg, move |dir| {
        std::fs::write(dir.join(MANIFEST_FILE), &swapped).unwrap();
    });
    assert!(
        matches!(&err, PlaneError::Package(m) if m.contains("changed between")),
        "{err:?}"
    );
}

#[test]
fn prepare_refuses_a_whole_package_swapped_after_the_manifest_was_read() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let pkg = package(a.path(), "kind-first", "#!/bin/sh\nexit 0\n", &[arch()]);
    let other = package(b.path(), "kind-second", "#!/bin/sh\nexit 1\n", &[arch()]);
    let err = prepare_with_swap(&pkg, move |dir| {
        std::fs::remove_dir_all(dir).unwrap();
        copy_dir(&other, dir);
    });
    // The manifest bytes read first are what seeding verifies the files
    // against, so the other package's files do not match them.
    assert!(
        matches!(&err, PlaneError::Package(m) if m.contains("tampered")),
        "{err:?}"
    );
}

#[test]
fn cognitum_verifier_is_refused_for_a_non_cog_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = echo_package(tmp.path());
    let on = VerifyPolicy {
        accept_cognitum_release: true,
        ..VerifyPolicy::default()
    };
    let err = verify_dir_in(&pkg, &anchors(), &on, &echo_registry()).unwrap_err();
    assert!(
        matches!(&err, VerifyError::Manifest(m) if m.contains("only to the cog kind")),
        "{err:?}"
    );
    // Off, the same package verifies; and the cog kind may turn it on.
    assert!(verify_dir_in(&pkg, &anchors(), &VerifyPolicy::default(), &echo_registry()).is_ok());
    let cog = package(
        tempfile::tempdir().unwrap().path(),
        "kind-cog",
        "#!/bin/sh\nexit 0\n",
        &[arch()],
    );
    let r = verify_dir_in(&cog, &anchors(), &on, &KindRegistry::builtin());
    assert!(
        !matches!(&r, Err(VerifyError::Manifest(m)) if m.contains("only to the cog kind")),
        "{r:?}"
    );
}

#[test]
fn manifest_text_echoed_into_errors_is_bounded() {
    let long = "k".repeat(300);
    for (field, doc) in [
        (
            "kind",
            format!(
                r#"{{"schema":"weftos.workload-package.v1","kind":"{long}","body":{{}},"signatures":[]}}"#
            ),
        ),
        (
            "schema",
            format!(r#"{{"schema":"{long}","kind":"cog","body":{{}},"signatures":[]}}"#),
        ),
    ] {
        let err = ManifestEnvelope::from_bytes(doc.as_bytes())
            .unwrap_err()
            .to_string();
        assert!(err.len() < 120, "{field}: {} bytes: {err}", err.len());
        assert!(!err.contains(&"k".repeat(33)), "{field}: {err}");
    }
}

#[cfg(feature = "native")]
#[tokio::test]
async fn a_registered_kind_fetches_across_nodes_only_with_its_registry() {
    use crate::mesh_artifact_tests::{connect, link, node};
    let tmp = tempfile::tempdir().unwrap();
    let pkg = echo_package(tmp.path());
    let reg = echo_registry();
    let (a, b) = (node("node-a"), node("node-b"));
    let seeded = a.ex.seed_package_dir_in(&pkg, &anchors(), &reg).unwrap();

    // The default (builtin) fetch refuses the kind: fails closed.
    let (s, server) = connect(&a, "node-b").await;
    let mut peers = link("node-a", s);
    let err =
        b.ex.fetch_package(&mut peers, &seeded.manifest_hash, &anchors())
            .await
            .unwrap_err();
    drop(peers);
    let _ = server.await;
    assert!(err.to_string().contains("unknown workload kind"), "{err}");

    // With the registry threaded through, the same fetch verifies.
    let (s, server) = connect(&a, "node-b").await;
    let mut peers = link("node-a", s);
    let got =
        b.ex.fetch_package_in(&mut peers, &seeded.manifest_hash, &anchors(), &reg)
            .await
            .expect("registered kind fetches with its registry");
    drop(peers);
    let _ = server.await;
    assert_eq!(got.package_id, seeded.package_id);
}
