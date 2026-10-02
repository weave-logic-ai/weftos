//! Tests for [`super`]: dirs are injected, never `HOME`.

use super::*;
use clawft_types::project::adopt_or_init;

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

fn user_kid() -> String {
    key_id(&user_key().verifying_key().to_bytes())
}

fn env_over(chain: Arc<ChainManager>, mdir: &Path) -> CertEnv {
    CertEnv { chain, user_key: user_key(), manifests_dir: mdir.to_path_buf() }
}

fn fixture() -> Fixture {
    let t = tempfile::tempdir().unwrap();
    let mdir = t.path().join("home/.weftos/projects");
    let root = t.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    let m = adopt_or_init(&root.canonicalize().unwrap(), &mdir, Some("demo")).unwrap();
    Fixture {
        env: env_over(Arc::new(ChainManager::new(0, 1000)), &mdir),
        root: m.root.clone(),
        id: m.id,
        _t: t,
    }
}

/// The same manifests, a chain that lost everything (crash before save).
fn restarted(f: &Fixture) -> CertEnv {
    env_over(Arc::new(ChainManager::new(0, 1000)), &f.env.manifests_dir)
}

fn request(f: &Fixture, key: &SigningKey) -> RegisterRequest {
    request_for(&f.id, &f.root, key)
}

fn request_for(id: &str, root: &Path, key: &SigningKey) -> RegisterRequest {
    let n = issue_challenge(id).unwrap();
    RegisterRequest {
        project_id: id.to_owned(),
        project_pubkey: key.verifying_key().to_bytes(),
        root_sha256: root_sha256(root),
        spawn: SpawnInfo { pid: 4242, exe_sha: "ab".repeat(32) },
        pop_sig: ident::pop_sign(key, PopOp::Register, &user_kid(), &n, id).unwrap(),
        nonce: claim_nonce(&n, id).unwrap(),
    }
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-01T09:30:00Z").unwrap().with_timezone(&Utc)
}

fn rekey_params(id: &str, key: &SigningKey) -> Value {
    let n = issue_challenge(id).unwrap();
    json!({
        "id": id,
        "new_pubkey": ident::hex(&key.verifying_key().to_bytes()),
        "nonce": n,
        "pop_sig": ident::hex(&ident::pop_sign(key, PopOp::Rekey, &user_kid(), &n, id).unwrap()),
        "reason": "rotate\nnow",
    })
}

fn count(f: &Fixture, kind: &str) -> usize {
    f.env.chain.tail_from(0).iter().filter(|e| e.kind == kind).count()
}

