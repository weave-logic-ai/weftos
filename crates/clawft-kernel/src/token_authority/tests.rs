use super::*;

fn chain() -> Arc<ChainManager> {
    Arc::new(ChainManager::new(0, 1000))
}

fn auth(c: &Arc<ChainManager>) -> TokenAuthority {
    TokenAuthority::new(Arc::clone(c), "node-test")
}

fn owner() -> Issuer {
    Issuer { uid: Some(501) }
}

#[test]
fn issue_validate_roundtrip_and_project_claim() {
    let c = chain();
    let a = auth(&c);
    let (secret, info) = a
        .issue(
            "playground",
            None,
            Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".into()),
            &owner(),
        )
        .unwrap();
    assert!(secret.starts_with(SECRET_PREFIX));
    assert_eq!(secret.len(), SECRET_PREFIX.len() + 64);
    assert_eq!(info.id.len(), 16);
    assert_eq!(info.expires_at - info.issued_at, DEFAULT_TTL);
    assert_eq!(a.validate(&secret), Some(info.clone()));
    assert!(a.validate("wft_nope").is_none());
    // project round-trips onto the chain event
    let ev = c
        .tail_from(0)
        .into_iter()
        .find(|e| e.kind == KIND_ISSUED)
        .unwrap();
    assert_eq!(ev.payload.unwrap()["project"], "01ARZ3NDEKTSV4RRFFQ69G5FAV");
}

#[test]
fn secret_is_never_on_the_chain_or_in_listings() {
    let c = chain();
    let a = auth(&c);
    let (secret, _) = a.issue("x", None, None, &owner()).unwrap();
    a.revoke(&a.list()[0].id);
    let dump = serde_json::to_string(&c.tail_from(0)).unwrap();
    assert!(!dump.contains(&secret));
    assert!(!dump.contains(secret.strip_prefix(SECRET_PREFIX).unwrap()));
    let (s2, _) = a.issue("y", None, None, &owner()).unwrap();
    assert!(!serde_json::to_string(&a.list()).unwrap().contains(&s2));
    assert!(!format!("{:?}", a.list()).contains(&s2));
}

#[test]
fn ttl_is_capped_and_validated() {
    let a = auth(&chain());
    assert_eq!(
        a.issue("x", Some(MAX_TTL + Duration::seconds(1)), None, &owner())
            .unwrap_err(),
        TokenError::TtlTooLong
    );
    assert_eq!(
        a.issue("x", Some(Duration::zero()), None, &owner())
            .unwrap_err(),
        TokenError::TtlNotPositive
    );
    assert!(a.issue("x", Some(MAX_TTL), None, &owner()).is_ok());
    assert_eq!(
        a.issue("", None, None, &owner()).unwrap_err(),
        TokenError::BadLabel
    );
    assert_eq!(
        a.issue("a\nb", None, None, &owner()).unwrap_err(),
        TokenError::BadLabel
    );
}

#[test]
fn expiry_is_enforced_on_validate_and_list() {
    let a = auth(&chain());
    let now = Utc::now();
    let (s, info) = a
        .issue_at(now, "x", Some(Duration::minutes(1)), None, &owner())
        .unwrap();
    assert!(a.validate_at(now + Duration::seconds(59), &s).is_some());
    assert!(a.validate_at(info.expires_at, &s).is_none());
    assert!(a.list_at(info.expires_at).is_empty());
}

#[test]
fn revoke_removes_and_is_idempotent() {
    let c = chain();
    let a = auth(&c);
    let (s, info) = a.issue("x", None, None, &owner()).unwrap();
    assert!(a.revoke(&info.id));
    assert!(a.validate(&s).is_none());
    assert!(!a.revoke(&info.id));
    assert!(!a.revoke("0000000000000000"));
    assert_eq!(
        c.tail_from(0)
            .iter()
            .filter(|e| e.kind == KIND_REVOKED)
            .count(),
        1
    );
}

#[test]
fn rebuild_keeps_live_drops_revoked_and_expired() {
    let c = chain();
    let a = auth(&c);
    let now = Utc::now();
    let (live, _) = a.issue_at(now, "live", None, None, &owner()).unwrap();
    let (gone, gi) = a.issue_at(now, "revoked", None, None, &owner()).unwrap();
    let (old, _) = a
        .issue_at(
            now - Duration::hours(2),
            "old",
            Some(Duration::hours(1)),
            None,
            &owner(),
        )
        .unwrap();
    a.revoke(&gi.id);
    drop(a);

    // a second instance over the same chain, as after a restart
    let b = auth(&c);
    assert!(b.validate(&live).is_some());
    assert!(b.validate(&gone).is_none());
    assert!(b.validate(&old).is_none());
    assert_eq!(b.list().len(), 1);
}

#[test]
fn rebuild_survives_chain_save_and_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chain.rvf");
    let c = chain();
    let (secret, _) = auth(&c).issue("x", None, None, &owner()).unwrap();
    c.save_to_rvf(&path).unwrap();
    let reloaded = Arc::new(ChainManager::load_from_rvf(&path, 1000).unwrap());
    assert!(auth(&reloaded).validate(&secret).is_some());
}

#[test]
fn tampered_event_with_mismatched_id_is_ignored() {
    let c = chain();
    let real = hash_secret("wft_real");
    c.append(
        SOURCE,
        KIND_ISSUED,
        Some(json!({
            "id": "ffffffffffffffff",
            "sha256": hex(&real),
            "label": "forged",
            "issued_at": Utc::now().to_rfc3339(),
            "expires_at": (Utc::now() + Duration::hours(1)).to_rfc3339(),
        })),
    );
    assert!(auth(&c).validate("wft_real").is_none());
}

#[test]
fn ct_eq_and_hash_helpers() {
    let a = hash_secret("a");
    assert!(ct_eq(&a, &hash_secret("a")));
    assert!(!ct_eq(&a, &hash_secret("b")));
    let mut near = a;
    near[31] ^= 1;
    assert!(!ct_eq(&a, &near));
    assert_eq!(unhex32(&hex(&a)), Some(a));
    assert_eq!(id_of(&a), hex(&a)[..16]);
}

#[test]
fn issued_event_records_issuer_without_claiming_verification() {
    let c = chain();
    auth(&c).issue("x", None, None, &owner()).unwrap();
    let ev = c
        .tail_from(0)
        .into_iter()
        .find(|e| e.kind == KIND_ISSUED)
        .unwrap();
    let p = ev.payload.unwrap();
    assert_eq!(p["issuer"]["node_id"], "node-test");
    assert_eq!(p["issuer"]["uid"], 501);
    assert_eq!(p["issuer"]["uid_verified"], false);
}
