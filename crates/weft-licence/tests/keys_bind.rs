mod common;
use common::*;
use std::os::unix::fs::PermissionsExt;
use weft_licence::bind;
use weft_licence::config::Config;
use weft_licence::keys;
use weft_licence::providers::*;
use weft_licence::state::OperatorKeys;
use weft_licence::{Service, SvcError};
use weft_licence_wire::{BindState, hex_encode};

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn init_makes_a_0600_key_in_a_0700_dir_and_never_overwrites() {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("state");
    let r = keys::init(&state).unwrap();
    assert_eq!(mode(&state), 0o700);
    assert_eq!(mode(&r.key_path), 0o600);
    assert!(r.fingerprint.starts_with("ed25519:") && r.fingerprint.len() == 8 + 16);
    let before = std::fs::read(&r.key_path).unwrap();
    assert_eq!(keys::init(&state).unwrap_err(), SvcError::KeyExists);
    assert_eq!(std::fs::read(&r.key_path).unwrap(), before, "the key is untouched");
    // The printed pubkey is the key's.
    assert_eq!(hex_encode(&keys::load(&state).unwrap().verifying_key().to_bytes()), r.grant_pubkey);
}

#[test]
fn a_key_or_dir_another_user_could_read_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("state");
    let r = keys::init(&state).unwrap();
    for bad in [0o640, 0o644, 0o604, 0o660] {
        std::fs::set_permissions(&r.key_path, std::fs::Permissions::from_mode(bad)).unwrap();
        assert!(matches!(keys::load(&state), Err(SvcError::KeyPerms(_))), "mode {bad:o}");
    }
    std::fs::set_permissions(&r.key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(keys::load(&state), Err(SvcError::KeyPerms(_))));
    // The service will not start on an unsafe key either.
    let cfg = Config { state_dir: state.clone(), device_id: "seed-test".into(), ..Config::default() };
    let open = Service::open(
        cfg,
        weft_licence::system_clock(),
        Box::new(StubLicence::all(None)),
        Box::new(StubFetcher::with(&[])),
        Box::new(StubDeviceSigner),
    );
    assert!(matches!(open, Err(SvcError::KeyPerms(_))));
    // And init refuses an unsafe pre-existing state dir.
    let loose = d.path().join("loose");
    std::fs::create_dir(&loose).unwrap();
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(keys::init(&loose), Err(SvcError::KeyPerms(_))));
}

#[test]
fn listen_addresses_must_be_named() {
    let mut c = Config {
        listen: vec!["0.0.0.0:8700".parse().unwrap()],
        ..Config::default()
    };
    assert!(c.validate().is_err());
    c.listen = vec!["[::]:8700".parse().unwrap()];
    assert!(c.validate().is_err());
    c.listen = vec!["169.254.42.1:8700".parse().unwrap(), "100.64.0.9:8700".parse().unwrap()];
    assert!(c.validate().is_ok());
    // The listener itself refuses too.
    let h = Harness::new(&[]);
    assert!(weft_licence::http::serve(h.svc.clone(), &["0.0.0.0:0".parse().unwrap()]).is_err());
    assert!(weft_licence::http::serve(h.svc.clone(), &[]).is_err());
}

fn setup() -> (tempfile::TempDir, std::path::PathBuf, OperatorKeys, String) {
    let d = tempfile::tempdir().unwrap();
    let state = d.path().join("state");
    let r = keys::init(&state).unwrap();
    let ops = OperatorKeys::load(&state, &[pk_hex(&operator())]).unwrap();
    (d, state, ops, r.grant_pubkey)
}

#[test]
fn a_seed_binds_to_one_mesh_only() {
    let (_d, state, ops, gpk) = setup();
    let b1 = bind::apply(&state, "seed-test", &ops, &binding(1, BindState::Bound, &gpk, &mesh()), None).unwrap();
    // The same mesh again with a higher seq (rebind to a new steward) is fine.
    let b2 = bind::apply(&state, "seed-test", &ops, &binding(2, BindState::Bound, &gpk, &mesh()), Some(&b1)).unwrap();
    // Another mesh while bound: seed_bound_elsewhere.
    let e = bind::apply(&state, "seed-test", &ops, &binding(3, BindState::Bound, &gpk, &other_mesh()), Some(&b2)).unwrap_err();
    assert_eq!(e, SvcError::BoundElsewhere);
    assert_eq!(e.to_string(), "seed_bound_elsewhere");
    // The stored binding is unchanged.
    assert_eq!(bind::load(&state, &ops).unwrap().unwrap().record.seq, 2);
}

