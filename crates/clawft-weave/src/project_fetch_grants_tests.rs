//! `project-fetch.json` reader and writer, in a temp dir (never `~/.weftos`).

use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};

use super::*;

const PEER: &str = "0123456789abcdef0123456789abcdef";
const OTHER: &str = "fedcba9876543210fedcba9876543210";
const ULID_A: &str = "01K00000000000000000000000";
const ULID_B: &str = "01K00000000000000000000001";

fn g(peer: &str, projects: &[&str]) -> Grant {
    Grant {
        peer_node: peer.into(),
        projects: projects.iter().map(|s| (*s).to_owned()).collect(),
        granted_at: Some("2026-10-08T00:00:00Z".into()),
        source: Some("dashboard-pair:act-1".into()),
        peer_ed25519: None,
    }
}

fn raw(dir: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join(FETCH_FILE)).unwrap()).unwrap()
}

#[test]
fn absent_file_grants_nothing() {
    let d = tempfile::tempdir().unwrap();
    let gr = Grants::load(d.path()).unwrap();
    assert!(gr.grants.is_empty());
    assert!(!gr.is_granted(PEER, ULID_A));
}

#[test]
fn grant_writes_0600_and_is_granted_only_for_that_peer_and_project() {
    let d = tempfile::tempdir().unwrap();
    let path = grant(d.path(), &g(PEER, &[ULID_A])).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let gr = Grants::load(d.path()).unwrap();
    assert!(gr.is_granted(PEER, ULID_A));
    assert!(!gr.is_granted(PEER, ULID_B), "another project");
    assert!(!gr.is_granted(OTHER, ULID_A), "another peer");
    assert_eq!(raw(d.path())["version"], 1);
}

#[test]
fn re_grant_replaces_rather_than_duplicates() {
    let d = tempfile::tempdir().unwrap();
    grant(d.path(), &g(PEER, &[ULID_A])).unwrap();
    grant(d.path(), &g(PEER, &[ULID_A, ULID_B])).unwrap();
    let gr = Grants::load(d.path()).unwrap();
    assert_eq!(gr.for_peer(PEER).len(), 1);
    assert!(gr.is_granted(PEER, ULID_B));
}

#[test]
fn unknown_entries_and_keys_are_preserved() {
    let d = tempfile::tempdir().unwrap();
    let hand = json!({
        "version": 1,
        "note": "operator wrote this",
        "grants": [
            {"peer_node": OTHER, "projects": [ULID_B], "custom": true},
            "not even an object"
        ]
    });
    crate::dashboard_token::write_atomic(&d.path().join(FETCH_FILE), &hand.to_string()).unwrap();
    grant(d.path(), &g(PEER, &[ULID_A])).unwrap();
    let after = raw(d.path());
    assert_eq!(after["note"], "operator wrote this");
    assert_eq!(after["grants"].as_array().unwrap().len(), 3);
    assert_eq!(after["grants"][0]["custom"], true);
    assert_eq!(after["grants"][1], "not even an object");
    let gr = Grants::load(d.path()).unwrap();
    assert!(gr.is_granted(OTHER, ULID_B));
    assert!(gr.is_granted(PEER, ULID_A));

    assert!(revoke(d.path(), PEER).unwrap());
    assert!(!revoke(d.path(), PEER).unwrap(), "nothing left to revoke");
    let after = raw(d.path());
    assert_eq!(after["grants"].as_array().unwrap().len(), 2);
    assert!(!Grants::load(d.path()).unwrap().is_granted(PEER, ULID_A));
}

#[test]
fn a_loose_file_or_a_wrong_version_is_refused_not_ignored() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(FETCH_FILE);
    std::fs::write(&p, json!({"version": 1, "grants": [{"peer_node": PEER, "projects": [ULID_A]}]}).to_string()).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o664)).unwrap();
    let e = Grants::load(d.path()).unwrap_err();
    assert!(e.contains("world-writable") || e.contains("group-"), "{e}");
    assert!(grant(d.path(), &g(PEER, &[ULID_A])).is_err());

    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Grants::load(d.path()).unwrap().is_granted(PEER, ULID_A));
    std::fs::write(&p, json!({"version": 2, "grants": []}).to_string()).unwrap();
    assert!(Grants::load(d.path()).unwrap_err().contains("version"));
    std::fs::write(&p, "{").unwrap();
    assert!(Grants::load(d.path()).is_err());
}
