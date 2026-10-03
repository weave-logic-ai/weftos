//! User-key rotation (ADR-103 A13) across certificates, anchor records and
//! the journal: dirs are injected, never `HOME`.

use std::sync::Arc;

use clawft_kernel::chain::ChainManager;
use clawft_kernel::project_identity::{RevocationView, RotationLog, RotationRecord, verify_cert_historic};
use clawft_types::project::cert::key_id;
use ed25519_dalek::SigningKey;

use super::record::{anchor_file, append_event, read_file, seal, write_file};
use super::tests::{Fx, fixture, later, project_key, stmt, user_key};
use super::*;
use clawft_types::project::cert::PopOp;
use crate::project_cert_rpc::{
    RegisterRequest, SpawnInfo, chain_rotations, claim_nonce, current_view, issue_challenge, register, root_sha256,
};

fn new_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

/// The fixture's store after a rotation at `later(100)`, as the next daemon
/// boot sees it: the new user key, the saved chain back, and the rotation
/// chained after everything the old key sealed.
fn rotated(f: &Fx) -> CertEnv {
    let rec = RotationRecord::sign(&user_key(), &new_key(), None, later(100));
    RotationLog::new(&f.env.manifests_dir).append(&rec).unwrap();
    super::drop_cache(&f.env);
    let env = CertEnv {
        chain: f.env.chain.clone(),
        user_key: new_key(),
        manifests_dir: f.env.manifests_dir.clone(),
    };
    chain_rotations(&env).unwrap();
    env
}

#[test]
fn an_anchor_record_and_a_certificate_sealed_by_the_old_key_verify_across_a_rotation() {
    let f = fixture();
    let first = submit(&f.env, &stmt(&f, 1, None, 10, later(5)), later(10)).unwrap();
    let env = rotated(&f);

    // The record the old key sealed is still the baseline (the new chain is empty).
    let last = last_accepted(&env, &f.id).unwrap().expect("record survives the rotation");
    assert_eq!(last.statement, first.statement);
    // The certificate the old key issued still verifies and is in force.
    let view = current_view(&env).unwrap();
    assert_eq!(view.rejected(), 0);
    assert_eq!(view.current_cert(&f.id).map(|c| c.serial), Some(1));
    assert_eq!(view.current_cert(&f.id).unwrap().user_key_id, key_id(&user_key().verifying_key().to_bytes()));
    // A child that has not re-registered keeps anchoring under it, and the new
    // statement is sealed by the new key.
    let second = submit(&env, &stmt(&f, 2, Some(first.statement.hash()), 11, later(200)), later(210)).unwrap();
    assert_eq!(second.statement.seq, 2);
    let history = crate::project_cert_rpc::user_history(&env).unwrap();
    assert_eq!(history.rotations(), 1);
    assert_eq!(last_accepted(&env, &f.id).unwrap().unwrap().statement.seq, 2);
}

#[test]
fn a_record_the_old_key_seals_after_the_rotation_point_is_refused() {
    let f = fixture();
    submit(&f.env, &stmt(&f, 1, None, 10, later(5)), later(10)).unwrap();
    // A genuine anchor event, chained before the rotation.
    let genuine = append_event(&f.env, &stmt(&f, 1, None, 10, later(50)), None, 0);
    let env = rotated(&f);
    let old_env = CertEnv { chain: env.chain.clone(), user_key: user_key(), manifests_dir: env.manifests_dir.clone() };
    let view = current_view(&env).unwrap();

    // Control: an old-key seal on a statement dated before the point, naming
    // a chain event below the rotation, is read.
    write_file(&env, &genuine).unwrap();
    assert!(read_file(&env, &f.id, &view).is_some());

    // After the point: refused, so it cannot set the baseline.
    let late = seal(&old_env, stmt(&f, 1, None, 10, later(300)), genuine.user_seq, genuine.user_event_hash.clone(), 0);
    write_file(&env, &late).unwrap();
    assert!(read_file(&env, &f.id, &view).is_none());
    assert!(anchor_file(&env.manifests_dir, &f.id).exists());
}

