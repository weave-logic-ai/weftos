mod common;
use common::*;
use std::sync::atomic::Ordering;
use weft_licence::providers::{Entitlement, LicenceCheckError};
use weft_licence_wire::{sha256_hex, verify_grant, verify_grant_signature};

const A: &[u8] = b"\x7fELF fall-detect arm build";
const B: &[u8] = b"\x7fELF fall-detect arm64 build";

fn state_slots(h: &Harness) -> serde_json::Value {
    let raw = std::fs::read(h.cfg.state_dir.join("slots.json")).unwrap();
    serde_json::from_slice(&raw).unwrap()
}

#[test]
fn checkout_returns_a_grant_that_verifies_under_the_1a_verifier() {
    let h = Harness::new(&[("fall-detect", "arm", A)]);
    let r = h.checkout("fall-detect", "arm");
    assert_eq!(status(&r), 200, "{:?}", r.json_body());
    let signed = grant_of(&r);
    let g = verify_grant(&signed, &h.grant_key(), &mesh()).expect("1a verifier accepts it");
    assert_eq!((g.cog_id.as_str(), g.version.as_str(), g.seq), ("fall-detect", "1.0.0", 1));
    assert_eq!(g.artifacts.len(), 1);
    let a = &g.artifacts[0];
    assert_eq!(a.sha256, sha256_hex(A));
    assert_eq!(a.blake3, weft_licence_wire::hex_encode(blake3::hash(A).as_bytes()));
    assert_eq!(g.expires_at - g.issued_at, 72 * 3600);
    // The mesh check is the verifier's: another mesh refuses it.
    assert!(verify_grant(&signed, &h.grant_key(), &other_mesh()).is_err());
    // The licence is a hash, never the account label.
    assert!(!signed.payload.contains("acct"));
    assert_eq!(g.licence.ref_sha256, sha256_hex(b"acct"));
}

#[test]
fn the_seq_is_on_disk_before_the_grant_is_released() {
    let h = Harness::new(&[("fall-detect", "arm", A)]);
    let dir = h.cfg.state_dir.clone();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = seen.clone();
    h.svc.set_release_hook(Box::new(move |grant| {
        let g: weft_licence_wire::CheckoutGrant = serde_json::from_str(&grant.payload).unwrap();
        let disk: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("slots.json")).unwrap()).unwrap();
        s2.lock().unwrap().push((g.seq, disk["slots"]["fall-detect@1.0.0"]["seq"].as_u64().unwrap()));
    }));
    assert_eq!(status(&h.checkout("fall-detect", "arm")), 200);
    assert_eq!(status(&h.call("POST", "/licence/v1/renew", b"")), 200);
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen, vec![(1, 1), (2, 2)], "disk seq == released seq at release time");
}

#[test]
fn a_crash_before_the_write_releases_nothing_and_a_crash_after_never_reuses_a_seq() {
    let h = Harness::new(&[("fall-detect", "arm", A)]);
    let released = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let r2 = released.clone();
    h.svc.set_release_hook(Box::new(move |_| {
        r2.fetch_add(1, Ordering::SeqCst);
    }));
    // Crash before persisting: no grant leaves the service.
    h.svc.inject_persist_failure(true);
    let r = h.checkout("fall-detect", "arm");
    assert_eq!((status(&r), code(&r).as_str()), (503, "persist_failed"));
    assert_eq!(released.load(Ordering::SeqCst), 0);
    assert!(r.json_body().unwrap().get("grant").is_none());
    h.svc.inject_persist_failure(false);
    // The failed attempt did not advance the in-memory table either.
    assert_eq!(grant_seq(&h.checkout("fall-detect", "arm")), 1);
    // Crash after persisting and releasing: restart. The next grant is above.
    let h = h.reopen();
    assert_eq!(state_slots(&h)["slots"]["fall-detect@1.0.0"]["seq"], 1);
    let renewed = h.call("POST", "/licence/v1/renew", b"");
    let grants = renewed.json_body().unwrap()["grants"].clone();
    let g: weft_licence_wire::CheckoutGrant =
        serde_json::from_str(grants[0]["payload"].as_str().unwrap()).unwrap();
    assert_eq!(g.seq, 2);
}

fn grant_seq(r: &weft_licence::Response) -> u64 {
    let g: weft_licence_wire::CheckoutGrant = serde_json::from_str(&grant_of(r).payload).unwrap();
    g.seq
}

#[test]
fn the_union_of_arches_applies_across_checkouts() {
    let h = Harness::new(&[("fall-detect", "arm", A), ("fall-detect", "arm64", B)]);
    let g1 = grant_of(&h.checkout("fall-detect", "arm"));
    let g2 = grant_of(&h.checkout("fall-detect", "arm64"));
    let (c1, c2): (weft_licence_wire::CheckoutGrant, weft_licence_wire::CheckoutGrant) =
        (serde_json::from_str(&g1.payload).unwrap(), serde_json::from_str(&g2.payload).unwrap());
    assert_eq!(c1.arches().into_iter().collect::<Vec<_>>(), vec!["arm"]);
    assert_eq!(c2.arches().into_iter().collect::<Vec<_>>(), vec!["arm", "arm64"]);
    assert_eq!((c1.seq, c2.seq), (1, 2));
    verify_grant(&g2, &h.grant_key(), &mesh()).unwrap();
    // Asking again for an arch already held returns the held grant: no new
    // seq, no fetch.
    let fetched = h.fetcher.fetch_count();
    let g3 = grant_of(&h.checkout("fall-detect", "arm"));
    assert_eq!(g3.payload, g2.payload);
    assert_eq!(h.fetcher.fetch_count(), fetched);
    // A registry that republishes the same version with new bytes is refused:
    // a newer grant may not change an arch it already carried.
    h.fetcher.cogs.lock().unwrap().insert(("fall-detect".into(), "arm".into()), b"different".to_vec());
    assert_eq!(code(&h.checkout("fall-detect", "arm")), "artifact_changed");
}

