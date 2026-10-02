//! Unit tests for [`crate::token_rpc`] method logic.

use super::*;
use clawft_kernel::chain::ChainManager;

fn authority() -> TokenAuthority {
    TokenAuthority::new(Arc::new(ChainManager::new(0, 1000)), "node-t")
}

fn ok(r: Response) -> Value {
    assert!(r.ok, "expected ok, got {:?}", r.error);
    r.result.unwrap()
}

#[test]
fn issue_validate_list_revoke_roundtrip() {
    let a = authority();
    let v = ok(run(
        &a,
        "auth.token.issue",
        &json!({"label": "pg"}),
        Some("admin"),
    ));
    let secret = v["secret"].as_str().unwrap().to_owned();
    let id = v["id"].as_str().unwrap().to_owned();
    let val = ok(run(
        &a,
        "auth.token.validate",
        &json!({"token": secret}),
        None,
    ));
    assert_eq!(val["valid"], true);
    assert_eq!(val["token"]["id"], id);
    assert!(val["token"].get("secret").is_none());
    let list = ok(run(&a, "auth.token.list", &Value::Null, Some("admin")));
    assert_eq!(list["tokens"].as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(&secret));
    assert_eq!(
        ok(run(
            &a,
            "auth.token.revoke",
            &json!({"id": id}),
            Some("admin")
        ))["revoked"],
        true
    );
    assert_eq!(
        ok(run(
            &a,
            "auth.token.validate",
            &json!({"token": secret}),
            None
        ))["valid"],
        false
    );
}

#[test]
fn ttl_above_24h_is_an_error_not_a_clamp() {
    let a = authority();
    let r = run(
        &a,
        "auth.token.issue",
        &json!({"ttl_secs": 24 * 3600 + 1}),
        Some("admin"),
    );
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("24 h"));
    assert!(
        run(
            &a,
            "auth.token.issue",
            &json!({"ttl_secs": 24 * 3600}),
            Some("admin")
        )
        .ok
    );
    assert!(
        !run(
            &a,
            "auth.token.issue",
            &json!({"ttl_secs": 0}),
            Some("admin")
        )
        .ok
    );
    assert!(
        !run(
            &a,
            "auth.token.issue",
            &json!({"ttl_secs": "5"}),
            Some("admin")
        )
        .ok
    );
}

#[test]
fn project_must_be_a_ulid_and_round_trips() {
    let a = authority();
    assert!(
        !run(
            &a,
            "auth.token.issue",
            &json!({"project": "nope"}),
            Some("admin")
        )
        .ok
    );
    let p = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let v = ok(run(
        &a,
        "auth.token.issue",
        &json!({"project": p}),
        Some("admin"),
    ));
    assert_eq!(v["project"], p);
}

#[test]
fn a_token_cannot_issue_revoke_or_list() {
    let a = authority();
    let v = ok(run(&a, "auth.token.issue", &json!({}), Some("admin")));
    let secret = v["secret"].as_str().unwrap();
    for m in ["auth.token.issue", "auth.token.revoke", "auth.token.list"] {
        let r = run(&a, m, &json!({"id": "x"}), Some(secret));
        assert_eq!(r.error_kind.as_deref(), Some(TOKEN_CANNOT_MINT_KIND), "{m}");
    }
    // validate stays open to a token bearer (the gateway path)
    assert!(
        run(
            &a,
            "auth.token.validate",
            &json!({"token": secret}),
            Some(secret)
        )
        .ok
    );
}

#[test]
fn huge_ttl_is_an_error_not_a_panic() {
    let a = authority();
    for ttl in [i64::MAX, i64::MAX / 2, 1 << 60] {
        let r = run(
            &a,
            "auth.token.issue",
            &json!({ "ttl_secs": ttl }),
            Some("admin"),
        );
        assert!(!r.ok, "{ttl}");
    }
    // u64 above i64::MAX is not an i64 at all
    let r = run(
        &a,
        "auth.token.issue",
        &json!({ "ttl_secs": u64::MAX }),
        Some("admin"),
    );
    assert!(!r.ok);
}

#[test]
fn legacy_journals_merge_into_the_new_location_without_touching_the_old() {
    let dir = tempfile::tempdir().unwrap();
    let new = dir.path().join("run").join(JOURNAL_FILE);
    let old1 = dir.path().join("legacy").join(JOURNAL_FILE);
    let old2 = dir.path().join("beside").join(JOURNAL_FILE);
    std::fs::create_dir_all(old1.parent().unwrap()).unwrap();
    std::fs::create_dir_all(old2.parent().unwrap()).unwrap();
    std::fs::create_dir_all(new.parent().unwrap()).unwrap();
    std::fs::write(&new, "{\"r\":1}\n").unwrap();
    std::fs::write(&old1, "{\"r\":1}\n{\"r\":2}\n").unwrap();
    std::fs::write(&old2, "{\"r\":3}\n\n").unwrap();
    let missing = dir.path().join("none").join(JOURNAL_FILE);
    let before = std::fs::read_to_string(&old1).unwrap();

    merge_legacy_journals(&new, &[old1.clone(), missing, old2, new.clone()]);
    let merged = std::fs::read_to_string(&new).unwrap();
    assert_eq!(merged, "{\"r\":1}\n{\"r\":2}\n{\"r\":3}\n");
    assert_eq!(std::fs::read_to_string(&old1).unwrap(), before);

    // Idempotent: a second boot adds nothing.
    merge_legacy_journals(&new, &[old1]);
    assert_eq!(std::fs::read_to_string(&new).unwrap(), merged);
}

#[test]
fn merge_with_no_legacy_journal_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let new = dir.path().join("run").join(JOURNAL_FILE);
    merge_legacy_journals(&new, &[dir.path().join("absent")]);
    assert!(!new.exists());
}
