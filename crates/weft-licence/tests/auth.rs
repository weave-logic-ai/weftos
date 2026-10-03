mod common;
use common::*;

const BIN: &[u8] = b"\x7fELF fake cog bytes";

#[test]
fn identity_is_unsigned_and_everything_else_needs_a_steward_signature() {
    let h = Harness::new(&[("fall-detect", "arm", BIN)]);
    let id = h.unsigned("GET", "/licence/v1/identity");
    assert_eq!(status(&id), 200);
    let j = id.json_body().unwrap();
    assert_eq!(j["service"], "weft-licence");
    assert_eq!(j["bound"], true);
    assert_eq!(j["device_signing"], "stub");

    for (m, t) in [
        ("POST", "/licence/v1/checkout"),
        ("POST", "/licence/v1/renew"),
        ("GET", "/licence/v1/grants?since=0"),
        ("GET", "/licence/v1/artifact/00"),
    ] {
        let r = h.unsigned(m, t);
        assert!(matches!(status(&r), 400 | 401), "{m} {t} -> {}", status(&r));
    }
    // A signature by any other key is refused before any work is done.
    let forged = h.signed_with(&sk(99), "GET", "/licence/v1/grants?since=0", b"", h.now() * 1000);
    let r = h.svc.handle(&forged);
    assert_eq!((status(&r), code(&r).as_str()), (401, "bad_signature"));
    assert_eq!(h.fetcher.fetch_count(), 0);
    // The real steward is accepted.
    assert_eq!(status(&h.call("GET", "/licence/v1/grants?since=0", b"")), 200);
}

#[test]
fn replays_stale_and_tampered_requests_are_refused() {
    let h = Harness::new(&[]);
    let req = h.signed("GET", "/licence/v1/grants?since=0", b"");
    assert_eq!(status(&h.svc.handle(&req)), 200);
    let again = h.svc.handle(&req);
    assert_eq!((status(&again), code(&again).as_str()), (401, "replayed"));
    // Replay protection survives a restart (the nonce list is persisted).
    let h = h.reopen();
    let r = h.svc.handle(&req);
    assert_eq!(code(&r), "replayed");
    // Stale timestamp.
    let old = h.signed_with(&steward(), "GET", "/licence/v1/grants?since=0", b"", (h.now() - 1000) * 1000);
    assert_eq!(code(&h.svc.handle(&old)), "stale_request");
    // The target is signed: changing the query breaks the signature.
    let mut t = h.signed("GET", "/licence/v1/grants?since=0", b"");
    t.target = "/licence/v1/grants?since=5".into();
    assert_eq!(code(&h.svc.handle(&t)), "bad_signature");
    // The body is signed too.
    let mut b = h.signed("POST", "/licence/v1/renew", b"{}");
    b.body = b"{\"release\":[]}".to_vec();
    assert_eq!(code(&h.svc.handle(&b)), "bad_signature");
    // Wrong node id.
    let mut w = h.signed("GET", "/licence/v1/grants?since=0", b"");
    w.headers.insert("x-licence-node".into(), "someone-else".into());
    assert_eq!(code(&h.svc.handle(&w)), "wrong_node");
}

#[test]
fn forged_traffic_cannot_use_up_the_stewards_budget() {
    let h = Harness::new(&[]);
    // 60 forged requests in one minute: the first 30 are refused as
    // bad_signature, the rest hit the shared unsigned pool.
    let mut pool_hit = 0;
    for _ in 0..60 {
        let f = h.signed_with(&sk(99), "GET", "/licence/v1/grants?since=0", b"", h.now() * 1000);
        if code(&h.svc.handle(&f)) == "rate_limited_unsigned" {
            pool_hit += 1;
        }
    }
    assert_eq!(pool_hit, 30);
    // Identity shares the exhausted unsigned pool.
    assert_eq!(status(&h.unsigned("GET", "/licence/v1/identity")), 429);
    // The genuine steward still gets its full 10 requests a minute...
    for i in 0..10 {
        assert_eq!(status(&h.call("GET", "/licence/v1/grants?since=0", b"")), 200, "request {i}");
    }
    // ...and then its own budget (not the unsigned pool) runs out.
    let r = h.call("GET", "/licence/v1/grants?since=0", b"");
    assert_eq!((status(&r), code(&r).as_str()), (429, "rate_limited"));
    // A minute later both pools are fresh.
    h.advance(61);
    assert_eq!(status(&h.unsigned("GET", "/licence/v1/identity")), 200);
    assert_eq!(status(&h.call("GET", "/licence/v1/grants?since=0", b"")), 200);
}

