use clawft_mesh_local::{node_id_from_pubkey, Principal};
use clawft_mesh_service::{BindError, BindHow, BindMeta, Bindings, Check, ConflictReason, Journal};
use ed25519_dalek::SigningKey;

fn open(dir: &std::path::Path) -> Journal {
    Journal::open(dir, SigningKey::from_bytes(&[7u8; 32])).unwrap()
}

fn k(n: u8) -> [u8; 32] {
    [n; 32]
}

fn u(n: u32) -> Principal {
    Principal::Uid(n)
}

fn bind(b: &mut Bindings, j: &mut Journal, p: u32, key: u8) -> Result<(), BindError> {
    b.bind(j, &u(p), &k(key), BindHow::Tofu, BindMeta::default())
}

#[test]
fn bind_check_and_idempotence() {
    let dir = tmpdir();
    let mut j = open(dir.path());
    let mut b = Bindings::default();
    assert_eq!(b.check(&u(501), &k(1)), Check::New);
    bind(&mut b, &mut j, 501, 1).unwrap();
    assert_eq!(b.check(&u(501), &k(1)), Check::Existing);
    let n = j.len();
    bind(&mut b, &mut j, 501, 1).unwrap();
    assert_eq!(j.len(), n, "re-binding the same pair appends nothing");
    assert_eq!(b.key_of(&u(501)), Some(k(1)));
    assert_eq!(b.principal_of(&k(1)), Some(&u(501)));
}

#[test]
fn one_key_per_principal_and_one_principal_per_key() {
    let dir = tmpdir();
    let mut j = open(dir.path());
    let mut b = Bindings::default();
    bind(&mut b, &mut j, 501, 1).unwrap();
    // Same principal, different key.
    assert_eq!(b.check(&u(501), &k(2)), Check::Conflict(ConflictReason::PrincipalHasOtherKey));
    assert!(matches!(bind(&mut b, &mut j, 501, 2), Err(BindError::Conflict(ConflictReason::PrincipalHasOtherKey))));
    // Key reuse by a second uid.
    assert!(matches!(bind(&mut b, &mut j, 502, 1), Err(BindError::Conflict(ConflictReason::KeyBoundToOtherPrincipal))));
    // Uid and sid are different principals.
    assert!(matches!(
        b.bind(&mut j, &Principal::Sid("S-1-5-21".into()), &k(1), BindHow::Tofu, BindMeta::default()),
        Err(BindError::Conflict(ConflictReason::KeyBoundToOtherPrincipal))
    ));
    assert_eq!(j.len(), 1, "refused binds append nothing");
}

#[test]
fn pending_then_approved() {
    let dir = tmpdir();
    let mut j = open(dir.path());
    let mut b = Bindings::default();
    b.bind_pending(&mut j, &u(501), &k(1), BindMeta::default()).unwrap();
    assert_eq!(b.check(&u(501), &k(1)), Check::Pending);
    assert_eq!(b.key_of(&u(501)), None, "pending is not bound");
    b.bind(&mut j, &u(501), &k(1), BindHow::Approved, BindMeta { by: Some(u(0)), ..Default::default() }).unwrap();
    assert_eq!(b.check(&u(501), &k(1)), Check::Existing);
    assert_eq!(b, Bindings::fold(&j).unwrap());
}

#[test]
fn certificate_serials_are_monotone_and_need_a_binding() {
    let dir = tmpdir();
    let mut j = open(dir.path());
    let mut b = Bindings::default();
    assert!(matches!(b.issue_cert(&mut j, &u(501), 10, 20), Err(BindError::NotBound)));
    bind(&mut b, &mut j, 501, 1).unwrap();
    bind(&mut b, &mut j, 502, 2).unwrap();
    assert_eq!(b.issue_cert(&mut j, &u(501), 10, 20).unwrap(), 1);
    assert_eq!(b.issue_cert(&mut j, &u(502), 10, 20).unwrap(), 2);
    assert_eq!(b.issue_cert(&mut j, &u(501), 30, 40).unwrap(), 3);
    assert_eq!(b.serials(&u(501)), vec![1, 3]);
    assert!(matches!(b.issue_cert(&mut j, &u(501), 50, 50), Err(BindError::BadValidity)));
}

