//! Forward-v2 refusals: the guest executes a request only when the pinned
//! user key signed exactly this project, method, params and target, freshly.
use super::*;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;

const PROJECT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const TARGET: &str = "target-key-id";
const NOW: u64 = 1_800_000_000_000;

fn user() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn signed(key: &SigningKey, project: &str, method: &str, params: Value, at: u64) -> Value {
    let hash = hex_encode(&Sha256::digest(canonical_json(&params).as_bytes()));
    let msg = format!("weftos-project-forward-v2\n{project}\n{at}\n{method}\n{hash}\n{TARGET}");
    let sig = hex_encode(&key.sign(msg.as_bytes()).to_bytes());
    json!({
        "method": method,
        "project": project,
        "params": params,
        "forward": {"project_id": project, "issued_at_ms": at, "sig": sig},
    })
}

fn verify(f: &mut Forward, req: &Value) -> Result<()> {
    f.verify(req, PROJECT, TARGET, &user().verifying_key().to_bytes(), NOW)
}

#[test]
fn a_fresh_signed_request_is_accepted_once() {
    let mut f = Forward::new();
    let req = signed(&user(), PROJECT, "chain.append", json!({"kind": "k"}), NOW);
    verify(&mut f, &req).unwrap();
    assert_eq!(verify(&mut f, &req).unwrap_err().to_string(), "forward_replayed");
}

#[test]
fn tampered_or_forged_requests_are_refused() {
    let mut f = Forward::new();
    let mut req = signed(&user(), PROJECT, "chain.append", json!({"kind": "k"}), NOW);
    req["params"]["kind"] = json!("changed.after.signing");
    assert!(verify(&mut f, &req).is_err());
    let mut req = signed(&user(), PROJECT, "chain.status", json!({}), NOW);
    req["method"] = json!("kernel.stop");
    assert!(verify(&mut f, &req).is_err());
    let stranger = SigningKey::from_bytes(&[9u8; 32]);
    let req = signed(&stranger, PROJECT, "kernel.stop", json!({}), NOW);
    assert!(verify(&mut f, &req).is_err());
    let mut req = signed(&user(), PROJECT, "kernel.stop", json!({}), NOW);
    req["forward"].as_object_mut().unwrap().remove("sig");
    assert!(verify(&mut f, &req).is_err());
}

#[test]
fn stale_future_and_cross_project_requests_are_refused() {
    let mut f = Forward::new();
    let old = signed(&user(), PROJECT, "chain.status", json!({}), NOW - 6000);
    assert_eq!(verify(&mut f, &old).unwrap_err().to_string(), "forward_expired");
    let future = signed(&user(), PROJECT, "chain.status", json!({}), NOW + 6000);
    assert_eq!(verify(&mut f, &future).unwrap_err().to_string(), "forward_expired");
    let other = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    let wrong = signed(&user(), other, "chain.status", json!({}), NOW);
    assert_eq!(verify(&mut f, &wrong).unwrap_err().to_string(), "project_scope_mismatch");
    let mut half = signed(&user(), PROJECT, "chain.status", json!({}), NOW);
    half["forward"]["project_id"] = json!(other);
    assert_eq!(verify(&mut f, &half).unwrap_err().to_string(), "project_scope_mismatch");
}
