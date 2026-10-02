//! Record and recovery tests for [`super`]: the signed anchor record, the
//! startup reconcile and the crash-safe write order.

use std::sync::Arc;

use clawft_kernel::chain::ChainManager;

use super::tests::{Fx, fixture, later, rekey_to, stmt, user_events, user_key};
use super::record::anchor_file;
use super::*;
use crate::project_cert_rpc::{revoke};
use ed25519_dalek::SigningKey;
use clawft_kernel::project_identity as ident;


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

fn restore_params(s: &ProjectAnchorStmt, user_seq: u64) -> serde_json::Value {
    serde_json::json!({ "statement": s, "user_seq": user_seq, "user_event_hash": "ab".repeat(32) })
}

#[test]
fn restore_reseeds_a_parent_that_lost_its_state() {
    let f = fixture();
    let s1 = stmt(&f, 1, None, 10, later(5));
    let s2 = stmt(&f, 2, Some(s1.hash()), 20, later(6));
    submit(&f.env, &s1, later(10)).unwrap();
    submit(&f.env, &s2, later(11)).unwrap();
    // The user daemon loses everything about anchors: record file and chain.
    std::fs::remove_file(anchor_file(&f.env.manifests_dir, &f.id)).unwrap();
    let env2 = lost_chain_env(&f);
    let s3 = stmt(&f, 3, Some(s2.hash()), 30, later(7));
    let e = submit(&env2, &s3, later(12)).unwrap_err();
    assert_eq!(e.kind(), "anchor_seq");
    assert!(e.to_string().contains("project.anchor.restore"), "{e}");

    let out = restore(&env2, &restore_params(&s2, 2), later(13)).unwrap();
    assert_eq!(out["seq"], 2);
    assert_eq!(last_accepted(&env2, &f.id).unwrap().unwrap().statement, s2);
    let ev = env2.chain.tail(0).into_iter().find(|e| e.source == ANCHOR_SOURCE).unwrap();
    assert_eq!(ev.payload.unwrap()["recovered"], true);
    submit(&env2, &s3, later(14)).unwrap();
    // A restart still finds the signed record.
    assert_eq!(last_accepted(&lost_chain_env(&f), &f.id).unwrap().unwrap().statement, s3);
}

#[test]
fn restore_refuses_forgeries_rewinds_and_compromised_keys() {
    let f = fixture();
    let s1 = stmt(&f, 1, None, 10, later(5));
    let s2 = stmt(&f, 2, Some(s1.hash()), 20, later(6));
    submit(&f.env, &s1, later(10)).unwrap();
    submit(&f.env, &s2, later(11)).unwrap();

    let mut forged = s2.clone();
    forged.head_seq = 21;
    assert_eq!(restore(&f.env, &restore_params(&forged, 2), later(13)).unwrap_err().kind(), "anchor_bad_signature");
    assert_eq!(
        restore(&f.env, &restore_params(&s1, 1), later(13)).unwrap_err().kind(),
        "anchor_bad_statement",
        "never rewinds"
    );
    assert!(restore(&f.env, &serde_json::json!({ "statement": s2 }), later(13)).is_err());

    // Identical restore is a no-op success.
    restore(&f.env, &restore_params(&s2, 2), later(13)).unwrap();

    revoke(&f.env, &serde_json::json!({ "id": f.id })).unwrap();
    let env2 = lost_chain_env(&f);
    std::fs::remove_file(anchor_file(&f.env.manifests_dir, &f.id)).unwrap();
    assert_eq!(restore(&env2, &restore_params(&s2, 2), later(14)).unwrap_err().kind(), "anchor_key_revoked");
}

#[test]
fn restore_accepts_a_rekeyed_out_key() {
    let f = fixture();
    let s1 = stmt(&f, 1, None, 10, later(5));
    submit(&f.env, &s1, later(10)).unwrap();
    rekey_to(&f, &SigningKey::from_bytes(&[4u8; 32]));
    std::fs::remove_file(anchor_file(&f.env.manifests_dir, &f.id)).unwrap();
    let env2 = lost_chain_env(&f);
    restore(&env2, &restore_params(&s1, 1), later(14)).unwrap();
    assert_eq!(last_accepted(&env2, &f.id).unwrap().unwrap().statement, s1);
}
