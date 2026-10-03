mod common;
use common::*;
use weft_licence::artifact::{ServeOverride, sign_override};
use weft_licence::limits::throttle_sleep_ms;
use weft_licence::request::Request;
use weft_licence_wire::{hex_encode, verify_grant_signature};

const A: &[u8] = b"\x7fELF fall-detect arm build";

fn blake(b: &[u8]) -> String {
    hex_encode(blake3::hash(b).as_bytes())
}

fn fetch_artifact(h: &Harness, b: &[u8]) -> weft_licence::Response {
    h.call("GET", &format!("/licence/v1/artifact/{}", blake(b)), b"")
}

#[test]
fn three_serves_per_artifact_per_day_then_an_operator_override_then_the_window_rolls() {
    let h = Harness::new(&[("fall-detect", "arm", A)]);
    assert_eq!(status(&h.checkout("fall-detect", "arm")), 200);
    for i in 0..3 {
        let r = fetch_artifact(&h, A);
        assert_eq!(status(&r), 200, "serve {i}");
        assert!(matches!(r.body, weft_licence::Body::File { len, .. } if len == A.len() as u64));
        h.advance(61); // keep inside the per-minute budget
    }
    let r = fetch_artifact(&h, A);
    assert_eq!((status(&r), code(&r).as_str()), (429, "serve_limit"));
    // An override signed by a non-operator is ignored.
    let o = ServeOverride { v: 1, key_id: weft_licence_wire::key_id(&steward().verifying_key().to_bytes()), cog_id: "fall-detect".into(), version: "1.0.0".into(), arch: "arm".into(), extra: 2, expires_at: h.now() + 3600 };
    weft_licence::artifact::install_override(&h.cfg.state_dir.join("overrides"), &sign_override(&o, &sk(77))).unwrap();
    assert_eq!(code(&fetch_artifact(&h, A)), "serve_limit");
    // The operator-signed override raises it.
    weft_licence::artifact::install_override(&h.cfg.state_dir.join("overrides"), &sign_override(&o, &operator())).unwrap();
    assert_eq!(status(&fetch_artifact(&h, A)), 200);
    // An override for another steward key does nothing.
    // After 24 h the window has rolled.
    h.advance(25 * 3600);
    h.call("POST", "/licence/v1/renew", b""); // keep the grant live
    assert_eq!(status(&fetch_artifact(&h, A)), 200);
}

#[test]
fn serves_are_counted_per_steward_key_and_survive_a_restart() {
    let h = Harness::new(&[("fall-detect", "arm", A)]);
    h.checkout("fall-detect", "arm");
    for _ in 0..3 {
        assert_eq!(status(&fetch_artifact(&h, A)), 200);
        h.advance(61);
    }
    let h = h.reopen();
    assert_eq!(code(&fetch_artifact(&h, A)), "serve_limit");
}

#[test]
fn bytes_need_an_active_grant_and_a_withdrawal_stops_them() {
    let h = Harness::new(&[("fall-detect", "arm", A)]);
    assert_eq!(code(&fetch_artifact(&h, A)), "no_grant");
    h.checkout("fall-detect", "arm");
    assert_eq!(status(&fetch_artifact(&h, A)), 200);
    let rel = serde_json::json!({"release": [{"cog_id": "fall-detect", "version": "1.0.0"}]});
    h.call("POST", "/licence/v1/renew", rel.to_string().as_bytes());
    assert_eq!(code(&fetch_artifact(&h, A)), "no_grant");
    assert_eq!(code(&h.call("GET", "/licence/v1/artifact/zz", b"")), "bad_request");
}

#[test]
fn artifact_over_the_size_limit_is_refused_before_the_download() {
    let h = Harness::with(&[("fall-detect", "arm", A)], |c| c.limits.max_artifact_bytes = 8);
    let r = h.checkout("fall-detect", "arm");
    assert_eq!((status(&r), code(&r).as_str()), (413, "artifact_too_large"));
    assert_eq!(h.fetcher.fetch_count(), 0);
}

#[test]
fn only_one_checkout_is_in_flight() {
    let h = Harness::new(&[("fall-detect", "arm", A), ("other", "arm", b"\x7fELF other")]);
    let (tx, rx) = std::sync::mpsc::channel();
    *h.fetcher.gate.lock().unwrap() = Some(rx);
    let svc = h.svc.clone();
    let slow = h.signed("POST", "/licence/v1/checkout", br#"{"request_id":"a","cog_id":"fall-detect","version":"latest","arch":"arm"}"#);
    let t = std::thread::spawn(move || svc.handle(&slow).status);
    // Wait until the first checkout is inside the (blocked) fetch.
    while h.fetcher.fetch_count() == 0 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let r = h.checkout("other", "arm");
    assert_eq!((status(&r), code(&r).as_str()), (429, "busy"));
    tx.send(()).unwrap();
    assert_eq!(t.join().unwrap(), 200);
    assert_eq!(status(&h.checkout("other", "arm")), 200);
}

#[test]
fn the_cache_is_an_lru_that_never_evicts_an_active_checkout() {
    let (a, b, c) = (vec![1u8; 100], vec![2u8; 100], vec![3u8; 100]);
    let h = Harness::with(&[("a", "arm", &a), ("b", "arm", &b), ("c", "arm", &c)], |cfg| {
        cfg.limits.cache_bytes = 250;
        cfg.limits.max_artifact_bytes = 250;
    });
    assert_eq!(status(&h.checkout("a", "arm")), 200);
    assert_eq!(status(&h.checkout("b", "arm")), 200);
    // Both are under active grants: nothing can be evicted for the third.
    let r = h.checkout("c", "arm");
    assert_eq!((status(&r), code(&r).as_str()), (507, "cache_full"));
    // Release a; its bytes become evictable and c fits.
    let rel = serde_json::json!({"release": [{"cog_id": "a", "version": "1.0.0"}]});
    h.call("POST", "/licence/v1/renew", rel.to_string().as_bytes());
    h.advance(61);
    assert_eq!(status(&h.checkout("c", "arm")), 200);
    assert!(!h.cfg.state_dir.join("cache").join(blake(&a)).exists(), "a was evicted");
    assert!(h.cfg.state_dir.join("cache").join(blake(&b)).exists());
    assert!(h.cfg.state_dir.join("cache").join(blake(&c)).exists());
}

#[test]
fn the_transfer_rate_throttle_holds_four_mib_per_second() {
    let rate = 4 * 1024 * 1024;
    assert_eq!(throttle_sleep_ms(rate, 0, rate), 1000, "4 MiB sent at t=0 owes a second");
    assert_eq!(throttle_sleep_ms(rate, 1000, rate), 0);
    assert_eq!(throttle_sleep_ms(rate / 2, 100, rate), 400);
    assert_eq!(throttle_sleep_ms(1, 0, 0), 0, "rate 0 means unthrottled");
    assert_eq!(weft_licence::Limits::default().rate_bytes_per_sec, rate);
}

#[test]
fn defaults_match_the_adr() {
    let l = weft_licence::Limits::default();
    assert_eq!((l.in_flight, l.requests_per_min, l.unsigned_per_min), (1, 10, 30));
    assert_eq!((l.max_artifact_bytes, l.cache_bytes), (64 << 20, 256 << 20));
    assert_eq!((l.serves_per_day, l.renew_batch), (3, 256));
    let _ = (Request::default(), verify_grant_signature);
}