#[test]
fn bindings_are_checked_against_the_operator_the_key_the_device_and_seq() {
    let (_d, state, ops, gpk) = setup();
    let good = binding(5, BindState::Bound, &gpk, &mesh());
    // Wrong grant key (the operator confirmed another fingerprint).
    let e = bind::apply(&state, "seed-test", &ops, &binding(5, BindState::Bound, &pk_hex(&sk(50)), &mesh()), None).unwrap_err();
    assert!(e.to_string().contains("grant key"), "{e}");
    // Signed by a non-operator.
    let mut forged = good.clone();
    forged.public_key = pk_hex(&sk(50));
    assert!(bind::apply(&state, "seed-test", &ops, &forged, None).is_err());
    // Wrong device.
    assert!(bind::apply(&state, "other-seed", &ops, &good, None).is_err());
    let b = bind::apply(&state, "seed-test", &ops, &good, None).unwrap();
    // Stale seq, equal-seq conflict, idempotent replay.
    assert!(bind::apply(&state, "seed-test", &ops, &binding(4, BindState::Bound, &gpk, &mesh()), Some(&b)).is_err());
    assert!(bind::apply(&state, "seed-test", &ops, &good, Some(&b)).is_ok());
    let conflict = {
        let mut r = serde_json::from_str::<weft_licence_wire::BindingRecord>(&good.payload).unwrap();
        r.steward_node_id = "node-other".into();
        weft_licence_wire::sign_binding(&r, &operator()).unwrap()
    };
    assert!(bind::apply(&state, "seed-test", &ops, &conflict, Some(&b)).unwrap_err().to_string().contains("conflict"));
}

#[test]
fn unbind_deletes_the_grant_key_and_a_fresh_init_can_bind_another_mesh() {
    let (_d, state, ops, gpk) = setup();
    let b = bind::apply(&state, "seed-test", &ops, &binding(1, BindState::Bound, &gpk, &mesh()), None).unwrap();
    let u = bind::apply(&state, "seed-test", &ops, &binding(2, BindState::Unbound, &gpk, &mesh()), Some(&b)).unwrap();
    assert!(!keys::key_path(&state).exists(), "the grant key is gone");
    assert!(!u.is_bound());
    // The old key cannot rebind the other mesh: there is no key.
    assert!(bind::apply(&state, "seed-test", &ops, &binding(3, BindState::Bound, &gpk, &other_mesh()), Some(&u)).is_err());
    // `init` runs again over USB and the new key binds the new mesh.
    let r2 = keys::init(&state).unwrap();
    assert_ne!(r2.grant_pubkey, gpk);
    bind::apply(&state, "seed-test", &ops, &binding(3, BindState::Bound, &r2.grant_pubkey, &other_mesh()), Some(&u)).unwrap();
}

#[test]
fn an_unbind_applied_by_the_cli_takes_effect_without_a_restart() {
    let h = Harness::new(&[("fall-detect", "arm", b"\x7fELF x")]);
    assert_eq!(status(&h.checkout("fall-detect", "arm")), 200);
    assert!(h.unsigned("GET", "/licence/v1/identity").json_body().unwrap()["grant_key_id"].is_string());
    let ops = OperatorKeys::load(&h.cfg.state_dir, &h.cfg.operator_pubkeys).unwrap();
    let cur = bind::load(&h.cfg.state_dir, &ops).unwrap();
    let gpk = hex_encode(&h.grant_key());
    // The CLI (another process) unbinds; the running service is not reopened.
    bind::apply(&h.cfg.state_dir, "seed-test", &ops, &binding(2, BindState::Unbound, &gpk, &mesh()), cur.as_ref()).unwrap();
    h.advance(61);
    let r = h.call("POST", "/licence/v1/renew", b"");
    assert_eq!((status(&r), code(&r).as_str()), (409, "seed_not_bound"));
    let id = h.unsigned("GET", "/licence/v1/identity").json_body().unwrap();
    assert_eq!(id["bound"], false);
    assert!(id["grant_key_id"].is_null(), "the key left memory");
    // Every checkout was released on disk.
    let slots: serde_json::Value = serde_json::from_slice(&std::fs::read(h.cfg.state_dir.join("slots.json")).unwrap()).unwrap();
    assert_eq!(slots["slots"]["fall-detect@1.0.0"]["status"], "released");
}