/// A backdated record sealed with the stolen old key passes the time rule, so
/// the chain must be what refuses it (review follow-up: corroboration).
#[test]
fn an_old_key_record_without_a_chain_event_below_the_rotation_is_refused() {
    let f = fixture();
    submit(&f.env, &stmt(&f, 1, None, 10, later(5)), later(10)).unwrap();
    let genuine = append_event(&f.env, &stmt(&f, 1, None, 10, later(50)), None, 0);
    let env = rotated(&f);
    let old_env = CertEnv { chain: env.chain.clone(), user_key: user_key(), manifests_dir: env.manifests_dir.clone() };
    let view = current_view(&env).unwrap();

    // Dated before the point, signed by the old key, but naming no chain event.
    let invented = seal(&old_env, stmt(&f, 1, None, 10, later(50)), 3, "ab".repeat(32), 0);
    write_file(&env, &invented).unwrap();
    assert!(read_file(&env, &f.id, &view).is_none(), "no event with that seq and hash");

    // Right sequence, wrong hash.
    let wrong_hash = seal(&old_env, stmt(&f, 1, None, 10, later(50)), genuine.user_seq, "cd".repeat(32), 0);
    write_file(&env, &wrong_hash).unwrap();
    assert!(read_file(&env, &f.id, &view).is_none(), "the event hash must match");

    // A real event the daemon appended AFTER the rotation, back-dated and sealed
    // with the old key: the chain position is above the rotation point.
    let after = append_event(&old_env, &stmt(&f, 1, None, 10, later(60)), None, 0);
    assert!(after.user_seq > genuine.user_seq);
    write_file(&env, &after).unwrap();
    assert!(read_file(&env, &f.id, &view).is_none(), "an event above the rotation is not corroboration");
}

/// A genuine anchor event for statement A must not vouch for a record that
/// claims statement B with A's sequence and hash (sealed with the old key),
/// and the refusal leaves a marker for doctor, once.
#[test]
fn a_genuine_event_does_not_corroborate_a_different_statement() {
    let f = fixture();
    submit(&f.env, &stmt(&f, 1, None, 10, later(5)), later(10)).unwrap();
    let genuine = append_event(&f.env, &stmt(&f, 1, None, 10, later(50)), None, 0);
    let env = rotated(&f);
    let old_env = CertEnv { chain: env.chain.clone(), user_key: user_key(), manifests_dir: env.manifests_dir.clone() };
    let view = current_view(&env).unwrap();
    // Control: the genuine record is read.
    write_file(&env, &genuine).unwrap();
    assert!(read_file(&env, &f.id, &view).is_some());
    // Statement B (another head_seq), A's chain coordinates, old-key seal.
    let other = stmt(&f, 1, None, 99, later(50));
    assert_ne!(other.hash(), genuine.statement.hash());
    let forged = seal(&old_env, other, genuine.user_seq, genuine.user_event_hash.clone(), 0);
    write_file(&env, &forged).unwrap();
    assert!(read_file(&env, &f.id, &view).is_none(), "the event is for another statement");
    let marker = super::record::ignored_marker(&env.manifests_dir, &f.id);
    assert!(marker.exists(), "doctor is told");
    // A later valid record clears the marker.
    write_file(&env, &genuine).unwrap();
    assert!(!marker.exists());
    assert!(read_file(&env, &f.id, &view).is_some());
}

#[test]
fn a_certificate_the_old_key_issues_after_the_rotation_point_is_dropped() {
    let f = fixture();
    let env = rotated(&f);
    let forged = ident::sign_cert(
        &user_key(),
        &clawft_types::project::CertRequest {
            project_id: f.id.clone(),
            project_pubkey: SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes(),
            serial: 9,
            issued_at: later(300),
            expires_at: None,
        },
    )
    .unwrap();
    let history = crate::project_cert_rpc::user_history(&env).unwrap();
    assert!(verify_cert_historic(&forged, &history).is_err());
    // Backdated before the rotation point, high serial, no chain evidence: also dropped.
    let backdated = ident::sign_cert(
        &user_key(),
        &clawft_types::project::CertRequest {
            project_id: f.id.clone(),
            project_pubkey: SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes(),
            serial: 99,
            issued_at: later(10),
            expires_at: None,
        },
    )
    .unwrap();
    verify_cert_historic(&backdated, &history).expect("the time rule alone would accept it");
    std::fs::write(env.manifests_dir.join(format!("{}.cert.json", f.id)), serde_json::to_vec(&forged).unwrap()).unwrap();
    let view = current_view(&env).unwrap();
    assert_eq!(view.rejected(), 1);
    assert_eq!(view.current_cert(&f.id).map(|c| c.serial), Some(1), "the genuine certificate stays in force");
    // The backdated one, planted as a cert file, is dropped for lack of chain evidence.
    std::fs::write(env.manifests_dir.join(format!("{}.cert.json", f.id)), serde_json::to_vec(&backdated).unwrap()).unwrap();
    let view = current_view(&env).unwrap();
    assert_eq!(view.rejected(), 1);
    assert_eq!(view.last_serial(&f.id), 1, "serial 99 never entered the view");
    assert_eq!(view.current_cert(&f.id).map(|c| c.serial), Some(1));
    let _: RevocationView = view;
}

