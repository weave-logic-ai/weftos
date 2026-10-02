//! Tests for [`super`]: dirs are injected, never `HOME`.

use super::*;
use clawft_types::project::adopt_or_init;

const NONCE_A: &str = "00112233445566778899aabbccddeeff";

struct Fixture {
    _t: tempfile::TempDir,
    env: CertEnv,
    root: PathBuf,
    id: String,
}

fn user_key() -> SigningKey {
    SigningKey::from_bytes(&[1u8; 32])
}

fn project_key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

/// A unique 32-hex nonce per call (the replay set is process-wide).
fn nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    format!("{:032x}", N.fetch_add(1, Ordering::Relaxed))
}

fn fixture() -> Fixture {
    let t = tempfile::tempdir().unwrap();
    let mdir = t.path().join("home/.weftos/projects");
    let root = t.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    let m = adopt_or_init(&root.canonicalize().unwrap(), &mdir, Some("demo")).unwrap();
    Fixture {
        env: CertEnv {
            chain: Arc::new(ChainManager::new(0, 1000)),
            user_key: user_key(),
            manifests_dir: mdir,
        },
        root: m.root.clone(),
        id: m.id,
        _t: t,
    }
}

fn request(f: &Fixture, key: &SigningKey) -> RegisterRequest {
    request_for(&f.id, &f.root, key)
}

fn request_for(id: &str, root: &Path, key: &SigningKey) -> RegisterRequest {
    let n = nonce();
    RegisterRequest {
        project_id: id.to_owned(),
        project_pubkey: key.verifying_key().to_bytes(),
        root_sha256: root_sha256(root),
        spawn: SpawnInfo { pid: 4242, exe_sha: "ab".repeat(32) },
        pop_sig: ident::pop_sign(key, &n, id).unwrap(),
        nonce: n,
    }
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-01T09:30:00Z").unwrap().with_timezone(&Utc)
}

fn rekey_params(id: &str, key: &SigningKey) -> Value {
    let n = nonce();
    json!({
        "id": id,
        "new_pubkey": ident::hex(&key.verifying_key().to_bytes()),
        "nonce": n,
        "pop_sig": ident::hex(&ident::pop_sign(key, &n, id).unwrap()),
        "reason": "rotate\nnow",
    })
}

