//! `project.anchor.reset` (review S2): dirs are injected, never `HOME`.

use std::sync::Arc;

use clawft_kernel::chain::ChainManager;
use serde_json::json;

use super::reset_record::current_epoch;
use super::tests::{Fx, fixture, later, project_key, stmt};
use super::*;

/// The same store after a user-daemon restart that lost the chain.
fn restarted(f: &Fx) -> CertEnv {
    CertEnv {
        chain: Arc::new(ChainManager::new(0, 100_000)),
        user_key: f.env.user_key.clone(),
        manifests_dir: f.env.manifests_dir.clone(),
    }
}

/// A clean restart: the chain was saved and is back.
fn clean_restart(f: &Fx) -> CertEnv {
    super::drop_cache(&f.env);
    CertEnv {
        chain: f.env.chain.clone(),
        user_key: f.env.user_key.clone(),
        manifests_dir: f.env.manifests_dir.clone(),
    }
}

fn params(f: &Fx) -> Value {
    json!({ "project_id": f.id, "reason": "moved the chain\naside" })
}

fn anchor_two(f: &Fx) -> Accepted {
    let a = submit(&f.env, &stmt(f, 1, None, 10, later(5)), later(10)).unwrap();
    submit(&f.env, &stmt(f, 2, Some(a.statement.hash()), 11, later(6)), later(11)).unwrap()
}

#[test]
fn a_project_chain_reset_is_a_dead_end_until_the_owner_resets_the_anchors() {
    let f = fixture();
    let head = anchor_two(&f);
    // The project moved its chain aside and restarts at seq 1 with no predecessor.
    let genesis = stmt(&f, 1, None, 1, later(20));
    let e = submit(&f.env, &genesis, later(21)).unwrap_err();
    assert_eq!(e.kind(), "anchor_seq", "{e}");
    assert!(e.to_string().contains("weaver project anchor reset"), "{e}");

    let out = reset(&f.env, &params(&f), later(30)).unwrap();
    assert_eq!(out["epoch"], 1);
    assert_eq!(out["retired"]["seq"], 2);
    assert_eq!(out["retired"]["statement_hash"], head.statement.hash());
    assert_eq!(last_accepted(&f.env, &f.id).unwrap(), None, "the retired head is no baseline");

    // The very statement that was refused is accepted now, from genesis.
    let a = submit(&f.env, &genesis, later(31)).unwrap();
    assert_eq!((a.statement.seq, a.epoch), (1, 1));
    submit(&f.env, &stmt(&f, 2, Some(a.statement.hash()), 2, later(22)), later(32)).unwrap();

    // The old statements and the reset are history on the user chain.
    let ev = f.env.chain.tail(0);
    assert_eq!(ev.iter().filter(|e| e.kind == KIND_ANCHOR).count(), 4);
    let r = ev.iter().find(|e| e.kind == KIND_RESET).expect("reset is chained");
    assert_eq!(r.source, ANCHOR_SOURCE);
    let p = r.payload.clone().unwrap();
    assert_eq!((p["epoch"].clone(), p["reason"].clone()), (json!(1), json!("moved the chainaside")));
    assert_eq!(p["retired"]["seq"], 2);
}

#[test]
fn the_reset_survives_a_clean_restart() {
    let f = fixture();
    anchor_two(&f);
    reset(&f.env, &params(&f), later(30)).unwrap();
    let env = clean_restart(&f);
    assert_eq!(current_epoch(&env, &f.id), 1);
    // The epoch-0 anchor record is still on disk but is not the baseline.
    assert!(anchor_file_exists(&env, &f.id));
    assert_eq!(last_accepted(&env, &f.id).unwrap(), None);
    let a = submit(&env, &stmt(&f, 1, None, 1, later(40)), later(41)).unwrap();
    assert_eq!(a.epoch, 1);
    // A second restart finds the epoch-1 record as the baseline.
    let env2 = clean_restart(&f);
    let last = last_accepted(&env2, &f.id).unwrap().unwrap();
    assert_eq!((last.statement.seq, last.epoch), (1, 1));
    submit(&env2, &stmt(&f, 2, Some(last.statement.hash()), 2, later(42)), later(43)).unwrap();
}

fn anchor_file_exists(env: &CertEnv, id: &str) -> bool {
    env.manifests_dir.join(format!("{id}.anchor.json")).exists()
}

