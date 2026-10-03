//! Review-round fixes: withdrawal-safe conflicts, save failures, read path,
//! key and shape bounds.

use std::sync::Arc;

use super::tests_common::*;
use super::*;

const DAY: u64 = 86_400;

fn put(fx: &Fx, g: &SignedGrant) -> Result<Outcome, LicenceError> {
    fx.store.accept_grant(g)
}

fn covered(fx: &Fx, arch: &str) -> bool {
    fx.store.valid_grant_covering(&b3_of(arch), "fall-detect", "1.2.0").is_some()
}

fn open_grants(dir: &std::path::Path) -> Result<CheckoutGrantStore, LicenceError> {
    CheckoutGrantStore::open(dir, anchors(), LocalMeshId::new(mesh()), system_clock())
}


#[test]
fn a_same_seq_conflict_does_not_undo_a_withdrawal() {
    let fx = Fx::new();
    fx.bind();
    put(&fx, &grant(1, T0, DAY, &["aarch64"])).unwrap();
    put(&fx, &grant(2, T0, DAY, &["aarch64"])).unwrap();
    put(&fx, &grant(3, T0 + 10, 0, &["aarch64"])).unwrap(); // withdrawal
    assert!(!covered(&fx, "aarch64"));
    let rival = grant(3, T0 + 11, DAY, &["aarch64"]);
    assert_eq!(put(&fx, &rival), Err(LicenceError::Conflict(3)));
    assert!(!covered(&fx, "aarch64"), "the withdrawal tombstone stays");
    assert_eq!(put(&fx, &grant(2, T0, DAY, &["aarch64"])), Ok(Outcome::Ignored));
    assert!(!covered(&fx, "aarch64"));
}

#[test]
fn a_rival_withdrawal_at_a_live_grants_seq_wins_the_conflict() {
    let fx = Fx::new();
    fx.bind();
    put(&fx, &grant(1, T0, DAY, &["aarch64"])).unwrap();
    put(&fx, &grant(2, T0, DAY, &["aarch64"])).unwrap();
    let rival = grant(2, T0 + 10, 0, &["aarch64"]);
    assert_eq!(put(&fx, &rival), Err(LicenceError::Conflict(2)));
    assert!(!covered(&fx, "aarch64"), "fail closed, not back to seq 1");
}

fn block_saves(fx: &Fx) {
    let p = fx.dir.path().join("checkout_grants.json");
    std::fs::remove_file(&p).unwrap();
    std::fs::create_dir(&p).unwrap(); // a rename onto a directory fails
}

fn unblock_saves(fx: &Fx) {
    std::fs::remove_dir(fx.dir.path().join("checkout_grants.json")).unwrap();
}

#[test]
fn a_failed_save_leaves_memory_unchanged() {
    let fx = Fx::new();
    fx.bind();
    block_saves(&fx);
    let g = grant(1, T0, DAY, &["aarch64"]);
    assert!(matches!(put(&fx, &g), Err(LicenceError::Persist(_))));
    assert!(!covered(&fx, "aarch64"));
    // A binding that cannot be saved is not taken either.
    let rebind = binding_rec(2, BindState::Bound, &sk(40), &mesh());
    let r = fx.store.accept_binding(
        &sign_binding(&rebind, &op()).unwrap(),
        posture(),
        &NoExtraChecks,
    );
    assert!(matches!(r, Err(LicenceError::Persist(_))));
    assert_eq!(fx.store.active_binding().unwrap().grant_pubkey, pk_hex(&grant_key()));
    unblock_saves(&fx);
    assert_eq!(put(&fx, &g), Ok(Outcome::Applied), "not a duplicate: nothing was kept");
}

#[test]
fn restrictive_records_apply_even_when_the_save_fails_and_tick_retries() {
    let mut fx = Fx::new();
    fx.bind();
    put(&fx, &grant(1, T0, DAY, &["aarch64"])).unwrap();
    block_saves(&fx);
    let w = grant(2, T0 + 10, 0, &["aarch64"]);
    assert!(matches!(put(&fx, &w), Err(LicenceError::Persist(_))));
    assert!(!covered(&fx, "aarch64"), "a disk fault must not keep a withdrawn grant alive");
    unblock_saves(&fx);
    fx.store.tick(); // retries the save
    fx.restart();
    assert!(!covered(&fx, "aarch64"));
}

#[test]
fn reads_never_write_the_file() {
    let fx = Fx::new();
    fx.bind();
    put(&fx, &grant(1, T0, DAY, &["aarch64"])).unwrap();
    let p = fx.dir.path().join("checkout_grants.json");
    std::fs::remove_file(&p).unwrap();
    fx.set_now(T0 + 3600);
    assert!(covered(&fx, "aarch64"));
    let _ = fx.store.floor();
    let _ = fx.store.verified_grants();
    assert!(!p.exists(), "only tick() persists the high-water mark");
    fx.store.tick();
    assert!(p.exists());
}

