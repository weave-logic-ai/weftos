use std::path::Path;

use clawft_kernel::node_id_from_pubkey;
use clawft_kernel::workload_ctl::msg::{ControllerPolicy, method};
use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::*;

const A: &str = "01K6ZQ8N3T4V5W6X7Y8Z9A0B1C";
const B: &str = "01K6ZQ8N3T4V5W6X7Y8Z9A0B1D";

fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}

fn node(k: &SigningKey) -> String {
    node_id_from_pubkey(&k.verifying_key().to_bytes())
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

fn peers(dir: &Path, entries: &[(&SigningKey, &str)]) {
    let list: Vec<String> = entries
        .iter()
        .map(|(k, tier)| format!("{{\"addr\":\"127.0.0.1:9471\",\"tier\":\"{tier}\",\"key\":\"{}\"}}", hex::encode(k.verifying_key().to_bytes())))
        .collect();
    write(dir, "workload-peers.json", &format!("[{}]", list.join(",")));
}

fn grant(dir: &Path, k: &SigningKey, projects: &[&str]) {
    let p: Vec<String> = projects.iter().map(|s| format!("\"{s}\"")).collect();
    write(dir, GRANTS_FILE, &format!("{{\"version\":1,\"grants\":[{{\"peer_node\":\"{}\",\"projects\":[{}],\"granted_at\":\"2026-10-08T00:00:00Z\",\"source\":\"test\"}}]}}", node(k), p.join(",")));
}

#[test]
fn no_file_means_no_grants_and_a_bad_file_denies_everything() {
    let d = tempfile::tempdir().unwrap();
    let g = FetchGrants::load(d.path()).unwrap();
    assert!(!g.allows("n", A) && !g.any_for("n"));
    write(d.path(), GRANTS_FILE, r#"{"version":1,"grants":[{"peer_node":"n","projects":["not-a-ulid"]}]}"#);
    assert!(FetchGrants::load(d.path()).is_err());
    write(d.path(), GRANTS_FILE, r#"{"version":2,"grants":[]}"#);
    assert!(FetchGrants::load(d.path()).is_err());
    write(d.path(), GRANTS_FILE, "{not json");
    assert!(FetchGrants::load(d.path()).is_err());
    let m = key(9);
    peers(d.path(), &[(&m, "pinned")]);
    assert!(authorize(d.path(), &node(&m), A).unwrap_err().contains("unreadable"));
}

#[test]
fn authorize_needs_a_keyed_peer_at_paired_or_better_and_a_matching_grant() {
    let d = tempfile::tempdir().unwrap();
    let (m, other) = (key(1), key(2));
    peers(d.path(), &[(&m, "paired")]);
    grant(d.path(), &m, &[A]);
    assert_eq!(authorize(d.path(), &node(&m), A).unwrap(), TrustTier::Paired);
    // Another project's ULID.
    assert!(authorize(d.path(), &node(&m), B).unwrap_err().contains("no fetch grant"));
    // A peer that is not listed, even with a grant.
    grant(d.path(), &other, &[A]);
    assert!(authorize(d.path(), &node(&other), A).unwrap_err().contains("not a pinned or paired peer"));
    // Listed but only discovered.
    peers(d.path(), &[(&m, "discovered")]);
    grant(d.path(), &m, &[A]);
    assert!(authorize(d.path(), &node(&m), A).is_err());
    // Listed without a key: an address is not a node.
    write(d.path(), "workload-peers.json", r#"[{"addr":"127.0.0.1:9471","tier":"pinned"}]"#);
    assert!(authorize(d.path(), &node(&m), A).is_err());
    // Revoked: the grant file is removed.
    peers(d.path(), &[(&m, "pinned")]);
    std::fs::remove_file(d.path().join(GRANTS_FILE)).unwrap();
    assert!(authorize(d.path(), &node(&m), A).unwrap_err().contains("no fetch grant"));
}

#[test]
fn the_policy_admits_fetch_peers_for_describe_and_fetch_only() {
    let d = tempfile::tempdir().unwrap();
    let (ctl, m, stranger) = (key(3), key(4), key(5));
    peers(d.path(), &[(&m, "paired")]);
    grant(d.path(), &m, &[A]);
    let ctl_pk = ctl.verifying_key().to_bytes();
    let (m_pk, s_pk) = (m.verifying_key().to_bytes(), stranger.verifying_key().to_bytes());
    let p = FetchPeerPolicy::new(vec![ctl_pk], d.path().to_path_buf());
    for meth in [method::DESCRIBE, method::PROJECT_FETCH, method::PLACE, method::DASHBOARD_ROTATE] {
        assert!(p.allows_method(&ctl_pk, meth), "{meth}: a controller");
        assert!(!p.allows_method(&s_pk, meth), "{meth}: a stranger");
    }
    assert!(p.allows_method(&m_pk, method::DESCRIBE));
    assert!(p.allows_method(&m_pk, method::PROJECT_FETCH));
    assert!(!p.allows_method(&m_pk, method::PLACE), "a fetch peer never places");
    assert!(!p.allows_method(&m_pk, method::DASHBOARD_ROTATE));
    assert!(!p.allows_method(&m_pk, method::STATUS));
    assert!(!p.allows(&m_pk), "a fetch peer is not a controller");
    // Remove the grant: describe and fetch close too.
    std::fs::remove_file(d.path().join(GRANTS_FILE)).unwrap();
    assert!(!p.allows_method(&m_pk, method::PROJECT_FETCH));
    assert!(!p.allows_method(&m_pk, method::DESCRIBE));
}