#[test]
fn revoke_drops_binding_and_serials_and_bars_the_key() {
    let dir = tmpdir();
    let mut j = open(dir.path());
    let mut b = Bindings::default();
    bind(&mut b, &mut j, 501, 1).unwrap();
    b.issue_cert(&mut j, &u(501), 10, 20).unwrap();
    b.issue_cert(&mut j, &u(501), 15, 25).unwrap();
    b.revoke(&mut j, &u(501), "account removed", &u(0)).unwrap();
    let uid = node_id_from_pubkey(&k(1));
    assert_eq!(b.revoked_serials(), vec![1, 2]);
    assert!(b.is_serial_revoked(&uid, 2) && !b.is_serial_revoked(&uid, 3));
    assert_eq!(b.key_of(&u(501)), None);
    // The revoked key is not reusable, by anyone.
    assert!(matches!(bind(&mut b, &mut j, 501, 1), Err(BindError::Conflict(ConflictReason::KeyRevoked))));
    assert!(matches!(bind(&mut b, &mut j, 502, 1), Err(BindError::Conflict(ConflictReason::KeyRevoked))));
    // A fresh key for the same principal is fine, and serials keep rising.
    assert!(matches!(bind(&mut b, &mut j, 501, 5), Err(BindError::ApprovalRequired)));
    let by = BindMeta { by: Some(u(0)), ..Default::default() };
    b.bind(&mut j, &u(501), &k(5), BindHow::Approved, by).unwrap();
    assert_eq!(b.issue_cert(&mut j, &u(501), 30, 40).unwrap(), 3);
    assert!(matches!(b.revoke(&mut j, &u(777), "x", &u(0)), Err(BindError::NotBound)));
}

#[test]
fn rebind_replaces_key_and_revokes_old_serials() {
    let dir = tmpdir();
    let mut j = open(dir.path());
    let mut b = Bindings::default();
    bind(&mut b, &mut j, 501, 1).unwrap();
    bind(&mut b, &mut j, 502, 2).unwrap();
    b.issue_cert(&mut j, &u(501), 10, 20).unwrap();
    b.issue_cert(&mut j, &u(502), 10, 20).unwrap();

    // Into a key owned by someone else, or to an unbound principal: refused.
    assert!(matches!(b.rebind(&mut j, &u(501), &k(2), BindMeta::default()), Err(BindError::Conflict(ConflictReason::KeyBoundToOtherPrincipal))));
    assert!(matches!(b.rebind(&mut j, &u(900), &k(9), BindMeta::default()), Err(BindError::NotBound)));
    // Rebinding to the current key is a no-op.
    let n = j.len();
    b.rebind(&mut j, &u(501), &k(1), BindMeta::default()).unwrap();
    assert_eq!(j.len(), n);

    b.rebind(&mut j, &u(501), &k(3), BindMeta { by: Some(u(0)), ..Default::default() }).unwrap();
    assert_eq!(b.key_of(&u(501)), Some(k(3)));
    assert_eq!(b.revoked_serials(), vec![1], "only the old key's serials");
    assert_eq!(b.check(&u(501), &k(1)), Check::Conflict(ConflictReason::PrincipalHasOtherKey));
    // Old key can never come back, not even by its former owner via rebind.
    assert!(matches!(b.rebind(&mut j, &u(501), &k(1), BindMeta::default()), Err(BindError::Conflict(ConflictReason::KeyRevoked))));
    assert_eq!(b, Bindings::fold(&j).unwrap());
}

#[test]
fn rebind_cannot_be_requested_through_bind() {
    let dir = tmpdir();
    let mut j = open(dir.path());
    let mut b = Bindings::default();
    let r = b.bind(&mut j, &u(1), &k(1), BindHow::Rebind, BindMeta::default());
    assert!(matches!(r, Err(BindError::InvalidHow)));
}

#[test]
fn fold_is_deterministic_and_equals_incremental_state() {
    let dir = tmpdir();
    let mut j = open(dir.path());
    let mut b = Bindings::default();
    // Deterministic pseudo-random operation stream over 4 principals / 6 keys.
    let mut s: u64 = 0x9e3779b97f4a7c15;
    let mut next = |m: u64| {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (s >> 33) % m
    };
    let mut accepted = 0;
    for _ in 0..300 {
        let p = u(500 + next(4) as u32);
        let key = k(1 + next(60) as u8);
        let r = match next(6) {
            0 | 1 => b.bind(&mut j, &p, &key, BindHow::Tofu, BindMeta::default()),
            2 => b.rebind(&mut j, &p, &key, BindMeta::default()),
            3 => b.issue_cert(&mut j, &p, 10, 20).map(|_| ()),
            4 => b.revoke(&mut j, &p, "test", &u(0)),
            _ => b.bind_pending(&mut j, &p, &key, BindMeta::default()),
        };
        if r.is_ok() {
            accepted += 1;
        }
        assert_eq!(b, Bindings::fold(&j).unwrap(), "incremental state diverged from fold");
    }
    assert!(accepted > 60, "stream too degenerate: {accepted}");
    let f1 = Bindings::fold(&j).unwrap();
    let f2 = Bindings::fold(&j).unwrap();
    assert_eq!(f1, f2);
    drop(j);
    let j = open(dir.path());
    assert_eq!(Bindings::fold(&j).unwrap(), b, "state survives a reopen");
}

/// A temp dir the state-dir safety check accepts (0700).
fn tmpdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    d
}
