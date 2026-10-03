//! A remote.api node is only registered and placed on over a pinned link
//! (or an explicit lab opt-in).

use std::sync::Arc;

use clawft_types::placement::TrustTier;
use clawft_types::secret::SecretString;

use super::plane::{PlacementControlPlane, PlaneError};
use super::test_support::{anchors, exchange};
use super::transport::MeshConnector;
use crate::chain::ChainManager;
use crate::workload_governance::{NodeTrustTier, WorkloadGate};
use crate::workload_runtime::seed_tls::SeedTls;
use crate::workload_runtime::{
    HttpSeedTransport, RuntimeError, SeedApiRuntime, SeedConfig, SeedCredentials, SeedPin,
    WorkloadHost,
};

struct Creds;
impl SeedCredentials for Creds {
    fn get(&self, _: &str) -> Result<SecretString, RuntimeError> {
        Ok(SecretString::new("seed-token-link-tests".to_string()))
    }
    fn put(&self, _: &str, _: SecretString) -> Result<(), RuntimeError> {
        Ok(())
    }
}

fn host(t: HttpSeedTransport) -> Arc<WorkloadHost> {
    let rt = SeedApiRuntime::new(
        SeedConfig {
            node_id: "seed-link".into(),
            pins: vec![SeedPin::new("fall-detect", "1.0.0")],
            concurrency_cap: 3,
        },
        Arc::new(t),
        Arc::new(Creds),
    )
    .unwrap();
    Arc::new(WorkloadHost::new(
        Arc::new(rt),
        Arc::new(WorkloadGate::new(0.95, false)),
        "operator",
        NodeTrustTier::Paired,
    ))
}

fn plane() -> PlacementControlPlane {
    let chain = Arc::new(ChainManager::new(0, 1000));
    PlacementControlPlane::new(
        ed25519_dalek::SigningKey::from_bytes(&[70; 32]),
        Arc::new(WorkloadGate::new(0.95, false)),
        chain.clone(),
        exchange("ctl", &chain),
        anchors(),
        Arc::new(MeshConnector::new(false)),
    )
}

#[test]
fn an_unpinned_seed_link_is_refused_and_a_pinned_or_opted_in_one_is_accepted() {
    let p = plane();
    // Plain http, and https verified only by WebPKI: refused.
    for t in [
        HttpSeedTransport::new("http://169.254.42.1", SeedTls::WebPki).unwrap(),
        HttpSeedTransport::new("https://seed.example:8443", SeedTls::WebPki).unwrap(),
    ] {
        let e = p
            .add_seed("seed-link", host(t), TrustTier::Paired)
            .unwrap_err();
        assert!(matches!(e, PlaneError::Invalid(_)), "{e}");
        assert!(e.to_string().contains("pinned transport"), "{e}");
    }
    // Pinned certificate or key over https: accepted.
    for tls in [SeedTls::PinnedSha256([1; 32]), SeedTls::PinnedSpki([2; 32])] {
        let t = HttpSeedTransport::new("https://seed.example:8443", tls).unwrap();
        p.add_seed("seed-link", host(t), TrustTier::Paired).unwrap();
    }
    // The explicit lab opt-in on a USB link: accepted.
    let t = HttpSeedTransport::new("http://169.254.42.1", SeedTls::WebPki)
        .unwrap()
        .allow_unpinned_lab_link();
    p.add_seed("seed-link", host(t), TrustTier::Paired).unwrap();
}