#[test]
fn register_certifies_records_on_chain_and_writes_the_cert_file() {
    let f = fixture();
    let issued = register(&f.env, &request(&f, &project_key(2)), now()).unwrap();
    assert!(issued.new);
    ident::verify_cert_at(&issued.cert, &user_key().verifying_key().to_bytes(), now()).unwrap();
    assert_eq!(issued.cert.serial, 1);

    let events = f.env.chain.tail_from(0);
    let ev = events.iter().find(|e| e.kind == KIND_REGISTER).unwrap();
    assert_eq!(ev.source, SOURCE);
    let p = ev.payload.as_ref().unwrap();
    assert_eq!(p["name"], "demo");
    assert_eq!(p["root_sha256"], root_sha256(&f.root));
    assert_eq!(p["spawn"]["pid"], 4242);
    assert_eq!(p["manifest_schema"], 1);

    let path = cert_path(&f.env.manifests_dir, &f.id);
    let on_disk: ProjectCert = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(on_disk, issued.cert);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn chain_holds_no_private_bytes() {
    let f = fixture();
    let (pk, uk) = (project_key(2), user_key());
    register(&f.env, &request(&f, &pk), now()).unwrap();
    rekey(&f.env, &rekey_params(&f.id, &project_key(3)), now()).unwrap();
    revoke(&f.env, &json!({"id": f.id})).unwrap();
    let dump = serde_json::to_string(&f.env.chain.tail_from(0)).unwrap();
    for secret in [pk.to_bytes(), uk.to_bytes(), project_key(3).to_bytes()] {
        assert!(!dump.contains(&ident::hex(&secret)));
        assert!(!dump.contains(&format!("{:?}", secret.to_vec())));
    }
    let files = std::fs::read_dir(&f.env.manifests_dir).unwrap();
    for e in files.flatten() {
        let b = std::fs::read(e.path()).unwrap();
        assert!(!b.windows(32).any(|w| w == pk.to_bytes() || w == uk.to_bytes()));
    }
}

#[test]
fn registering_twice_is_idempotent_and_byte_identical() {
    let f = fixture();
    let a = register(&f.env, &request(&f, &project_key(2)), now()).unwrap();
    let later = now() + chrono::Duration::hours(5);
    let b = register(&f.env, &request(&f, &project_key(2)), later).unwrap();
    assert!(a.new && !b.new);
    assert_eq!(a.cert.canonical_bytes(), b.cert.canonical_bytes());
    assert_eq!(a.cert.sig, b.cert.sig);
    let n = f.env.chain.tail_from(0).iter().filter(|e| e.kind == KIND_REGISTER).count();
    assert_eq!(n, 1, "no second event");
}

#[test]
fn second_key_for_an_id_is_refused_until_rekey() {
    let f = fixture();
    register(&f.env, &request(&f, &project_key(2)), now()).unwrap();
    let err = register(&f.env, &request(&f, &project_key(3)), now()).unwrap_err();
    assert_eq!(err.kind(), "key_conflict");

    let old = register(&f.env, &request(&f, &project_key(2)), now()).unwrap().cert;
    let new = rekey(&f.env, &rekey_params(&f.id, &project_key(3)), now()).unwrap().cert;
    assert_eq!(new.serial, 2);
    assert_ne!(new.project_key_id, old.project_key_id);

    let view = view_for(&f.env, &f.id);
    assert!(matches!(view.check_cert(&old), Err(IdentityError::KeyRevoked { .. })));
    view.check_cert(&new).unwrap();
    // The old key cannot come back by registering.
    assert_eq!(
        register(&f.env, &request(&f, &project_key(2)), now()).unwrap_err().kind(),
        "key_revoked"
    );
    // The new key registers idempotently.
    assert!(!register(&f.env, &request(&f, &project_key(3)), now()).unwrap().new);
    // The rekey reason lost its control characters.
    let ev = f.env.chain.tail_from(0).into_iter().find(|e| e.kind == KIND_REKEY).unwrap();
    assert_eq!(ev.payload.unwrap()["reason"], "rotatenow");
}

#[test]
fn revocation_survives_a_chain_that_was_not_saved() {
    let f = fixture();
    let old = register(&f.env, &request(&f, &project_key(2)), now()).unwrap().cert;
    revoke(&f.env, &json!({"id": f.id, "reason": "lost"})).unwrap();
    assert!(!cert_path(&f.env.manifests_dir, &f.id).exists());
    // A fresh chain (crash before save) still sees the revocation.
    let fresh = CertEnv {
        chain: Arc::new(ChainManager::new(0, 1000)),
        user_key: user_key(),
        manifests_dir: f.env.manifests_dir.clone(),
    };
    let view = view_for(&fresh, &f.id);
    assert!(matches!(view.check_cert(&old), Err(IdentityError::KeyRevoked { .. })));
    assert_eq!(
        register(&fresh, &request(&f, &project_key(2)), now()).unwrap_err().kind(),
        "key_revoked"
    );
    // A different key is certified afresh (serial continues from the chain
    // the daemon actually has; here the fresh chain restarts at 1).
    assert!(register(&f.env, &request(&f, &project_key(3)), now()).unwrap().new);
    assert_eq!(revoke(&fresh, &json!({"id": f.id})).unwrap_err().kind(), "not_certified");
}

#[test]
fn no_cert_for_another_projects_id_or_root() {
    let f = fixture();
    let k = project_key(2);
    // An id nobody registered.
    let unknown = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
    let e = register(&f.env, &request_for(unknown, &f.root, &k), now()).unwrap_err();
    assert_eq!(e.kind(), "project_not_found");
    // Another registered project's root with this project's id.
    let other_root = f._t.path().join("other");
    std::fs::create_dir_all(&other_root).unwrap();
    let other = adopt_or_init(&other_root.canonicalize().unwrap(), &f.env.manifests_dir, None).unwrap();
    let e = register(&f.env, &request_for(&f.id, &other.root, &k), now()).unwrap_err();
    assert_eq!(e.kind(), "root_mismatch");
    // Another project's id with this project's root.
    let e = register(&f.env, &request_for(&other.id, &f.root, &k), now()).unwrap_err();
    assert_eq!(e.kind(), "root_mismatch");
    // A malformed id never reaches the filesystem.
    let e = register(&f.env, &request_for("../x", &f.root, &k), now());
    assert!(e.is_err());
    assert_eq!(f.env.chain.tail_from(0).iter().filter(|e| e.kind == KIND_REGISTER).count(), 0);
}

#[test]
fn no_cert_for_a_pubkey_the_caller_cannot_prove() {
    let f = fixture();
    let (mine, victim) = (project_key(2), project_key(7));
    // Present the victim's pubkey but sign with my key.
    let mut r = request(&f, &mine);
    r.project_pubkey = victim.verifying_key().to_bytes();
    assert_eq!(register(&f.env, &r, now()).unwrap_err().kind(), "pop_failed");
    // A PoP bound to a different project id.
    let mut r = request(&f, &mine);
    r.pop_sig = ident::pop_sign(&mine, &r.nonce, "01JB8Z3Q0V6X9KQ4M2N7T5R1WE").unwrap();
    assert_eq!(register(&f.env, &r, now()).unwrap_err().kind(), "pop_failed");
    // A garbage nonce.
    let mut r = request(&f, &mine);
    r.nonce = NONCE_A.to_uppercase();
    assert_eq!(register(&f.env, &r, now()).unwrap_err().kind(), "pop_failed");
    assert_eq!(f.env.chain.tail_from(0).len(), 0);
}

#[test]
fn a_replayed_pop_is_refused() {
    let f = fixture();
    let r = request(&f, &project_key(2));
    register(&f.env, &r, now()).unwrap();
    assert_eq!(register(&f.env, &r, now()).unwrap_err().kind(), "pop_failed");
}

#[test]
fn rekey_needs_a_certified_key_a_proof_and_a_different_key() {
    let f = fixture();
    // Nothing certified yet.
    assert_eq!(
        rekey(&f.env, &rekey_params(&f.id, &project_key(3)), now()).unwrap_err().kind(),
        "not_certified"
    );
    register(&f.env, &request(&f, &project_key(2)), now()).unwrap();
    // Same key.
    assert_eq!(
        rekey(&f.env, &rekey_params(&f.id, &project_key(2)), now()).unwrap_err().kind(),
        "key_conflict"
    );
    // Proof by the wrong key.
    let mut p = rekey_params(&f.id, &project_key(3));
    p["new_pubkey"] = json!(ident::hex(&project_key(4).verifying_key().to_bytes()));
    assert_eq!(rekey(&f.env, &p, now()).unwrap_err().kind(), "pop_failed");
    // Missing params.
    assert_eq!(rekey(&f.env, &json!({"id": f.id}), now()).unwrap_err().kind(), "invalid_params");
}

#[test]
fn show_reports_the_certificate_in_force() {
    let f = fixture();
    let v = show(&f.env, &json!({"id": f.id})).unwrap();
    assert_eq!(v["certified"], false);
    assert!(v["cert"].is_null());
    let c = register(&f.env, &request(&f, &project_key(2)), now()).unwrap().cert;
    let v = show(&f.env, &json!({"id": f.id})).unwrap();
    assert_eq!(v["certified"], true);
    assert_eq!(v["cert"]["project_key_id"], c.project_key_id.as_str());
    assert_eq!(show(&f.env, &json!({"id": "nope"})).unwrap_err().kind(), "invalid_params");
}