#[test]
fn register_certifies_journals_records_on_chain_and_writes_the_cert_file() {
    let f = fixture();
    let issued = register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    assert!(issued.new);
    ident::verify_cert_at(&issued.cert, &user_key().verifying_key().to_bytes(), now()).unwrap();
    assert_eq!(issued.cert.serial, 1);

    let ev = f.env.chain.tail_from(0).into_iter().find(|e| e.kind == KIND_REGISTER).unwrap();
    assert_eq!(ev.source, SOURCE);
    let p = ev.payload.unwrap();
    assert_eq!(p["name"], "demo");
    assert_eq!(p["root_sha256"], root_sha256(&f.root));
    assert_eq!(p["spawn"]["claimed_pid"], 4242);
    assert_eq!(p["manifest_schema"], 1);

    let recs = IdentityJournal::new(&f.env.manifests_dir).read(true).unwrap();
    assert_eq!(recs, vec![JournalRecord::Register { cert: issued.cert.clone() }]);

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
fn nothing_private_reaches_the_chain_journal_or_cert_files() {
    let f = fixture();
    let (pk, uk, pk3) = (project_key(2), user_key(), project_key(3));
    register(&f.env, request(&f, &pk), now()).unwrap();
    rekey(&f.env, &rekey_params(&f.id, &pk3), now()).unwrap();
    revoke(&f.env, &json!({"id": f.id})).unwrap();
    let dump = serde_json::to_string(&f.env.chain.tail_from(0)).unwrap();
    for secret in [pk.to_bytes(), uk.to_bytes(), pk3.to_bytes()] {
        assert!(!dump.contains(&ident::hex(&secret)));
        assert!(!dump.contains(&format!("{:?}", secret.to_vec())));
    }
    for e in std::fs::read_dir(&f.env.manifests_dir).unwrap().flatten() {
        let b = std::fs::read(e.path()).unwrap();
        for secret in [pk.to_bytes(), uk.to_bytes(), pk3.to_bytes()] {
            assert!(!b.windows(32).any(|w| w == secret), "{:?}", e.path());
            assert!(!String::from_utf8_lossy(&b).contains(&ident::hex(&secret)), "{:?}", e.path());
        }
    }
}

#[test]
fn registering_twice_is_idempotent_and_byte_identical() {
    let f = fixture();
    let a = register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    let b = register(&f.env, request(&f, &project_key(2)), now() + chrono::Duration::hours(5)).unwrap();
    assert!(a.new && !b.new);
    assert_eq!(a.cert.canonical_bytes(), b.cert.canonical_bytes());
    assert_eq!(a.cert.sig, b.cert.sig);
    assert_eq!(count(&f, KIND_REGISTER), 1, "no second event");
    assert_eq!(IdentityJournal::new(&f.env.manifests_dir).read(true).unwrap().len(), 1);
}

#[test]
fn second_key_for_an_id_is_refused_until_rekey() {
    let f = fixture();
    register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    assert_eq!(register(&f.env, request(&f, &project_key(3)), now()).unwrap_err().kind(), "key_conflict");

    let old = register(&f.env, request(&f, &project_key(2)), now()).unwrap().cert;
    let new = rekey(&f.env, &rekey_params(&f.id, &project_key(3)), now()).unwrap().cert;
    assert_eq!(new.serial, 2);
    assert_ne!(new.project_key_id, old.project_key_id);

    let view = current_view(&f.env).unwrap();
    assert!(matches!(view.check_cert(&old), Err(IdentityError::KeyRevoked { .. })));
    view.check_cert(&new).unwrap();
    assert_eq!(register(&f.env, request(&f, &project_key(2)), now()).unwrap_err().kind(), "key_revoked");
    assert!(!register(&f.env, request(&f, &project_key(3)), now()).unwrap().new);
    let ev = f.env.chain.tail_from(0).into_iter().find(|e| e.kind == KIND_REKEY).unwrap();
    assert_eq!(ev.payload.unwrap()["reason"], "rotatenow");
}

#[test]
fn a_crash_does_not_reset_tofu() {
    let f = fixture();
    register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    // Restart with an empty chain: K2 must still be refused, K1 idempotent.
    let r = restarted(&f);
    assert_eq!(register(&r, request(&f, &project_key(3)), now()).unwrap_err().kind(), "key_conflict");
    assert!(!register(&r, request(&f, &project_key(2)), now()).unwrap().new);
    // A deleted journal is not "no history": everything refuses.
    std::fs::remove_file(IdentityJournal::new(&f.env.manifests_dir).path()).unwrap();
    assert_eq!(register(&r, request(&f, &project_key(3)), now()).unwrap_err().kind(), "journal_corrupt");
}

#[test]
fn revocation_and_rekey_survive_a_chain_that_was_not_saved() {
    let f = fixture();
    let old = register(&f.env, request(&f, &project_key(2)), now()).unwrap().cert;
    revoke(&f.env, &json!({"id": f.id, "reason": "lost"})).unwrap();
    assert!(!cert_path(&f.env.manifests_dir, &f.id).exists());
    let r = restarted(&f);
    // A verifier sees a revocation only the journal remembers.
    let view = current_view(&r).unwrap();
    assert!(matches!(view.check_cert(&old), Err(IdentityError::KeyRevoked { .. })));
    assert_eq!(register(&r, request(&f, &project_key(2)), now()).unwrap_err().kind(), "key_revoked");
    // A new key is certified with the next serial, not serial 1 again.
    let c = register(&r, request(&f, &project_key(3)), now()).unwrap();
    assert_eq!(c.cert.serial, 2);
    assert_eq!(revoke(&restarted(&f), &json!({"id": "nope"})).unwrap_err().kind(), "invalid_params");
}

#[test]
fn a_corrupt_journal_stops_everything_and_is_left_alone() {
    let f = fixture();
    register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    let jpath = IdentityJournal::new(&f.env.manifests_dir).path();
    for bad in ["", "garbage\n", "{\"op\":\"revoke\""] {
        std::fs::write(&jpath, bad).unwrap();
        assert_eq!(register(&f.env, request(&f, &project_key(2)), now()).unwrap_err().kind(), "journal_corrupt");
        assert_eq!(register(&f.env, request(&f, &project_key(3)), now()).unwrap_err().kind(), "journal_corrupt");
        assert_eq!(rekey(&f.env, &rekey_params(&f.id, &project_key(3)), now()).unwrap_err().kind(), "journal_corrupt");
        assert_eq!(revoke(&f.env, &json!({"id": f.id})).unwrap_err().kind(), "journal_corrupt");
        assert_eq!(show(&f.env, &json!({"id": f.id})).unwrap_err().kind(), "journal_corrupt");
        assert_eq!(current_view(&f.env).unwrap_err().kind(), "journal_corrupt");
        assert_eq!(std::fs::read_to_string(&jpath).unwrap(), bad, "journal was modified");
    }
    assert_eq!(count(&f, KIND_REVOKE) + count(&f, KIND_REKEY), 0);
}

#[test]
fn a_deleted_journal_cannot_launder_a_revocation() {
    let f = fixture();
    register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    revoke(&f.env, &json!({"id": f.id})).unwrap();
    // Chain unsaved (restart), journal deleted: the cert file is gone too,
    // but the lock file remembers this install used the journal.
    std::fs::remove_file(IdentityJournal::new(&f.env.manifests_dir).path()).unwrap();
    let r = restarted(&f);
    let e = register(&r, request(&f, &project_key(2)), now()).unwrap_err();
    assert_eq!(e.kind(), "journal_corrupt");
    assert!(e.to_string().contains("project.identity.repair"), "{e}");
    // With the lock file gone as well nothing at all remembers, which is
    // indistinguishable from a fresh install; that limit is documented.
}

#[test]
fn repair_rebuilds_from_cert_files_and_chain_and_refuses_a_healthy_journal() {
    let f = fixture();
    assert_eq!(repair(&f.env).unwrap_err().kind(), "invalid_params", "fresh/healthy");
    register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    assert_eq!(repair(&f.env).unwrap_err().kind(), "invalid_params", "healthy");
    let jpath = IdentityJournal::new(&f.env.manifests_dir).path();
    std::fs::write(&jpath, "garbage\n").unwrap();
    assert_eq!(show(&f.env, &json!({"id": f.id})).unwrap_err().kind(), "journal_corrupt");
    let r = repair(&restarted(&f)).unwrap();
    assert_eq!(r["records"], 1);
    assert!(std::fs::read_to_string(r["moved_aside"].as_str().unwrap()).unwrap().contains("garbage"));
    // Back in service, and TOFU still holds from the rebuilt journal.
    let r = restarted(&f);
    assert_eq!(register(&r, request(&f, &project_key(3)), now()).unwrap_err().kind(), "key_conflict");
    assert!(!register(&r, request(&f, &project_key(2)), now()).unwrap().new);
}

#[test]
fn a_key_cannot_certify_two_projects() {
    let f = fixture();
    let other_root = f._t.path().join("other");
    std::fs::create_dir_all(&other_root).unwrap();
    let other = adopt_or_init(&other_root.canonicalize().unwrap(), &f.env.manifests_dir, None).unwrap();
    register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    let e = register(&f.env, request_for(&other.id, &other.root, &project_key(2)), now()).unwrap_err();
    assert_eq!(e.kind(), "key_reuse");
    // Not even after the first project revoked it.
    revoke(&f.env, &json!({"id": f.id})).unwrap();
    let e = register(&f.env, request_for(&other.id, &other.root, &project_key(2)), now()).unwrap_err();
    assert_eq!(e.kind(), "key_reuse");
    // Rekeying onto another project's key is refused too.
    register(&f.env, request_for(&other.id, &other.root, &project_key(5)), now()).unwrap();
    register(&f.env, request(&f, &project_key(6)), now()).unwrap();
    let e = rekey(&f.env, &rekey_params(&f.id, &project_key(5)), now()).unwrap_err();
    assert_eq!(e.kind(), "key_reuse");
}

#[test]
fn concurrent_registrations_of_different_keys_have_one_winner() {
    let f = fixture();
    let (chain, dir) = (Arc::clone(&f.env.chain), f.env.manifests_dir.clone());
    let reqs: Vec<_> = (2u8..10).map(|n| request(&f, &project_key(n))).collect();
    let handles: Vec<_> = reqs
        .into_iter()
        .map(|r| {
            let env = env_over(Arc::clone(&chain), &dir);
            std::thread::spawn(move || register(&env, r, now()))
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let wins = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(wins, 1, "{:?}", results.iter().map(|r| r.as_ref().map(|_| ()).map_err(|e| e.kind())).collect::<Vec<_>>());
    assert!(results.iter().filter_map(|r| r.as_ref().err()).all(|e| e.kind() == "key_conflict"));
    assert_eq!(count(&f, KIND_REGISTER), 1);
    assert_eq!(IdentityJournal::new(&dir).read(true).unwrap().len(), 1);
}

#[test]
fn no_cert_for_another_projects_id_or_root() {
    let f = fixture();
    let k = project_key(2);
    let unknown = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
    assert_eq!(register(&f.env, request_for(unknown, &f.root, &k), now()).unwrap_err().kind(), "project_not_found");
    let other_root = f._t.path().join("other");
    std::fs::create_dir_all(&other_root).unwrap();
    let other = adopt_or_init(&other_root.canonicalize().unwrap(), &f.env.manifests_dir, None).unwrap();
    assert_eq!(register(&f.env, request_for(&f.id, &other.root, &k), now()).unwrap_err().kind(), "root_mismatch");
    assert_eq!(register(&f.env, request_for(&other.id, &f.root, &k), now()).unwrap_err().kind(), "root_mismatch");
    assert!(issue_challenge("../x").is_err());
    assert_eq!(count(&f, KIND_REGISTER), 0);
}

#[test]
fn no_cert_for_a_pubkey_the_caller_cannot_prove() {
    let f = fixture();
    let (mine, victim) = (project_key(2), project_key(7));
    let mut r = request(&f, &mine);
    r.project_pubkey = victim.verifying_key().to_bytes();
    assert_eq!(register(&f.env, r, now()).unwrap_err().kind(), "pop_failed");
    // A PoP made for another operation, or for another user chain.
    let mut r = request(&f, &mine);
    r.pop_sig = ident::pop_sign(&mine, PopOp::Rekey, &user_kid(), r.nonce.as_str(), &f.id).unwrap();
    assert_eq!(register(&f.env, r, now()).unwrap_err().kind(), "pop_failed");
    let mut r = request(&f, &mine);
    r.pop_sig = ident::pop_sign(&mine, PopOp::Register, &"0".repeat(32), r.nonce.as_str(), &f.id).unwrap();
    assert_eq!(register(&f.env, r, now()).unwrap_err().kind(), "pop_failed");
    assert_eq!(count(&f, KIND_REGISTER), 0);
    assert!(IdentityJournal::new(&f.env.manifests_dir).read(false).unwrap().is_empty());
}

#[test]
fn only_daemon_issued_single_use_nonces_work() {
    let f = fixture();
    // An invented nonce cannot become a DaemonNonce.
    assert_eq!(claim_nonce(&"ab".repeat(16), &f.id).unwrap_err().kind(), "pop_failed");
    // Single use.
    let n = issue_challenge(&f.id).unwrap();
    claim_nonce(&n, &f.id).unwrap();
    assert_eq!(claim_nonce(&n, &f.id).unwrap_err().kind(), "pop_failed");
    // Bound to its project: another id cannot claim it, and the failed
    // claim does not burn it for its real holder.
    let n = issue_challenge(&f.id).unwrap();
    assert_eq!(claim_nonce(&n, "01JB8Z3Q0V6X9KQ4M2N7T5R1WE").unwrap_err().kind(), "pop_failed");
    claim_nonce(&n, &f.id).unwrap();
    // A rekey nonce is single use too.
    let p = rekey_params(&f.id, &project_key(3));
    register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    rekey(&f.env, &p, now()).unwrap();
    assert_eq!(rekey(&f.env, &p, now()).unwrap_err().kind(), "pop_failed");
}

#[test]
fn rekey_needs_a_certified_key_a_proof_and_a_different_key() {
    let f = fixture();
    assert_eq!(rekey(&f.env, &rekey_params(&f.id, &project_key(3)), now()).unwrap_err().kind(), "not_certified");
    register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    assert_eq!(rekey(&f.env, &rekey_params(&f.id, &project_key(2)), now()).unwrap_err().kind(), "key_conflict");
    let mut p = rekey_params(&f.id, &project_key(3));
    p["new_pubkey"] = json!(ident::hex(&project_key(4).verifying_key().to_bytes()));
    assert_eq!(rekey(&f.env, &p, now()).unwrap_err().kind(), "pop_failed");
    assert_eq!(rekey(&f.env, &json!({"id": f.id}), now()).unwrap_err().kind(), "invalid_params");
}

#[test]
fn show_and_challenge() {
    let f = fixture();
    let v = show(&f.env, &json!({"id": f.id})).unwrap();
    assert_eq!(v["certified"], false);
    assert!(v["cert"].is_null());
    let c = register(&f.env, request(&f, &project_key(2)), now()).unwrap().cert;
    let v = show(&f.env, &json!({"id": f.id})).unwrap();
    assert_eq!(v["certified"], true);
    assert_eq!(v["cert"]["project_key_id"], c.project_key_id.as_str());
    assert_eq!(show(&f.env, &json!({"id": "nope"})).unwrap_err().kind(), "invalid_params");

    let ch = challenge(&f.env, &json!({"id": f.id})).unwrap();
    assert_eq!(ch["user_key_id"], user_kid().as_str());
    assert_eq!(ch["op"], "rekey");
    claim_nonce(ch["nonce"].as_str().unwrap(), &f.id).unwrap();
    let unreg = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
    assert_eq!(challenge(&f.env, &json!({"id": unreg})).unwrap_err().kind(), "project_not_found");
}

#[test]
fn revoke_reports_a_cert_file_it_could_not_remove() {
    let f = fixture();
    register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    let path = cert_path(&f.env.manifests_dir, &f.id);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let e = revoke(&f.env, &json!({"id": f.id})).unwrap_err();
    assert_eq!(e.kind(), "project_store_error");
    assert!(e.to_string().contains("is revoked"), "{e}");
    // The revocation itself took effect.
    assert!(current_view(&f.env).unwrap().bound_key_id(&f.id).is_none());
}

#[cfg(unix)]
#[test]
fn distinct_non_utf8_roots_hash_differently() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let a = root_sha256(Path::new(OsStr::from_bytes(b"/tmp/\xff")));
    let b = root_sha256(Path::new(OsStr::from_bytes(b"/tmp/\xfe")));
    assert_ne!(a, b);
    assert_eq!(a, ident::hex(&Sha256::digest(b"/tmp/\xff")));
}

#[test]
fn repair_keeps_journal_only_revocations_before_a_torn_tail() {
    use std::io::Write as _;
    let f = fixture();
    register(&f.env, request(&f, &project_key(2)), now()).unwrap();
    revoke(&f.env, &json!({"id": f.id})).unwrap();
    let jpath = IdentityJournal::new(&f.env.manifests_dir).path();
    std::fs::OpenOptions::new().append(true).open(&jpath).unwrap().write_all(b"{\"op\":\"rev").unwrap();
    let r = restarted(&f);
    assert_eq!(current_view(&r).unwrap_err().kind(), "journal_corrupt");
    repair(&r).unwrap();
    assert_eq!(register(&r, request(&f, &project_key(2)), now()).unwrap_err().kind(), "key_revoked");
}

#[test]
fn an_unreadable_manifests_dir_fails_closed() {
    let f = fixture();
    let missing = f._t.path().join("no/such/dir");
    assert!(store_read_certs(&missing).unwrap().is_empty());
    let file = f._t.path().join("a-file");
    std::fs::write(&file, b"x").unwrap();
    assert!(store_read_certs(&file).is_err());
}