#[test]
fn a_rebind_to_another_mesh_carries_nothing_over() {
    let h = Harness::new(&[("fall-detect", "arm", b"\x7fELF x")]);
    let g1 = grant_of(&h.checkout("fall-detect", "arm"));
    let c1: weft_licence_wire::CheckoutGrant = serde_json::from_str(&g1.payload).unwrap();
    let ops = OperatorKeys::load(&h.cfg.state_dir, &h.cfg.operator_pubkeys).unwrap();
    let cur = bind::load(&h.cfg.state_dir, &ops).unwrap();
    let gpk = hex_encode(&h.grant_key());
    let u = bind::apply(&h.cfg.state_dir, "seed-test", &ops, &binding(2, BindState::Unbound, &gpk, &mesh()), cur.as_ref()).unwrap();
    let r2 = keys::init(&h.cfg.state_dir).unwrap();
    bind::apply(&h.cfg.state_dir, "seed-test", &ops, &binding(3, BindState::Bound, &r2.grant_pubkey, &other_mesh()), Some(&u)).unwrap();
    h.advance(61);
    // The old mesh's grants are not listed, renewed or served for the new mesh.
    let list = h.call("GET", "/licence/v1/grants?since=0", b"").json_body().unwrap();
    assert!(list["grants"].as_array().unwrap().is_empty());
    let renew = h.call("POST", "/licence/v1/renew", b"").json_body().unwrap();
    assert!(renew["grants"].as_array().unwrap().is_empty());
    let b3 = hex_encode(blake3::hash(b"\x7fELF x").as_bytes());
    assert_eq!(code(&h.call("GET", &format!("/licence/v1/artifact/{b3}"), b"")), "no_grant");
    // A fresh checkout is a new grant for the new mesh and key; seq keeps rising.
    let g2 = grant_of(&h.checkout("fall-detect", "arm"));
    let c2: weft_licence_wire::CheckoutGrant = serde_json::from_str(&g2.payload).unwrap();
    assert_eq!(c2.mesh_id, other_mesh().to_hex());
    assert!(c2.seq > c1.seq);
    assert_eq!(c2.arches().len(), 1);
    weft_licence_wire::verify_grant(&g2, &h.grant_key(), &other_mesh()).unwrap();
}

#[test]
fn a_service_restarted_after_an_unbind_releases_every_checkout() {
    let h = Harness::new(&[("fall-detect", "arm", b"\x7fELF x")]);
    h.checkout("fall-detect", "arm");
    let ops = OperatorKeys::load(&h.cfg.state_dir, &h.cfg.operator_pubkeys).unwrap();
    let cur = bind::load(&h.cfg.state_dir, &ops).unwrap();
    let gpk = hex_encode(&h.grant_key());
    bind::apply(&h.cfg.state_dir, "seed-test", &ops, &binding(2, BindState::Unbound, &gpk, &mesh()), cur.as_ref()).unwrap();
    let h = h.reopen();
    let slots: serde_json::Value = serde_json::from_slice(&std::fs::read(h.cfg.state_dir.join("slots.json")).unwrap()).unwrap();
    assert_eq!(slots["slots"]["fall-detect@1.0.0"]["status"], "released");
}

