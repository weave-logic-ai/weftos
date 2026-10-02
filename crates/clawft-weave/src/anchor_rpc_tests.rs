//! Tests for [`super`]: dirs are injected, never `HOME`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use clawft_kernel::chain::ChainManager;
use clawft_kernel::chain_anchor::{AnchorSubmitError, ParentAnchor, ParentAnchorConfig, ParentTransport};
use clawft_kernel::project_identity::{self as ident, is_reserved_source};
use clawft_types::project::adopt_or_init;
use clawft_types::project::cert::{PopOp, ts};
use ed25519_dalek::SigningKey;

use super::record::anchor_file;
use super::*;
use crate::project_cert_rpc::{
    RegisterRequest, SpawnInfo, claim_nonce, issue_challenge, register, rekey, revoke, root_sha256,
};

fn user_key() -> SigningKey {
    SigningKey::from_bytes(&[1u8; 32])
}

fn project_key() -> SigningKey {
    SigningKey::from_bytes(&[2u8; 32])
}

fn t0() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-01T09:30:00Z").unwrap().with_timezone(&Utc)
}

fn later(secs: i64) -> DateTime<Utc> {
    t0() + Duration::seconds(secs)
}

struct Fx {
    _t: tempfile::TempDir,
    env: CertEnv,
    id: String,
}

fn fixture() -> Fx {
    let t = tempfile::tempdir().unwrap();
    let mdir = t.path().join("home/.weftos/projects");
    let root = t.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    let m = adopt_or_init(&root.canonicalize().unwrap(), &mdir, Some("demo")).unwrap();
    let env = CertEnv {
        chain: Arc::new(ChainManager::new(0, 100_000)),
        user_key: user_key(),
        manifests_dir: mdir,
    };
    certify(&env, &m.id, &m.root, &project_key());
    Fx { _t: t, env, id: m.id }
}

fn certify(env: &CertEnv, id: &str, root: &Path, key: &SigningKey) {
    let n = issue_challenge(id).unwrap();
    let uk = clawft_types::project::cert::key_id(&user_key().verifying_key().to_bytes());
    register(
        env,
        RegisterRequest {
            project_id: id.to_owned(),
            project_pubkey: key.verifying_key().to_bytes(),
            root_sha256: root_sha256(root),
            spawn: SpawnInfo { pid: 1, exe_sha: "ab".repeat(32) },
            pop_sig: ident::pop_sign(key, PopOp::Register, &uk, &n, id).unwrap(),
            nonce: claim_nonce(&n, id).unwrap(),
        },
        t0(),
    )
    .unwrap();
}

fn stmt(f: &Fx, seq: u64, prev: Option<String>, head_seq: u64, at: DateTime<Utc>) -> ProjectAnchorStmt {
    ProjectAnchorStmt {
        project_id: f.id.clone(),
        project_key_id: String::new(),
        cert_serial: 1,
        seq,
        chain_id: 0,
        head_hash: "ab".repeat(32),
        head_seq,
        rule_hash: "cd".repeat(32),
        at: ts(at),
        prev_anchor: prev,
        sig: String::new(),
    }
    .sign(&project_key())
}

fn user_events(f: &Fx) -> usize {
    f.env.chain.tail(0).iter().filter(|e| e.source == ANCHOR_SOURCE).count()
}