#[test]
fn a_binding_naming_a_trust_anchor_as_grant_or_steward_key_is_refused() {
    let fx = Fx::new();
    for anchor in [op(), sk(5)] {
        let mut r = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
        r.grant_pubkey = pk_hex(&anchor);
        let s = sign_binding(&r, &op()).unwrap();
        let got = fx.store.accept_binding(&s, posture(), &NoExtraChecks);
        assert!(matches!(got, Err(LicenceError::Malformed(_))), "grant key");
        let mut r = binding_rec(1, BindState::Bound, &grant_key(), &mesh());
        r.steward_pubkey = pk_hex(&anchor);
        let s = sign_binding(&r, &op()).unwrap();
        let got = fx.store.accept_binding(&s, posture(), &NoExtraChecks);
        assert!(matches!(got, Err(LicenceError::Malformed(_))), "steward key");
    }
}

#[test]
fn grant_shape_bounds_are_enforced() {
    let pk = grant_key().verifying_key().to_bytes();
    let check = |f: &dyn Fn(&mut CheckoutGrant)| {
        let mut r = grant_rec(1, T0, DAY, &["aarch64", "x86_64"]);
        f(&mut r);
        verify_grant(&sign_grant(&r, &grant_key()).unwrap(), &pk, &mesh())
    };
    assert!(check(&|_| {}).is_ok());
    // Two arches sharing a sha256 (with different blake3) or a blake3.
    let dup_sha = check(&|r| r.artifacts[1].sha256 = r.artifacts[0].sha256.clone());
    assert!(matches!(dup_sha, Err(LicenceError::Malformed(_))));
    let dup_b3 = check(&|r| r.artifacts[1].blake3 = r.artifacts[0].blake3.clone());
    assert!(matches!(dup_b3, Err(LicenceError::Malformed(_))));
    assert!(check(&|r| r.artifacts[0].size = 0).is_err());
    assert!(check(&|r| r.artifacts[0].size = MAX_ARTIFACT_BYTES + 1).is_err());
    assert!(check(&|r| r.registry = "reg\u{7f}istry".into()).is_err());
    assert!(check(&|r| r.registry = "r\u{e9}gistry".into()).is_err());
    assert!(check(&|r| r.licence.expires = MAX_UNIX_TIME + 1).is_err());
    assert!(check(&|r| r.licence.expires = 0).is_err());
}

#[test]
fn a_tampered_floors_map_fails_the_load_and_old_key_floors_are_pruned() {
    let fx = Fx::new();
    fx.bind();
    put(&fx, &grant(1, T0, DAY, &["aarch64"])).unwrap();
    let path = fx.dir.path().join("checkout_grants.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["floors"][pk_hex(&sk(41))] = serde_json::json!({"max_issued": 1, "hw": 1});
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(matches!(open_grants(fx.dir.path()), Err(LicenceError::Malformed(_))));
    v["floors"].as_object_mut().unwrap().remove(&pk_hex(&sk(41)));
    v["floors"][pk_hex(&grant_key())]["hw"] = serde_json::json!(MAX_UNIX_TIME + 1);
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(open_grants(fx.dir.path()).is_err());
    // A key change prunes the old key's floor.
    let fx = Fx::new();
    fx.bind();
    put(&fx, &grant(1, T0, DAY, &["aarch64"])).unwrap();
    let r = binding_rec(2, BindState::Bound, &sk(40), &mesh());
    fx.store.accept_binding(&sign_binding(&r, &op()).unwrap(), posture(), &NoExtraChecks).unwrap();
    let v: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fx.dir.path().join("checkout_grants.json")).unwrap(),
    )
    .unwrap();
    assert!(v["floors"].as_object().unwrap().keys().all(|k| *k == pk_hex(&sk(40))));
}

#[test]
fn events_are_emitted_after_the_lock_is_released() {
    struct Reenter(std::sync::Mutex<Option<Arc<CheckoutGrantStore>>>);
    impl LicenceEventSink for Reenter {
        fn emit(&self, _: LicenceEvent) {
            // Would deadlock if the store still held its lock.
            if let Some(s) = self.0.lock().unwrap().as_ref() {
                let _ = s.active_binding();
            }
        }
    }
    let fx = Fx::new();
    let sink = Arc::new(Reenter(Default::default()));
    *sink.0.lock().unwrap() = Some(fx.store.clone());
    fx.store.set_sink(sink);
    let open = AdmissionPosture { open_membership: true, ..posture() };
    let _ = fx.store.accept_binding(&binding(1, BindState::Bound), open, &NoExtraChecks);
}

#[test]
fn an_unsaved_unbind_is_retried_by_tick_even_though_no_binding_is_in_effect() {
    let mut fx = Fx::new();
    fx.bind();
    put(&fx, &grant(1, T0, DAY, &["aarch64"])).unwrap();
    block_saves(&fx);
    let unbind = binding(2, BindState::Unbound);
    let r = fx.store.accept_binding(&unbind, posture(), &NoExtraChecks);
    assert!(matches!(r, Err(LicenceError::Persist(_))));
    assert!(!covered(&fx, "aarch64"), "applied in memory");
    unblock_saves(&fx);
    fx.store.tick();
    fx.restart();
    assert_eq!(fx.store.binding_status(), Err(LicenceError::Unbound), "the unbind reached disk");
}
