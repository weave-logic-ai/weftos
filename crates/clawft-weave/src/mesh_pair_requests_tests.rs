//! Pending pair requests file, in a temp dir (never `~/.weftos`).

use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};

use super::*;

const NODE: &str = "0123456789abcdef0123456789abcdef";
const OTHER: &str = "fedcba9876543210fedcba9876543210";
const ULID_A: &str = "01K00000000000000000000000";
const ULID_B: &str = "01K00000000000000000000001";

fn v(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| (*x).to_owned()).collect()
}

#[test]
fn record_writes_0600_and_is_idempotent_per_node_and_project_set() {
    let d = tempfile::tempdir().unwrap();
    let r1 = record(d.path(), NODE, &v(&[ULID_A])).unwrap();
    let mode = std::fs::metadata(d.path().join(REQUESTS_FILE)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert!(uuid::Uuid::parse_str(&r1.request_id).is_ok());
    assert_eq!(r1.with_node, NODE);

    let again = record(d.path(), NODE, &v(&[ULID_A])).unwrap();
    assert_eq!(again, r1, "same node and projects: the same request");
    let wider = record(d.path(), NODE, &v(&[ULID_A, ULID_B])).unwrap();
    assert_ne!(wider.request_id, r1.request_id, "a different project set is a new request");
    let any = record(d.path(), NODE, &[]).unwrap();
    assert!(any.projects.is_empty());
    assert_eq!(list(d.path()).unwrap().len(), 3);
}

#[test]
fn record_validates_its_inputs() {
    let d = tempfile::tempdir().unwrap();
    assert!(record(d.path(), "pg", &[]).unwrap_err().contains("mesh node id"));
    assert!(record(d.path(), &NODE.to_uppercase(), &[]).is_err(), "lower-case hex only");
    assert!(record(d.path(), NODE, &v(&["not-a-ulid"])).unwrap_err().contains("ULID"));
    let dup = record(d.path(), NODE, &v(&[ULID_A, ULID_A])).unwrap();
    assert_eq!(dup.projects, v(&[ULID_A]), "duplicates collapse");
    let many: Vec<String> = (0..=MAX_PROJECTS).map(|i| format!("01K000000000000000000{i:05}")).collect();
    assert!(record(d.path(), NODE, &many).is_err());
    assert!(!d.path().join(REQUESTS_FILE).exists() || list(d.path()).unwrap().len() == 1);
}

#[test]
fn cancel_and_settle_drop_the_right_requests() {
    let d = tempfile::tempdir().unwrap();
    let a = record(d.path(), NODE, &v(&[ULID_A])).unwrap();
    let b = record(d.path(), OTHER, &v(&[ULID_B])).unwrap();
    let c = record(d.path(), OTHER, &[]).unwrap();

    assert!(cancel(d.path(), &a.request_id).unwrap());
    assert!(!cancel(d.path(), &a.request_id).unwrap(), "already gone");
    assert_eq!(list(d.path()).unwrap(), vec![b.clone(), c.clone()]);

    // The pair action for OTHER settles both of its requests.
    assert_eq!(settle(d.path(), OTHER, Some(&b.request_id)).unwrap(), 2);
    assert!(list(d.path()).unwrap().is_empty());
    assert_eq!(settle(d.path(), OTHER, None).unwrap(), 0);

    // A request id names a request for a node other than the action's peer.
    let x = record(d.path(), NODE, &[]).unwrap();
    assert_eq!(settle(d.path(), OTHER, Some(&x.request_id)).unwrap(), 1);
}

#[test]
fn the_file_caps_pending_requests_and_keeps_unknown_keys() {
    let d = tempfile::tempdir().unwrap();
    let hand = json!({"version": 1, "requests": [], "operator_note": "keep me"});
    crate::dashboard_token::write_atomic(&d.path().join(REQUESTS_FILE), &hand.to_string()).unwrap();
    for i in 0..MAX_STORED {
        record(d.path(), NODE, &v(&[&format!("01K000000000000000000{i:05}")])).unwrap();
    }
    let e = record(d.path(), OTHER, &[]).unwrap_err();
    assert!(e.contains("pending requests"), "{e}");
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join(REQUESTS_FILE)).unwrap()).unwrap();
    assert_eq!(raw["operator_note"], "keep me");
    assert_eq!(raw["version"], 1);
}

#[test]
fn attach_reports_at_most_eight_oldest_first() {
    let reqs: Vec<PairRequest> = (0..10)
        .map(|i| PairRequest {
            request_id: format!("r{i}"),
            with_node: NODE.into(),
            projects: vec![],
            requested_at: format!("2026-10-08T00:00:{i:02}Z"),
        })
        .collect();
    let mut report = json!({ "host": "x" });
    attach(&mut report, &reqs);
    let shown = report["pair_requests"].as_array().unwrap();
    assert_eq!(shown.len(), MAX_REPORTED);
    assert_eq!(shown[0]["request_id"], "r0");
    assert_eq!(shown[7]["request_id"], "r7");
    assert_eq!(shown[0]["with_node"], NODE);
    assert!(shown[0]["projects"].is_array());
    assert_eq!(report["host"], "x");
}

#[test]
fn a_loose_file_is_refused() {
    let d = tempfile::tempdir().unwrap();
    record(d.path(), NODE, &[]).unwrap();
    let p = d.path().join(REQUESTS_FILE);
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(list(d.path()).is_err());
    assert!(record(d.path(), OTHER, &[]).is_err());
}