#[test]
fn an_unbound_seed_verifies_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = base_config(dir.path());
    weft_licence::keys::init(&cfg.state_dir).unwrap();
    let h = Harness::open_dir(dir, cfg, StubLicence::all(None), StubFetcher::with(&[]), T0);
    let r = h.call("GET", "/licence/v1/grants?since=0", b"");
    assert_eq!((status(&r), code(&r).as_str()), (409, "seed_not_bound"));
    assert_eq!(h.unsigned("GET", "/licence/v1/identity").json_body().unwrap()["bound"], false);
}

#[test]
fn clock_below_the_floor_refuses_to_verify_or_sign() {
    let h = Harness::new(&[("fall-detect", "arm", BIN)]);
    assert_eq!(status(&h.checkout("fall-detect", "arm")), 200);
    // Unset clock (no RTC): before the build-time floor.
    h.clock.store(5_000, std::sync::atomic::Ordering::SeqCst);
    let r = h.checkout("fall-detect", "arm");
    assert_eq!((status(&r), code(&r).as_str()), (503, "clock_not_set"));
    assert_eq!(h.unsigned("GET", "/licence/v1/identity").json_body().unwrap()["clock_ok"], false);
    // Above the floor but behind the last issued_at: also refused.
    h.clock.store(T0 - 10, std::sync::atomic::Ordering::SeqCst);
    let r = h.call("POST", "/licence/v1/renew", b"");
    assert_eq!(code(&r), "clock_not_set");
    // Caught up: works again.
    h.clock.store(T0 + 5, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(status(&h.call("POST", "/licence/v1/renew", b"")), 200);
}

#[test]
fn a_signing_attempt_before_the_floor_is_refused_even_after_verification() {
    // The clock passes the floor for the request and drops below it by the
    // time of signing (the fetch is slow): the grant is not signed.
    let h = Harness::with(&[("fall-detect", "arm", BIN)], |c| c.clock_floor = T0 + 100);
    h.clock.store(T0 + 200, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(status(&h.checkout("fall-detect", "arm")), 200);
    h.clock.store(T0 + 50, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(code(&h.checkout("fall-detect", "arm")), "clock_not_set");
}

#[test]
fn the_request_layout_and_clock_floor_match_the_bridge() {
    // COG-011 bridge: CLOCK_FLOOR_MS = 1_780_000_000_000 (2026-05-28), ms timestamps,
    // alphanumeric 16-64 nonces, one field per line.
    assert_eq!(weft_licence::CLOCK_FLOOR * 1000, 1_780_000_000_000);
    let s = weft_licence::request::signing_string("POST", "/p", "n", 1_791_000_000_123, "abcdef0123456789", b"x");
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(&lines[..6], ["weft-licence-v1/request", "POST", "/p", "n", "1791000000123", "abcdef0123456789"]);
    assert_eq!(lines[6], weft_licence_wire::sha256_hex(b"x"));
    let h = Harness::new(&[]);
    // A non-hex alphanumeric nonce is accepted; a seconds timestamp is stale.
    let nonce = "ZZzz0123456789ab";
    let hdr = weft_licence::request::sign_request(&steward(), NODE, "GET", "/licence/v1/grants?since=0", b"", h.now() * 1000, nonce);
    let req = weft_licence::request::Request { method: "GET".into(), target: "/licence/v1/grants?since=0".into(), headers: hdr, body: vec![] };
    assert_eq!(status(&h.svc.handle(&req)), 200);
    let secs = h.signed_with(&steward(), "GET", "/licence/v1/grants?since=0", b"", h.now());
    assert_eq!(code(&h.svc.handle(&secs)), "stale_request");
}