#[test]
fn a_declared_licence_must_be_operator_signed_cover_the_cog_and_the_mesh() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("licence.json");
    let ops = OperatorKeys::load(d.path(), &[pk_hex(&operator())]).unwrap();
    let lic = LocalDeclaredLicence::new(path.clone(), Box::new(move |pk| ops.contains(pk)));
    let m = mesh().to_hex();
    assert_eq!(lic.entitlement("fall-detect", &m, T0), Err(LicenceCheckError::Unlicensed), "no file");
    let rec = |cogs: &[&str], expires: Option<u64>, mesh: &str| DeclaredLicence {
        v: 1, mesh_id: mesh.into(), source: "cognitum".into(), account_ref: "acct-123".into(),
        cogs: cogs.iter().map(|s| s.to_string()).collect(), expires, issued_at: T0,
    };
    let write = |r: &DeclaredLicence, key: &ed25519_dalek::SigningKey| {
        std::fs::write(&path, serde_json::to_vec(&sign_licence(r, key)).unwrap()).unwrap()
    };
    write(&rec(&["fall-detect"], Some(T0 + 100), &m), &operator());
    let e = lic.entitlement("fall-detect", &m, T0).unwrap();
    assert_eq!(e.ref_sha256, weft_licence_wire::sha256_hex(b"acct-123"));
    assert_eq!(e.expires, Some(T0 + 100));
    assert_eq!(lic.entitlement("other", &m, T0), Err(LicenceCheckError::Unlicensed));
    assert_eq!(lic.entitlement("fall-detect", &other_mesh().to_hex(), T0), Err(LicenceCheckError::Unlicensed));
    assert_eq!(lic.entitlement("fall-detect", &m, T0 + 100), Err(LicenceCheckError::Expired));
    write(&rec(&["*"], None, &m), &operator());
    assert!(lic.entitlement("anything", &m, T0).is_ok());
    // Signed by a key that is not a pinned operator key: unreadable, so no checkout.
    write(&rec(&["*"], None, &m), &sk(42));
    assert!(matches!(lic.entitlement("anything", &m, T0), Err(LicenceCheckError::Unreadable(_))));
    // Tampered after signing.
    write(&rec(&["*"], None, &m), &operator());
    let tampered = std::fs::read_to_string(&path).unwrap().replace("acct-123", "acct-999");
    std::fs::write(&path, tampered).unwrap();
    assert!(matches!(lic.entitlement("anything", &m, T0), Err(LicenceCheckError::Unreadable(_))));
}

#[test]
fn the_example_config_and_unit_in_dist_are_consistent() {
    let cfg: Config = toml::from_str(include_str!("../dist/config.example.toml")).unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.limits, weft_licence::Limits::default(), "the example shows the defaults");
    assert_eq!(cfg.grant_ttl_secs, 72 * 3600);
    let unit = include_str!("../dist/weft-licence.service");
    assert!(unit.contains("User=weft-licence") && unit.contains("StateDirectoryMode=0700"));
    assert!(!unit.contains("0.0.0.0"));
}

#[test]
fn listen_must_be_a_link_local_tailnet_or_loopback_address_unless_opted_in() {
    let mut c = Config::default();
    for ok in ["127.0.0.1:1", "169.254.42.1:1", "100.64.0.9:1", "100.127.255.1:1", "[::1]:1", "[fe80::1]:1", "[fd7a:115c:a1e0::5]:1"] {
        c.listen = vec![ok.parse().unwrap()];
        assert!(c.validate().is_ok(), "{ok}");
    }
    for bad in ["192.168.1.5:1", "10.0.0.2:1", "100.128.0.1:1", "8.8.8.8:1", "[fd00::1]:1", "[2001:db8::1]:1"] {
        c.listen = vec![bad.parse().unwrap()];
        assert!(c.validate().is_err(), "{bad}");
        c.allow_lan_listen = true;
        assert!(c.validate().is_ok(), "{bad} with the opt-in");
        c.allow_lan_listen = false;
    }
}

#[test]
fn device_id_and_limits_are_validated() {
    let mut c = Config {
        device_id: "bad id with spaces".into(),
        ..Config::default()
    };
    assert!(c.validate().is_err());
    c.device_id = "seed-1".into();
    assert!(c.validate().is_ok());
    c.limits.rate_bytes_per_sec = 0;
    assert!(c.validate().is_err());
    c.limits.rate_bytes_per_sec = 1;
    c.limits.cache_bytes = c.limits.max_artifact_bytes - 1;
    assert!(c.validate().is_err());
}