#[test]
fn unlicensed_expired_and_unknown_requests_are_refused_before_any_download() {
    let h = Harness::new(&[("fall-detect", "arm", A)]);
    h.licence.set("*", Err(LicenceCheckError::Unlicensed));
    assert_eq!(code(&h.checkout("fall-detect", "arm")), "cog_unlicensed");
    h.licence.set("*", Err(LicenceCheckError::Expired));
    assert_eq!(code(&h.checkout("fall-detect", "arm")), "licence_expired");
    assert_eq!(h.fetcher.fetch_count(), 0);
    h.licence.set("*", Ok(Entitlement { ref_sha256: sha256_hex(b"x"), expires: None }));
    assert_eq!(code(&h.checkout("no-such-cog", "arm")), "cog_not_found");
    assert_eq!(code(&h.checkout("fall-detect", "riscv")), "arch_unavailable");
    let bad = h.call("POST", "/licence/v1/checkout", b"{\"cog_id\":\"../etc\",\"version\":\"latest\",\"arch\":\"arm\"}");
    assert_eq!(code(&bad), "bad_request");
}

#[test]
fn a_grant_never_outlives_the_licence() {
    let h = Harness::new(&[("fall-detect", "arm", A)]);
    h.licence.set("*", Ok(Entitlement { ref_sha256: sha256_hex(b"x"), expires: Some(T0 + 3600) }));
    let g: weft_licence_wire::CheckoutGrant = serde_json::from_str(&grant_of(&h.checkout("fall-detect", "arm")).payload).unwrap();
    assert_eq!(g.expires_at, T0 + 3600);
    assert_eq!(g.licence.expires, T0 + 3600);
}

#[test]
fn renewal_extends_a_lapse_withdraws_and_release_withdraws() {
    let h = Harness::new(&[("fall-detect", "arm", A), ("other", "arm", b"\x7fELF other")]);
    h.checkout("fall-detect", "arm");
    h.checkout("other", "arm");
    h.advance(12 * 3600);
    let r = h.call("POST", "/licence/v1/renew", b"");
    let grants: Vec<weft_licence_wire::SignedGrant> = serde_json::from_value(r.json_body().unwrap()["grants"].clone()).unwrap();
    assert_eq!(grants.len(), 2);
    for s in &grants {
        let g = verify_grant(s, &h.grant_key(), &mesh()).unwrap();
        assert_eq!(g.seq, 2);
        assert_eq!(g.expires_at, h.now() + 72 * 3600);
        assert!(!g.is_withdrawal());
    }
    // Release one: a withdrawal (expires_at <= issued_at) that keeps its arches.
    let rel = serde_json::json!({"release": [{"cog_id": "other", "version": "1.0.0"}]});
    let r = h.call("POST", "/licence/v1/renew", rel.to_string().as_bytes());
    let out: Vec<weft_licence_wire::SignedGrant> = serde_json::from_value(r.json_body().unwrap()["grants"].clone()).unwrap();
    let w: Vec<_> = out.iter().map(|s| verify_grant_signature(s, &h.grant_key()).unwrap()).collect();
    let withdrawn = w.iter().find(|g| g.cog_id == "other").unwrap();
    assert!(withdrawn.is_withdrawal() && withdrawn.seq == 3 && withdrawn.artifacts.len() == 1);
    // The licence stops covering fall-detect: the next pull withdraws it.
    h.licence.set("fall-detect", Err(LicenceCheckError::Expired));
    let r = h.call("POST", "/licence/v1/renew", b"");
    let out: Vec<weft_licence_wire::SignedGrant> = serde_json::from_value(r.json_body().unwrap()["grants"].clone()).unwrap();
    assert_eq!(out.len(), 1, "the released slot no longer renews");
    assert!(verify_grant_signature(&out[0], &h.grant_key()).unwrap().is_withdrawal());
    // GET /grants lists the latest grant per slot, with a cursor.
    let all = h.call("GET", "/licence/v1/grants?since=0", b"").json_body().unwrap();
    assert_eq!(all["grants"].as_array().unwrap().len(), 2);
    let next = all["next"].as_u64().unwrap();
    h.advance(1);
    let none = h.call("GET", &format!("/licence/v1/grants?since={next}"), b"").json_body().unwrap();
    assert!(none["grants"].as_array().unwrap().is_empty());
}

#[test]
fn state_files_are_never_group_or_world_accessible() {
    let h = Harness::new(&[("fall-detect", "arm", A)]);
    h.checkout("fall-detect", "arm");
    fn walk(p: &std::path::Path, bad: &mut Vec<String>) {
        use std::os::unix::fs::PermissionsExt;
        for e in std::fs::read_dir(p).unwrap().flatten() {
            let m = e.metadata().unwrap().permissions().mode() & 0o777;
            if m & 0o077 != 0 {
                bad.push(format!("{} {:o}", e.path().display(), m));
            }
            if e.metadata().unwrap().is_dir() {
                walk(&e.path(), bad);
            }
        }
    }
    let mut bad = Vec::new();
    walk(&h.cfg.state_dir, &mut bad);
    assert!(bad.is_empty(), "{bad:?}");
}