#[test]
fn a_child_registering_after_the_rotation_gets_a_certificate_from_the_new_key() {
    let f = fixture();
    let env = rotated(&f);
    let n = issue_challenge(&f.id).unwrap();
    let uk = key_id(&new_key().verifying_key().to_bytes());
    let root = clawft_types::project::find_by_id(&env.manifests_dir, &f.id).unwrap().unwrap().root;
    let issued = register(
        &env,
        RegisterRequest {
            project_id: f.id.clone(),
            project_pubkey: project_key().verifying_key().to_bytes(),
            root_sha256: root_sha256(&root),
            spawn: SpawnInfo { pid: 1, exe_sha: "ab".repeat(32) },
            pop_sig: ident::pop_sign(&project_key(), PopOp::Register, &uk, &n, &f.id).unwrap(),
            nonce: claim_nonce(&n, &f.id).unwrap(),
        },
        later(400),
    )
    .unwrap();
    assert!(issued.new);
    assert_eq!(issued.cert.serial, 2);
    assert_eq!(issued.cert.user_key_id, uk);
    assert_eq!(issued.cert.project_key_id, current_view(&env).unwrap().bound_key_id(&f.id).unwrap());
    // Idempotent afterwards.
    assert_eq!(current_view(&env).unwrap().current_cert(&f.id).unwrap().serial, 2);
}

#[test]
fn a_rotation_log_that_does_not_end_at_the_key_in_use_stops_verification() {
    let f = fixture();
    let _ = rotated(&f);
    let wrong = CertEnv {
        chain: Arc::new(ChainManager::new(0, 100)),
        user_key: SigningKey::from_bytes(&[8u8; 32]),
        manifests_dir: f.env.manifests_dir.clone(),
    };
    assert!(current_view(&wrong).is_err());
}

#[test]
fn a_deleted_or_truncated_rotation_log_is_rebuilt_from_the_chain() {
    let f = fixture();
    let env = rotated(&f);
    let log = RotationLog::new(&env.manifests_dir);
    std::fs::remove_file(log.path()).unwrap();
    // Without repair this would silently verify against the new key only.
    chain_rotations(&env).unwrap();
    assert_eq!(log.read().unwrap().len(), 1);
    assert_eq!(crate::project_cert_rpc::user_history(&env).unwrap().rotations(), 1);
    assert_eq!(current_view(&env).unwrap().current_cert(&f.id).map(|c| c.serial), Some(1));
    // A chain whose records do not end at the key in use cannot rebuild it.
    std::fs::remove_file(log.path()).unwrap();
    let wrong = CertEnv { chain: env.chain.clone(), user_key: SigningKey::from_bytes(&[8u8; 32]), manifests_dir: env.manifests_dir.clone() };
    let e = chain_rotations(&wrong).unwrap_err();
    assert!(e.to_string().contains("rotation log is missing"), "{e}");
}

#[test]
fn rotation_records_are_chained_once() {
    let f = fixture();
    let env = rotated(&f);
    // `rotated` already chained it; doing so again appends nothing.
    assert_eq!(chain_rotations(&env).unwrap(), 0);
    assert_eq!(env.chain.tail_from(0).iter().filter(|e| e.kind == ident::KIND_ROTATED).count(), 1);
    let ev = env.chain.tail_from(0).into_iter().find(|e| e.kind == ident::KIND_ROTATED).unwrap();
    assert_eq!(ev.source, ident::SOURCE);
    assert_eq!(ev.payload.unwrap()["record"]["seq"], 1);
}