#[test]
fn accepts_a_valid_first_statement_and_records_it_twice() {
    let f = fixture();
    let s = stmt(&f, 1, None, 10, later(5));
    let a = submit(&f.env, &s, later(10)).unwrap();
    assert_eq!(user_events(&f), 1);
    let ev = f.env.chain.tail(0).into_iter().find(|e| e.source == ANCHOR_SOURCE).unwrap();
    assert_eq!(ev.kind, KIND_ANCHOR);
    assert_eq!(ev.sequence, a.user_seq);
    assert_eq!(ident::hex(&ev.hash), a.user_event_hash);
    assert_eq!(ev.payload.unwrap()["statement"]["head_seq"], 10);
    assert_eq!(last_accepted(&f.env, &f.id).unwrap(), Some(a));
    assert!(is_reserved_source(ANCHOR_SOURCE));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = anchor_file(&f.env.manifests_dir, &f.id);
        assert_eq!(std::fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn refuses_a_bad_signature() {
    let f = fixture();
    let mut s = stmt(&f, 1, None, 10, later(5));
    s.head_seq = 11; // signed over 10
    assert_eq!(submit(&f.env, &s, later(10)).unwrap_err().kind(), "anchor_bad_signature");
    // Signed by a key that is not the certified one.
    let other = stmt(&f, 1, None, 10, later(5)).sign(&SigningKey::from_bytes(&[3u8; 32]));
    let e = submit(&f.env, &other, later(10)).unwrap_err();
    assert_eq!(e.kind(), "anchor_key_revoked", "{e}");
    assert_eq!(user_events(&f), 0);
}

#[test]
fn refuses_a_replayed_seq_but_answers_an_identical_resend() {
    let f = fixture();
    let first = stmt(&f, 1, None, 10, later(5));
    let a = submit(&f.env, &first, later(10)).unwrap();
    // Identical resend: same acknowledgement, no second event.
    assert_eq!(submit(&f.env, &first, later(11)).unwrap(), a);
    assert_eq!(user_events(&f), 1);
    // A different statement under the same seq is refused and carries `last`.
    let other = stmt(&f, 1, None, 12, later(6));
    let e = submit(&f.env, &other, later(12)).unwrap_err();
    assert_eq!(e.kind(), "anchor_seq");
    assert_eq!(e.resync().unwrap().last.statement, first);
    assert_eq!(user_events(&f), 1);
}

#[test]
fn refuses_a_skipped_seq() {
    let f = fixture();
    let e = submit(&f.env, &stmt(&f, 2, None, 10, later(5)), later(10)).unwrap_err();
    assert_eq!(e.kind(), "anchor_seq");
    let first = stmt(&f, 1, None, 10, later(5));
    submit(&f.env, &first, later(10)).unwrap();
    let skip = stmt(&f, 3, Some(first.hash()), 20, later(6));
    assert_eq!(submit(&f.env, &skip, later(12)).unwrap_err().kind(), "anchor_seq");
    assert_eq!(user_events(&f), 1);
}

#[test]
fn refuses_a_forged_prev_anchor() {
    let f = fixture();
    let first = stmt(&f, 1, None, 10, later(5));
    submit(&f.env, &first, later(10)).unwrap();
    let forged = stmt(&f, 2, Some("00".repeat(32)), 20, later(6));
    assert_eq!(submit(&f.env, &forged, later(12)).unwrap_err().kind(), "anchor_prev");
    let none = stmt(&f, 2, None, 20, later(6));
    assert_eq!(submit(&f.env, &none, later(12)).unwrap_err().kind(), "anchor_prev");
    // The honest successor is accepted.
    submit(&f.env, &stmt(&f, 2, Some(first.hash()), 20, later(6)), later(12)).unwrap();
}

#[test]
fn refuses_a_revoked_or_replaced_key() {
    let f = fixture();
    let s = stmt(&f, 1, None, 10, later(5));
    revoke(&f.env, &json!({ "id": f.id })).unwrap();
    assert_eq!(submit(&f.env, &s, later(10)).unwrap_err().kind(), "anchor_key_revoked");

    let g = fixture();
    let old = stmt(&g, 1, None, 10, later(5));
    let new_key = SigningKey::from_bytes(&[4u8; 32]);
    let n = issue_challenge(&g.id).unwrap();
    let uk = clawft_types::project::cert::key_id(&user_key().verifying_key().to_bytes());
    rekey(
        &g.env,
        &json!({
            "id": g.id,
            "new_pubkey": ident::hex(&new_key.verifying_key().to_bytes()),
            "nonce": n,
            "pop_sig": ident::hex(&ident::pop_sign(&new_key, PopOp::Rekey, &uk, &n, &g.id).unwrap()),
        }),
        t0(),
    )
    .unwrap();
    assert_eq!(submit(&g.env, &old, later(10)).unwrap_err().kind(), "anchor_key_revoked");
    assert_eq!(user_events(&g), 0);
}

#[test]
fn refuses_an_uncertified_project() {
    let f = fixture();
    let mut s = stmt(&f, 1, None, 10, later(5));
    s.project_id = "01JZZZZZZZZZZZZZZZZZZZZZZZ".into();
    let s = s.sign(&project_key());
    let e = submit(&f.env, &s, later(10)).unwrap_err();
    assert_eq!(e.kind(), "anchor_not_certified", "{e}");
}

#[test]
fn refuses_a_future_at_but_accepts_the_edge_and_the_distant_past() {
    let f = fixture();
    let future = stmt(&f, 1, None, 10, later(10 + MAX_AHEAD_SECS + 1));
    assert_eq!(submit(&f.env, &future, later(10)).unwrap_err().kind(), "anchor_future");
    let edge = stmt(&f, 1, None, 10, later(10 + MAX_AHEAD_SECS));
    submit(&f.env, &edge, later(10)).unwrap();
    // A statement replayed after a long outage is old, and that is fine.
    let old = stmt(&f, 2, Some(edge.hash()), 11, later(-86_400 * 3));
    submit(&f.env, &old, later(20)).unwrap();
}

#[test]
fn refuses_a_head_that_goes_backwards() {
    let f = fixture();
    let first = stmt(&f, 1, None, 50, later(5));
    submit(&f.env, &first, later(10)).unwrap();
    let back = stmt(&f, 2, Some(first.hash()), 49, later(6));
    assert_eq!(submit(&f.env, &back, later(12)).unwrap_err().kind(), "anchor_head_regress");
    submit(&f.env, &stmt(&f, 2, Some(first.hash()), 50, later(6)), later(12)).unwrap();
}

#[test]
fn refuses_a_stale_cert_serial_and_malformed_statements() {
    let f = fixture();
    let mut s = stmt(&f, 1, None, 10, later(5));
    s.cert_serial = 9;
    let s = s.sign(&project_key());
    assert_eq!(submit(&f.env, &s, later(10)).unwrap_err().kind(), "anchor_cert_invalid");
    let mut bad = stmt(&f, 1, None, 10, later(5));
    bad.head_hash = "AB".repeat(32); // uppercase: not strict lowercase hex
    let bad = bad.sign(&project_key());
    assert_eq!(submit(&f.env, &bad, later(10)).unwrap_err().kind(), "anchor_bad_statement");
}

#[test]
fn accepted_anchor_survives_a_user_chain_lost_in_a_crash() {
    let f = fixture();
    let first = stmt(&f, 1, None, 10, later(5));
    let a = submit(&f.env, &first, later(10)).unwrap();
    // Same manifests (journal, certs, anchor file), a chain that was never saved.
    let env2 = CertEnv {
        chain: Arc::new(ChainManager::new(0, 100_000)),
        user_key: user_key(),
        manifests_dir: f.env.manifests_dir.clone(),
    };
    // Recovery re-appends the event: same statement, the rewritten ack.
    let rec = last_accepted(&env2, &f.id).unwrap().unwrap();
    assert_eq!(rec.statement, first);
    let ev = env2.chain.tail(0).into_iter().find(|e| e.source == ANCHOR_SOURCE).unwrap();
    assert_eq!((rec.user_seq, rec.user_event_hash.clone()), (ev.sequence, ident::hex(&ev.hash)));
    let p = ev.payload.unwrap();
    assert_eq!(p["recovered"], true);
    assert_eq!(p["original_user_seq"], a.user_seq);
    assert_eq!(p["original_user_event_hash"], a.user_event_hash);
    // A resend after recovery returns the rewritten ack and adds no event.
    assert_eq!(submit(&env2, &first, later(11)).unwrap(), rec);
    assert_eq!(env2.chain.tail(0).iter().filter(|e| e.source == ANCHOR_SOURCE).count(), 1);
    let next = stmt(&f, 2, Some(first.hash()), 20, later(6));
    submit(&env2, &next, later(12)).unwrap();
}

/// A transport that calls the RPC logic in-process, with a switchable link
/// and a clock the test sets.
struct Direct {
    env: CertEnv,
    down: Mutex<bool>,
    now: Mutex<DateTime<Utc>>,
    submits: Mutex<Vec<ProjectAnchorStmt>>,
}

impl ParentTransport for Direct {
    fn submit(&self, s: &ProjectAnchorStmt) -> Result<AnchorAck, AnchorSubmitError> {
        self.submits.lock().unwrap().push(s.clone());
        if *self.down.lock().unwrap() {
            return Err(AnchorSubmitError::Unreachable("user daemon is down".into()));
        }
        submit(&self.env, s, *self.now.lock().unwrap())
            .map(|a| a.ack())
            .map_err(|e| e.to_submit_error())
    }
}

#[test]
fn project_to_user_daemon_end_to_end_with_an_outage() {
    let f = fixture();
    let direct = Arc::new(Direct {
        env: CertEnv {
            chain: f.env.chain.clone(),
            user_key: user_key(),
            manifests_dir: f.env.manifests_dir.clone(),
        },
        down: Mutex::new(false),
        now: Mutex::new(later(0)),
        submits: Mutex::default(),
    });
    let pchain = Arc::new(ChainManager::new(0, 100_000));
    pchain.append("kernel", "boot", None);
    let run = tempfile::tempdir().unwrap();
    let pending = run.path().join("anchor.pending.jsonl");
    let anchor = ParentAnchor::new(
        pchain.clone(),
        project_key(),
        f.id.clone(),
        1,
        direct.clone(),
        &pending,
        ParentAnchorConfig::default(),
    );

    anchor.anchor_head_at(later(1), true).unwrap();
    assert_eq!(user_events(&f), 1);

    // Outage: the project keeps appending and anchors pile up as one pending statement.
    *direct.down.lock().unwrap() = true;
    for round in 0..3 {
        for _ in 0..120 {
            pchain.append("kernel", "work", None);
        }
        anchor.anchor_head_at(later(100 + round * 400), true).unwrap_err();
    }
    assert_eq!(anchor.pending().unwrap().seq, 2);
    assert_eq!(user_events(&f), 1);

    // The daemon returns: one replay, accepted, `project.anchored` recorded.
    *direct.down.lock().unwrap() = false;
    *direct.now.lock().unwrap() = later(5_000);
    direct.submits.lock().unwrap().clear();
    anchor.retry_pending_at(later(5_000)).unwrap().expect("replayed");
    assert_eq!(direct.submits.lock().unwrap().len(), 1);
    assert_eq!(user_events(&f), 2);
    let last = last_accepted(&f.env, &f.id).unwrap().unwrap();
    assert_eq!(last.statement.seq, 2);
    assert_eq!(last.statement.head_seq, pchain.tail(0).iter().rev().nth(1).unwrap().sequence);
    assert!(anchor.pending().is_none());
    assert!(
        pchain
            .tail(0)
            .iter()
            .any(|e| e.kind == "project.anchored" && e.payload.as_ref().unwrap()["user_seq"] == last.user_seq)
    );
}

fn rekey_to(f: &Fx, key: &SigningKey) {
    let n = issue_challenge(&f.id).unwrap();
    let uk = clawft_types::project::cert::key_id(&user_key().verifying_key().to_bytes());
    rekey(
        &f.env,
        &json!({
            "id": f.id,
            "new_pubkey": ident::hex(&key.verifying_key().to_bytes()),
            "nonce": n,
            "pop_sig": ident::hex(&ident::pop_sign(key, PopOp::Rekey, &uk, &n, &f.id).unwrap()),
        }),
        t0(),
    )
    .unwrap();
}

#[test]
fn key_history_has_rekeyed_out_keys_but_never_compromised_ones() {
    let f = fixture();
    let k2 = SigningKey::from_bytes(&[4u8; 32]);
    rekey_to(&f, &k2);
    let view = current_view(&f.env).unwrap();
    let hist = history_keys(&view, &f.id);
    assert_eq!(hist.len(), 2, "current and rekeyed-out");
    assert!(hist.contains(&ident::hex(&project_key().verifying_key().to_bytes())));

    let g = fixture();
    revoke(&g.env, &json!({ "id": g.id })).unwrap();
    let view = current_view(&g.env).unwrap();
    assert!(history_keys(&view, &g.id).is_empty(), "a compromise-revoked key is not history");
    assert_eq!(view.all_certs(&g.id).len(), 1, "but the daemon can still re-verify its own records");
}

fn direct(f: &Fx) -> Arc<Direct> {
    Arc::new(Direct {
        env: CertEnv {
            chain: f.env.chain.clone(),
            user_key: user_key(),
            manifests_dir: f.env.manifests_dir.clone(),
        },
        down: Mutex::new(false),
        now: Mutex::new(later(0)),
        submits: Mutex::default(),
    })
}

fn pa(f: &Fx, d: &Arc<Direct>, pchain: &Arc<ChainManager>, key: SigningKey, serial: u64, dir: &Path) -> ParentAnchor {
    ParentAnchor::new(
        pchain.clone(),
        key,
        f.id.clone(),
        serial,
        d.clone(),
        dir.join("anchor.pending.jsonl"),
        ParentAnchorConfig::default(),
    )
}

#[test]
fn rekey_then_restart_continues_the_anchor_chain_at_n_plus_one() {
    let f = fixture();
    let d = direct(&f);
    let pchain = Arc::new(ChainManager::new(0, 100_000));
    pchain.append("kernel", "boot", None);
    let run = tempfile::tempdir().unwrap();
    let k1 = pa(&f, &d, &pchain, project_key(), 1, run.path());
    k1.anchor_head_at(later(1), true).unwrap();
    for _ in 0..5 {
        pchain.append("kernel", "work", None);
    }
    k1.anchor_head_at(later(2), true).unwrap();
    drop(k1);
    for _ in 0..150 {
        pchain.append("kernel", "work", None);
    }

    let k2_key = SigningKey::from_bytes(&[4u8; 32]);
    rekey_to(&f, &k2_key);
    let k2 = pa(&f, &d, &pchain, k2_key.clone(), 2, run.path());
    k2.anchor_head_at(later(3), true).unwrap();
    assert_eq!(k2.last_accepted().unwrap().0, 3);
    let last = last_accepted(&f.env, &f.id).unwrap().unwrap();
    assert_eq!(last.statement.seq, 3);
    assert_eq!(last.statement.cert_serial, 2);
    assert_eq!(user_events(&f), 3);
}

#[test]
fn a_file_ahead_of_the_chain_is_re_appended_at_startup() {
    let f = fixture();
    let first = stmt(&f, 1, None, 10, later(5));
    let a = submit(&f.env, &first, later(10)).unwrap();
    // A crash: the chain is back to what was saved (nothing), the file stays.
    let env2 = CertEnv {
        chain: Arc::new(ChainManager::new(0, 100_000)),
        user_key: user_key(),
        manifests_dir: f.env.manifests_dir.clone(),
    };
    // Post-crash events reuse sequence numbers: the old user_seq now names another event.
    for _ in 0..=a.user_seq {
        env2.chain.append("kernel", "other", None);
    }
    // Replay the identity events so the view still sees the project (journal + cert file do).
    let fixed = reconcile(&env2).unwrap();
    assert_eq!(fixed, vec![f.id.clone()]);
    let ev = env2.chain.tail(0).into_iter().find(|e| e.source == ANCHOR_SOURCE).unwrap();
    assert_eq!(ev.payload.as_ref().unwrap()["recovered"], true);
    assert_eq!(ev.payload.as_ref().unwrap()["original_user_seq"], a.user_seq);
    let now_last = last_accepted(&env2, &f.id).unwrap().unwrap();
    assert_eq!(now_last.statement, first);
    assert_eq!(now_last.user_seq, ev.sequence);
    // The record now names the re-appended event, and a second pass changes nothing.
    assert!(reconcile(&env2).unwrap().is_empty());
    assert_eq!(env2.chain.tail(0).iter().filter(|e| e.source == ANCHOR_SOURCE).count(), 1);
}

#[test]
fn a_tampered_anchor_record_is_ignored() {
    let f = fixture();
    let first = stmt(&f, 1, None, 10, later(5));
    submit(&f.env, &first, later(10)).unwrap();
    let path = anchor_file(&f.env.manifests_dir, &f.id);
    let mut rec: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    rec["statement"]["head_seq"] = json!(999_999);
    std::fs::write(&path, serde_json::to_vec(&rec).unwrap()).unwrap();
    let env2 = CertEnv {
        chain: Arc::new(ChainManager::new(0, 100_000)),
        user_key: user_key(),
        manifests_dir: f.env.manifests_dir.clone(),
    };
    assert_eq!(last_accepted(&env2, &f.id).unwrap(), None, "a forged record does not set the baseline");
}

#[test]
fn authenticated_refusals_back_off_but_garbage_does_not() {
    let f = fixture();
    // Unsigned or badly signed garbage never counts.
    for i in 0..10 {
        let mut g = stmt(&f, 1, None, 10, later(5));
        g.head_seq = 11 + i;
        let _ = submit(&f.env, &g, later(10)).unwrap_err();
    }
    submit(&f.env, &stmt(&f, 1, None, 10, later(5)), later(10)).unwrap();
    // Three signed refusals in a row, then the honest next statement waits.
    for _ in 0..3 {
        let e = submit(&f.env, &stmt(&f, 9, None, 20, later(6)), later(11)).unwrap_err();
        assert_eq!(e.kind(), "anchor_seq");
    }
    let first_hash = last_accepted(&f.env, &f.id).unwrap().unwrap().statement.hash();
    let next = stmt(&f, 2, Some(first_hash), 20, later(6));
    assert_eq!(submit(&f.env, &next, later(11)).unwrap_err().kind(), "anchor_backoff");
    submit(&f.env, &next, later(13)).unwrap();
}

fn lost_chain_env(f: &Fx) -> CertEnv {
    CertEnv {
        chain: Arc::new(ChainManager::new(0, 100_000)),
        user_key: user_key(),
        manifests_dir: f.env.manifests_dir.clone(),
    }
}

#[test]
fn an_unwritable_record_still_yields_exactly_one_event() {
    let f = fixture();
    // A directory where the record file belongs: every write of it fails.
    std::fs::create_dir(anchor_file(&f.env.manifests_dir, &f.id)).unwrap();
    let s = stmt(&f, 1, None, 10, later(5));
    assert_eq!(submit(&f.env, &s, later(10)).unwrap_err().kind(), "anchor_store");
    // The event is on the chain and in the index: the retry is an identical resend.
    let a = submit(&f.env, &s, later(11)).unwrap();
    assert_eq!(a.statement, s);
    assert_eq!(user_events(&f), 1, "no second project.anchor event");
    // A refused store never counts towards the backoff.
    for _ in 0..4 {
        submit(&f.env, &s, later(12)).unwrap();
    }
    assert_eq!(user_events(&f), 1);
}

#[test]
fn a_record_with_a_bad_rec_sig_is_ignored() {
    let f = fixture();
    submit(&f.env, &stmt(&f, 1, None, 10, later(5)), later(10)).unwrap();
    let path = anchor_file(&f.env.manifests_dir, &f.id);
    let mut rec: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    rec["user_seq"] = json!(77);
    std::fs::write(&path, serde_json::to_vec(&rec).unwrap()).unwrap();
    assert_eq!(last_accepted(&lost_chain_env(&f), &f.id).unwrap(), None);
    // Unsigned records (or signed by another key) are ignored too.
    rec["user_seq"] = json!(1);
    rec.as_object_mut().unwrap().remove("rec_sig");
    std::fs::write(&path, serde_json::to_vec(&rec).unwrap()).unwrap();
    assert_eq!(last_accepted(&lost_chain_env(&f), &f.id).unwrap(), None);
}

#[test]
fn a_compromise_revoked_statement_is_not_re_appended() {
    let f = fixture();
    submit(&f.env, &stmt(&f, 1, None, 10, later(5)), later(10)).unwrap();
    revoke(&f.env, &json!({ "id": f.id })).unwrap();
    let env2 = lost_chain_env(&f);
    assert!(reconcile(&env2).unwrap().is_empty());
    assert_eq!(env2.chain.tail(0).iter().filter(|e| e.source == ANCHOR_SOURCE).count(), 0);
    assert_eq!(last_accepted(&env2, &f.id).unwrap(), None);

    // A rekeyed-out key is history, not compromise: it is re-appended.
    let g = fixture();
    submit(&g.env, &stmt(&g, 1, None, 10, later(5)), later(10)).unwrap();
    rekey_to(&g, &SigningKey::from_bytes(&[4u8; 32]));
    let env3 = lost_chain_env(&g);
    assert_eq!(reconcile(&env3).unwrap(), vec![g.id.clone()]);
}

#[test]
fn an_identical_resend_is_never_backed_off() {
    let f = fixture();
    let first = stmt(&f, 1, None, 10, later(5));
    submit(&f.env, &first, later(10)).unwrap();
    for _ in 0..5 {
        submit(&f.env, &stmt(&f, 9, None, 20, later(6)), later(11)).unwrap_err();
    }
    let next = stmt(&f, 2, Some(first.hash()), 20, later(6));
    assert_eq!(submit(&f.env, &next, later(11)).unwrap_err().kind(), "anchor_backoff");
    submit(&f.env, &first, later(11)).expect("the resend of the last accepted statement is not refused");
}