#[test]
fn a_symlinked_state_dir_or_key_is_refused_and_foreign_ownership_is_detected() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    keys::init(&real).unwrap();
    let link = d.path().join("link");
    symlink(&real, &link).unwrap();
    assert!(matches!(keys::load(&link), Err(SvcError::KeyPerms(_))));
    let other = d.path().join("other");
    keys::init(&other).unwrap();
    std::fs::remove_file(other.join("grant.key")).unwrap();
    symlink(real.join("grant.key"), other.join("grant.key")).unwrap();
    assert!(matches!(keys::load(&other), Err(SvcError::KeyPerms(_))));
    // The CLI refuses a state dir it does not own (the root directory here).
    if weft_licence::fsio::euid() != 0 {
        assert!(weft_licence::fsio::require_owner(std::path::Path::new("/")).is_err());
    }
    assert!(weft_licence::fsio::require_owner(&real).is_ok());
}

#[test]
fn an_unbind_during_the_fetch_signs_nothing() {
    let h = Harness::new(&[("fall-detect", "arm", b"\x7fELF slow")]);
    let released = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let r2 = released.clone();
    h.svc.set_release_hook(Box::new(move |_| {
        r2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let (tx, rx) = std::sync::mpsc::channel();
    *h.fetcher.gate.lock().unwrap() = Some(rx);
    let svc = h.svc.clone();
    let req = h.signed("POST", "/licence/v1/checkout", br#"{"request_id":"a","cog_id":"fall-detect","version":"latest","arch":"arm"}"#);
    let t = std::thread::spawn(move || {
        let r = svc.handle(&req);
        (r.status, r.json_body())
    });
    while h.fetcher.fetch_count() == 0 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // The operator unbinds while the checkout is downloading.
    let ops = OperatorKeys::load(&h.cfg.state_dir, &h.cfg.operator_pubkeys).unwrap();
    let cur = bind::load(&h.cfg.state_dir, &ops).unwrap();
    let gpk = hex_encode(&h.grant_key());
    bind::apply(&h.cfg.state_dir, "seed-test", &ops, &binding(2, BindState::Unbound, &gpk, &mesh()), cur.as_ref()).unwrap();
    tx.send(()).unwrap();
    let (st, body) = t.join().unwrap();
    assert_eq!(st, 409, "{body:?}");
    let code = body.unwrap()["error"].as_str().unwrap().to_string();
    assert!(code == "binding_changed" || code == "seed_not_bound", "{code}");
    assert_eq!(released.load(std::sync::atomic::Ordering::SeqCst), 0, "no grant was released");
    let slots = std::fs::read(h.cfg.state_dir.join("slots.json")).ok();
    assert!(slots.is_none_or(|b| !String::from_utf8_lossy(&b).contains("\"seq\":1")), "no seq was consumed");
}

#[test]
fn a_binding_replaced_by_rename_with_the_same_size_and_mtime_is_still_seen() {
    let h = Harness::new(&[]);
    assert_eq!(h.unsigned("GET", "/licence/v1/identity").json_body().unwrap()["bound"], true);
    let path = h.cfg.state_dir.join("binding.json");
    let old_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    // An unbind record of exactly the same encoded length: "unbound" is two
    // characters longer than "bound", so the device id is two shorter.
    let gpk = hex_encode(&h.grant_key());
    let mut rec: weft_licence_wire::BindingRecord =
        serde_json::from_str(&binding(1, BindState::Bound, &gpk, &mesh()).payload).unwrap();
    rec.state = BindState::Unbound;
    rec.device_id = "seed-te".into();
    rec.seq = 1;
    let env = weft_licence_wire::sign_binding(&rec, &operator()).unwrap();
    let bytes = serde_json::to_vec(&env).unwrap();
    assert_eq!(bytes.len() as u64, std::fs::metadata(&path).unwrap().len(), "same length");
    let tmp = h.cfg.state_dir.join("binding.tmp");
    std::fs::write(&tmp, &bytes).unwrap();
    std::fs::File::options().write(true).open(&tmp).unwrap().set_modified(old_mtime).unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), old_mtime);
    assert_eq!(h.unsigned("GET", "/licence/v1/identity").json_body().unwrap()["bound"], false);
}

#[test]
fn require_owner_refuses_a_symlinked_state_dir() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = d.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(weft_licence::fsio::require_owner(&real).is_ok());
    assert!(weft_licence::fsio::require_owner(&link).unwrap_err().contains("symlink"));
}