#[test]
fn a_reset_needs_a_registered_project_and_a_head_to_retire() {
    let f = fixture();
    // Nothing accepted yet.
    assert_eq!(reset(&f.env, &params(&f), later(1)).unwrap_err().kind(), "anchor_nothing_to_reset");
    anchor_two(&f);
    let unknown = json!({ "project_id": "01JZZZZZZZZZZZZZZZZZZZZZZZ" });
    assert_eq!(reset(&f.env, &unknown, later(2)).unwrap_err().kind(), "anchor_bad_statement");
    assert_eq!(reset(&f.env, &json!({}), later(2)).unwrap_err().kind(), "anchor_bad_statement");
    reset(&f.env, &params(&f), later(3)).unwrap();
    // Twice in a row retires nothing the second time.
    assert_eq!(reset(&f.env, &params(&f), later(4)).unwrap_err().kind(), "anchor_nothing_to_reset");
}

#[test]
fn a_crash_that_lost_the_chain_loses_the_reset_and_it_can_be_repeated() {
    let f = fixture();
    anchor_two(&f);
    reset(&f.env, &params(&f), later(30)).unwrap();
    // The record file alone is not evidence: no chain event, no epoch.
    let env = restarted(&f);
    assert_eq!(current_epoch(&env, &f.id), 0);
    assert!(super::reset_record::corroborated(&env, &f.id).is_empty());
    assert_eq!(last_accepted(&env, &f.id).unwrap().unwrap().statement.seq, 2);
    assert_eq!(reset(&env, &params(&f), later(31)).unwrap()["epoch"], 1);
}

#[test]
fn an_epoch_jump_is_ignored_even_with_a_valid_seal() {
    let f = fixture();
    anchor_two(&f);
    // Seal a record for epoch 999 with the real user key, as only the daemon could.
    let mut rec = ResetRecord {
        project_id: f.id.clone(),
        epoch: 999,
        retired_statement_hash: "00".repeat(32),
        retired_seq: 2,
        user_seq: 1,
        user_event_hash: "00".repeat(32),
        at: ts(later(1)),
        reason: String::new(),
        rec_sig: String::new(),
    };
    super::reset_record::sign(&f.env, &mut rec);
    super::reset_record::record(&f.env, &rec).unwrap();
    // And a chain event that jumps the epoch.
    f.env.chain.append(
        ANCHOR_SOURCE,
        KIND_RESET,
        Some(json!({ "project_id": f.id, "epoch": 999, "at": ts(later(1)) })),
    );
    let env = clean_restart(&f);
    assert_eq!(current_epoch(&env, &f.id), 0);
    assert_eq!(last_accepted(&env, &f.id).unwrap().unwrap().statement.seq, 2, "history intact");
    let a = submit(&env, &stmt(&f, 3, Some(last_accepted(&env, &f.id).unwrap().unwrap().statement.hash()), 12, later(7)), later(12)).unwrap();
    assert_eq!(a.epoch, 0);
}

#[test]
fn a_forged_reset_record_changes_nothing() {
    let f = fixture();
    anchor_two(&f);
    let forged = json!({ "resets": [{
        "project_id": f.id, "epoch": 9, "retired_statement_hash": "00".repeat(32), "retired_seq": 2,
        "user_seq": 1, "user_event_hash": "00".repeat(32), "at": ts(later(1)), "reason": "", "rec_sig": "00".repeat(64),
    }]});
    std::fs::write(f.env.manifests_dir.join(format!("{}.anchor-reset.json", f.id)), forged.to_string()).unwrap();
    let env = clean_restart(&f);
    assert_eq!(current_epoch(&env, &f.id), 0);
    assert_eq!(last_accepted(&env, &f.id).unwrap().unwrap().statement.seq, 2);
}

#[test]
fn a_different_chain_id_inside_an_epoch_is_refused() {
    let f = fixture();
    let a = submit(&f.env, &stmt(&f, 1, None, 10, later(5)), later(10)).unwrap();
    let mut s = stmt(&f, 2, Some(a.statement.hash()), 11, later(6));
    s.chain_id = 7;
    let s = s.sign(&project_key());
    let e = submit(&f.env, &s, later(11)).unwrap_err();
    assert_eq!(e.kind(), "anchor_chain_id", "{e}");
}
