//! Phase 2 cross-check: grants signed by the real `weft-licence` service
//! (driven in process, stub licence and registry) are accepted by the 1a
//! member-side stores, including renewal, the arch union and withdrawal.

use std::collections::BTreeMap;
use std::sync::Arc;

use weft_licence::providers::*;
use weft_licence::request::{Request, sign_request};
use weft_licence::state::OperatorKeys;
use weft_licence::{Config, Service};

use super::tests_common::*;
use super::*;

struct Lic;
impl LicenceProvider for Lic {
    fn entitlement(&self, _: &str, _: &str, _: u64) -> Result<Entitlement, LicenceCheckError> {
        Ok(Entitlement { ref_sha256: sha256_hex(b"licence"), expires: None })
    }
}

struct Reg(BTreeMap<String, Vec<u8>>);
impl CogFetcher for Reg {
    fn resolve(&self, cog: &str, _: &str) -> Result<CogEntry, FetchError> {
        let artifacts = self
            .0
            .iter()
            .map(|(a, b)| EntryArtifact { arch: a.clone(), size: b.len() as u64, sha256: sha256_hex(b) })
            .collect();
        Ok(CogEntry {
            cog_id: cog.into(),
            version: "1.2.0".into(),
            registry: "registry.example".into(),
            manifest_sha256: sha256_hex(b"manifest"),
            artifacts,
        })
    }
    fn fetch(&self, _: &CogEntry, arch: &str) -> Result<Vec<u8>, FetchError> {
        self.0.get(arch).cloned().ok_or(FetchError::ArchUnavailable(arch.into()))
    }
}

fn call(svc: &Service, n: &mut u64, m: &str, target: &str, body: &[u8]) -> serde_json::Value {
    *n += 1;
    let steward = sk(21);
    let headers = sign_request(&steward, "node-steward", m, target, body, T0 * 1000, &format!("{:032x}", *n));
    let r = svc.handle(&Request { method: m.into(), target: target.into(), headers, body: body.to_vec() });
    assert_eq!(r.status, 200, "{:?}", r.json_body());
    r.json_body().unwrap()
}

#[test]
fn grants_from_the_seed_service_are_accepted_by_the_member_stores() {
    // The Seed side: init the key, apply the operator-signed binding.
    let seed_dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        state_dir: seed_dir.path().join("state"),
        device_id: "seed-test".into(),
        operator_pubkeys: vec![pk_hex(&op())],
        clock_floor: 0,
        ..Config::default()
    };
    let init = weft_licence::keys::init(&cfg.state_dir).unwrap();
    let mut rec = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
    rec.grant_pubkey = init.grant_pubkey.clone();
    let signed_binding = sign_binding(&rec, &op()).unwrap();
    let ops = OperatorKeys::load(&cfg.state_dir, &cfg.operator_pubkeys).unwrap();
    weft_licence::bind::apply(&cfg.state_dir, "seed-test", &ops, &signed_binding, None).unwrap();
    let regs = BTreeMap::from([("aarch64".to_string(), b"bin-aarch64".to_vec()), ("arm".to_string(), b"bin-arm".to_vec())]);
    let svc = Service::open(
        cfg,
        Arc::new(|| T0),
        Box::new(Lic),
        Box::new(Reg(regs)),
        Box::new(StubDeviceSigner),
    )
    .unwrap();

    // The member side: the same signed binding, accepted under enforce.
    let fx = Fx::new();
    fx.store.accept_binding(&signed_binding, posture(), &NoExtraChecks).unwrap();

    let mut n = 0u64;
    let req = |arch: &str| format!(r#"{{"request_id":"r","cog_id":"fall-detect","version":"latest","arch":"{arch}"}}"#);
    let j = call(&svc, &mut n, "POST", "/licence/v1/checkout", req("aarch64").as_bytes());
    let g1: SignedGrant = serde_json::from_value(j["grant"].clone()).unwrap();
    assert_eq!(fx.store.accept_grant(&g1), Ok(Outcome::Applied));
    // BLAKE3 in the grant is the swarm content hash the member looks up.
    assert!(fx.store.valid_grant_covering(&b3_of("aarch64"), "fall-detect", "1.2.0").is_some());

    // A second arch carries the union; the store accepts it (no DropsArch).
    let j = call(&svc, &mut n, "POST", "/licence/v1/checkout", req("arm").as_bytes());
    let g2: SignedGrant = serde_json::from_value(j["grant"].clone()).unwrap();
    assert_eq!(fx.store.accept_grant(&g2), Ok(Outcome::Applied));
    for a in ["aarch64", "arm"] {
        assert!(fx.store.valid_grant_covering(&b3_of(a), "fall-detect", "1.2.0").is_some(), "{a}");
    }

    // Renewal: a higher seq, accepted.
    let j = call(&svc, &mut n, "POST", "/licence/v1/renew", b"");
    let renewed: SignedGrant = serde_json::from_value(j["grants"][0].clone()).unwrap();
    assert_eq!(fx.store.accept_grant(&renewed), Ok(Outcome::Applied));

    // Withdrawal (release): accepted, and it stops serving at once.
    let rel = br#"{"release":[{"cog_id":"fall-detect","version":"1.2.0"}]}"#;
    let j = call(&svc, &mut n, "POST", "/licence/v1/renew", rel);
    let w: SignedGrant = serde_json::from_value(j["grants"][0].clone()).unwrap();
    assert_eq!(fx.store.accept_grant(&w), Ok(Outcome::Applied));
    assert!(fx.store.valid_grant_covering(&b3_of("arm"), "fall-detect", "1.2.0").is_none());
}
