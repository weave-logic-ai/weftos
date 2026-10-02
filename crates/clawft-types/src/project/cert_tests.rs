//! Certificate and anchor tests. The golden vectors were produced by an
//! independent implementation (Python `cryptography` with sorted-key
//! `json.dumps(separators=(",", ":"))`); other packages can match them.

use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signer, SigningKey};

use super::canon::{canonical_json, hex_decode, hex_encode};
use super::cert::*;

const PID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
const USER_PUB: &str = "8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c";
const PROJ_PUB: &str = "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394";
const PROJ_KID: &str = "6a3803d5f059902a1c6dafbc9ba47292";
const CERT_CANON: &str = r#"{"expires_at":null,"issued_at":"2026-10-01T09:30:00Z","project_id":"01JB8Z3Q0V6X9KQ4M2N7T5R1WD","project_key_id":"6a3803d5f059902a1c6dafbc9ba47292","project_pubkey":"8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394","serial":1,"type":"project-cert","user_key_id":"34750f98bd59fcfc946da45aaabe933b","user_pubkey":"8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c","v":1}"#;
const CERT_SIG: &str = "081ef31a626b34369d2da9df686d53f2ed1f96e9fc78ab5c554e5921b96bdf7e77d612e6196a0c60ac69c952e0a4d91e2ed1b8d70224274fb53479d8160cb701";
const ANCHOR_CANON: &str = r#"{"at":"2026-10-01T10:00:00Z","cert_serial":1,"chain_id":0,"head_hash":"abababababababababababababababababababababababababababababababab","head_seq":4210,"prev_anchor":null,"project_id":"01JB8Z3Q0V6X9KQ4M2N7T5R1WD","project_key_id":"6a3803d5f059902a1c6dafbc9ba47292","rule_hash":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd","seq":17}"#;
const ANCHOR_SIG: &str = "6e390a39c81f8b60753abbd1e494dcae41acf4b056a9eab0eb387656795fd39b81d9a36409b63b977ac14cc43efe7df94e79a62e1f2500171710a238e788900d";
const ANCHOR_HASH: &str = "4b97b9304b53143e0772be7ea09bcb03733f9e136edc8b95ea6b83c4b0e35dc4";
const POP_NONCE: &str = "00112233445566778899aabbccddeeff";
const POP_SIG_REKEY: &str = "c09c3915e40139316c6a753d1857272c9466f816ef3e80d7a639a55d6072240860d0d97946edd571ee567e71af164ae93d51934841f2f95ae9d27375763f5c0f";
const USER_KID: &str = "34750f98bd59fcfc946da45aaabe933b";
const POP_SIG: &str = "a091f8884ef78b01ea1caac70b8c01b8e5e8b9cef8ac162321c1450596279d72426a8d4b86f94e9c13d38f944f9d291826e931fb9286999bc0c19a33466be10f";

fn user_key() -> SigningKey {
    SigningKey::from_bytes(&[1u8; 32])
}
fn proj_key() -> SigningKey {
    SigningKey::from_bytes(&[2u8; 32])
}
fn t(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}
fn now() -> DateTime<Utc> {
    t("2026-10-02T00:00:00Z")
}
fn user_pk() -> [u8; 32] {
    user_key().verifying_key().to_bytes()
}
fn req() -> CertRequest {
    CertRequest {
        project_id: PID.into(),
        project_pubkey: proj_key().verifying_key().to_bytes(),
        serial: 1,
        issued_at: t("2026-10-01T09:30:00Z"),
        expires_at: None,
    }
}
fn cert() -> ProjectCert {
    ProjectCert::sign(&user_key(), &req())
}
fn anchor() -> ProjectAnchorStmt {
    ProjectAnchorStmt {
        project_id: PID.into(),
        project_key_id: String::new(),
        cert_serial: 1,
        seq: 17,
        chain_id: 0,
        head_hash: "ab".repeat(32),
        head_seq: 4210,
        rule_hash: "cd".repeat(32),
        at: "2026-10-01T10:00:00Z".into(),
        prev_anchor: None,
        sig: String::new(),
    }
    .sign(&proj_key())
}

#[test]
fn golden_keys_and_ids() {
    assert_eq!(hex_encode(&user_pk()), USER_PUB);
    assert_eq!(hex_encode(&proj_key().verifying_key().to_bytes()), PROJ_PUB);
    assert_eq!(key_id(&proj_key().verifying_key().to_bytes()), PROJ_KID);
}

#[test]
fn cert_golden_vector() {
    let c = cert();
    assert_eq!(c.sig, CERT_SIG);
    let expect = format!("{CERT_DOMAIN}{CERT_CANON}");
    assert_eq!(c.canonical_bytes(), expect.as_bytes());
    assert_eq!(c.computed_key_id().unwrap(), PROJ_KID);
}

#[test]
fn cert_round_trips_through_json_and_verifies() {
    let c = cert();
    let json = serde_json::to_string(&c).unwrap();
    assert!(json.contains(r#""type":"project-cert""#));
    assert!(json.contains(r#""expires_at":null"#));
    let back: ProjectCert = serde_json::from_str(&json).unwrap();
    assert_eq!(back, c);
    back.verify(&user_pk(), now()).unwrap();
}

#[test]
fn cert_parses_the_plan_example_shape() {
    // `expires_at` may be absent in a hand-written file; the field is Option.
    let mut v = serde_json::to_value(cert()).unwrap();
    v.as_object_mut().unwrap().remove("expires_at");
    let c: ProjectCert = serde_json::from_value(v).unwrap();
    c.verify(&user_pk(), now()).unwrap();
}

#[test]
fn issuing_twice_is_byte_identical() {
    let (a, b) = (cert(), cert());
    assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    assert_eq!(a.sig, b.sig);
}

#[test]
fn tampering_each_field_fails() {
    let flip = |s: &str| {
        let mut b = s.as_bytes().to_vec();
        b[0] = if b[0] == b'0' { b'1' } else { b'0' };
        String::from_utf8(b).unwrap()
    };
    let cases: Vec<(&str, Box<dyn Fn(&mut ProjectCert)>)> = vec![
        ("v", Box::new(|c| c.v = 2)),
        ("type", Box::new(|c| c.kind = "other".into())),
        ("project_id", Box::new(|c| c.project_id = "01JB8Z3Q0V6X9KQ4M2N7T5R1WE".into())),
        ("project_pubkey", Box::new(move |c| c.project_pubkey = flip(&c.project_pubkey))),
        ("project_key_id", Box::new(move |c| c.project_key_id = flip(&c.project_key_id))),
        ("user_key_id", Box::new(move |c| c.user_key_id = flip(&c.user_key_id))),
        ("user_pubkey", Box::new(move |c| c.user_pubkey = flip(&c.user_pubkey))),
        ("serial", Box::new(|c| c.serial = 2)),
        ("issued_at", Box::new(|c| c.issued_at = "2026-10-01T09:30:01Z".into())),
        ("expires_at", Box::new(|c| c.expires_at = Some("2099-01-01T00:00:00Z".into()))),
        ("sig", Box::new(move |c| c.sig = flip(&c.sig))),
    ];
    for (name, tamper) in cases {
        let mut c = cert();
        tamper(&mut c);
        assert!(c.verify(&user_pk(), now()).is_err(), "tampered `{name}` verified");
    }
}

#[test]
fn error_kinds_are_specific() {
    let mut c = cert();
    c.project_key_id = "0".repeat(32);
    assert_eq!(
        c.verify(&user_pk(), now()),
        Err(CertError::KeyIdMismatch("project_key_id"))
    );
    let other = SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes();
    assert_eq!(cert().verify(&other, now()), Err(CertError::UntrustedUser));
    let mut c = cert();
    c.sig = "00".repeat(64);
    assert_eq!(c.verify(&user_pk(), now()), Err(CertError::BadSignature));
    let mut c = cert();
    c.sig = "zz".into();
    assert_eq!(c.verify(&user_pk(), now()), Err(CertError::BadHex("sig")));
    let mut c = cert();
    c.v = 7;
    assert_eq!(c.verify(&user_pk(), now()), Err(CertError::BadVersion(7)));
}

#[test]
fn cert_signed_by_the_wrong_user_key_fails() {
    let imposter = SigningKey::from_bytes(&[9u8; 32]);
    let c = ProjectCert::sign(&imposter, &req());
    // Internally consistent, but not the key the verifier trusts.
    assert_eq!(c.verify(&user_pk(), now()), Err(CertError::UntrustedUser));
    c.verify(&imposter.verifying_key().to_bytes(), now()).unwrap();
}

#[test]
fn expiry_is_honoured() {
    let mut r = req();
    r.expires_at = Some(now() + Duration::hours(1));
    let c = ProjectCert::sign(&user_key(), &r);
    c.verify(&user_pk(), now()).unwrap();
    assert_eq!(
        c.verify(&user_pk(), now() + Duration::hours(2)),
        Err(CertError::Expired)
    );
    assert_eq!(
        c.verify(&user_pk(), now() + Duration::hours(1)),
        Err(CertError::Expired)
    );
}

#[test]
fn domain_separation() {
    let c = cert();
    // The same JSON without the domain tag was never signed.
    let bare = CERT_CANON.as_bytes();
    let sig = ed25519_dalek::Signature::from_bytes(&hex_decode::<64>(&c.sig).unwrap());
    assert!(user_key().verifying_key().verify_strict(bare, &sig).is_err());
    // A signature over another domain's tag does not verify as a cert.
    let mut forged = c.clone();
    let wrong = format!("{ANCHOR_DOMAIN}{CERT_CANON}");
    forged.sig = hex_encode(&user_key().sign(wrong.as_bytes()).to_bytes());
    assert_eq!(forged.verify(&user_pk(), now()), Err(CertError::BadSignature));
    // A cert signature cannot be replayed as an anchor signature and back.
    let mut a = anchor();
    a.sig = c.sig.clone();
    assert!(a.verify(&proj_key().verifying_key().to_bytes()).is_err());
    // The three domain tags are distinct.
    assert_ne!(CERT_DOMAIN, ANCHOR_DOMAIN);
    assert_ne!(ANCHOR_DOMAIN, POP_DOMAIN);
    assert_ne!(CERT_DOMAIN, POP_DOMAIN);
}

#[test]
fn anchor_golden_vector() {
    let a = anchor();
    assert_eq!(a.project_key_id, PROJ_KID);
    assert_eq!(a.sig, ANCHOR_SIG);
    let expect = format!("{ANCHOR_DOMAIN}{ANCHOR_CANON}");
    assert_eq!(a.canonical_bytes(), expect.as_bytes());
    assert_eq!(a.hash(), ANCHOR_HASH);
    a.verify(&proj_key().verifying_key().to_bytes()).unwrap();
}

#[test]
fn anchor_round_trip_and_tamper() {
    let a = anchor();
    let back: ProjectAnchorStmt = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
    assert_eq!(back, a);
    let pk = proj_key().verifying_key().to_bytes();
    let cases: Vec<Box<dyn Fn(&mut ProjectAnchorStmt)>> = vec![
        Box::new(|a| a.seq = 18),
        Box::new(|a| a.head_seq = 4211),
        Box::new(|a| a.head_hash = "ac".repeat(32)),
        Box::new(|a| a.rule_hash = "ce".repeat(32)),
        Box::new(|a| a.at = "2026-10-01T10:00:01Z".into()),
        Box::new(|a| a.prev_anchor = Some("00".repeat(32))),
        Box::new(|a| a.cert_serial = 2),
        Box::new(|a| a.chain_id = 1),
        Box::new(|a| a.project_id = "01JB8Z3Q0V6X9KQ4M2N7T5R1WE".into()),
    ];
    for (i, tamper) in cases.iter().enumerate() {
        let mut a = anchor();
        tamper(&mut a);
        assert!(a.verify(&pk).is_err(), "tamper case {i} verified");
    }
    // Wrong key.
    let other = SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes();
    assert_eq!(
        a.verify(&other),
        Err(CertError::KeyIdMismatch("project_key_id"))
    );
}

#[test]
fn anchor_hash_covers_the_signature_and_chains() {
    let a = anchor();
    let mut b = a.clone();
    b.sig = "00".repeat(64);
    assert_ne!(a.hash(), b.hash());
    let next = ProjectAnchorStmt {
        seq: 18,
        prev_anchor: Some(a.hash()),
        ..a.clone()
    }
    .sign(&proj_key());
    next.verify(&proj_key().verifying_key().to_bytes()).unwrap();
    assert_eq!(next.prev_anchor.as_deref(), Some(ANCHOR_HASH));
}

#[test]
fn pop_bytes_and_golden_signature() {
    let bytes = pop_signed_bytes(PopOp::Register, USER_KID, POP_NONCE, PID).unwrap();
    assert_eq!(
        bytes,
        format!("weftos-mesh-local-pop-v2\nregister\n{USER_KID}\n{POP_NONCE}\n{PID}").into_bytes()
    );
    assert_eq!(hex_encode(&proj_key().sign(&bytes).to_bytes()), POP_SIG);
    let rk = pop_signed_bytes(PopOp::Rekey, USER_KID, POP_NONCE, PID).unwrap();
    assert_eq!(hex_encode(&proj_key().sign(&rk).to_bytes()), POP_SIG_REKEY);
    // Binding: op, user chain, nonce and project id each change the bytes.
    assert_ne!(bytes, rk);
    let other_nonce = POP_NONCE.replace('f', "e");
    assert_ne!(bytes, pop_signed_bytes(PopOp::Register, USER_KID, &other_nonce, PID).unwrap());
    assert_ne!(
        bytes,
        pop_signed_bytes(PopOp::Register, USER_KID, POP_NONCE, "01JB8Z3Q0V6X9KQ4M2N7T5R1WE").unwrap()
    );
    assert_ne!(
        bytes,
        pop_signed_bytes(PopOp::Register, &"0".repeat(32), POP_NONCE, PID).unwrap()
    );
}

#[test]
fn pop_refuses_separator_smuggling() {
    use PopOp::Register as R;
    let u = USER_KID;
    // ("aa\nBB", "C") and ("aa", "BB\nC") would join to the same bytes.
    assert_eq!(pop_signed_bytes(R, u, "aa\nBB", "C"), Err(CertError::BadNonce));
    assert_eq!(pop_signed_bytes(R, u, "aa", "BB\nC"), Err(CertError::BadNonce));
    let n = POP_NONCE;
    assert_eq!(pop_signed_bytes(R, u, n, "BB\nC"), Err(CertError::BadProjectId));
    assert_eq!(pop_signed_bytes(R, u, n, &format!("{PID}\n")), Err(CertError::BadProjectId));
    for bad in ["", "00ff", &n.to_uppercase(), &format!("{n}0"), "zz112233445566778899aabbccddeeff"] {
        assert_eq!(pop_signed_bytes(R, u, bad, PID), Err(CertError::BadNonce), "{bad}");
    }
    for bad in ["", "ab\ncd", &u.to_uppercase(), &format!("{u}0")] {
        assert_eq!(pop_signed_bytes(R, bad, n, PID), Err(CertError::BadHex("user_key_id")), "{bad}");
    }
}

#[test]
fn uppercase_sig_is_refused_everywhere() {
    let mut c = cert();
    c.sig = c.sig.to_uppercase();
    assert_eq!(c.verify(&user_pk(), now()), Err(CertError::BadHex("sig")));
    let mut a = anchor();
    let lower_hash = a.hash();
    a.sig = a.sig.to_uppercase();
    assert!(a.verify(&proj_key().verifying_key().to_bytes()).is_err());
    // The hash of the genuine statement is unchanged by the refused spelling.
    assert_eq!(anchor().hash(), lower_hash);
    // Uppercase keys fail too.
    let mut c = cert();
    c.project_pubkey = c.project_pubkey.to_uppercase();
    assert!(c.verify(&user_pk(), now()).is_err());
}

#[test]
fn cert_project_id_and_issue_time_are_checked() {
    let mut r = req();
    r.project_id = "not-a-ulid\nx".into();
    let c = ProjectCert::sign(&user_key(), &r);
    assert_eq!(c.verify(&user_pk(), now()), Err(CertError::BadProjectId));
    // issued_at slightly ahead is skew; well ahead is refused.
    let mut r = req();
    r.issued_at = now() + Duration::seconds(30);
    ProjectCert::sign(&user_key(), &r).verify(&user_pk(), now()).unwrap();
    r.issued_at = now() + Duration::seconds(120);
    assert_eq!(
        ProjectCert::sign(&user_key(), &r).verify(&user_pk(), now()),
        Err(CertError::NotYetValid)
    );
    // Non-canonical spellings of a timestamp are refused.
    let mut c = cert();
    c.issued_at = "2026-10-01T09:30:00+00:00".into();
    assert_eq!(c.verify(&user_pk(), now()), Err(CertError::BadTimestamp("issued_at")));
}

#[test]
fn anchor_digest_fields_are_shape_checked() {
    let pk = proj_key().verifying_key().to_bytes();
    let resign = |f: &dyn Fn(&mut ProjectAnchorStmt)| {
        let mut a = anchor();
        f(&mut a);
        a.sign(&proj_key())
    };
    let cases: Vec<(&str, ProjectAnchorStmt)> = vec![
        ("head_hash", resign(&|a| a.head_hash = "AB".repeat(32))),
        ("head_hash", resign(&|a| a.head_hash = "ab".repeat(31))),
        ("rule_hash", resign(&|a| a.rule_hash = "x".repeat(64))),
        ("prev_anchor", resign(&|a| a.prev_anchor = Some("Ab".repeat(32)))),
    ];
    for (name, a) in cases {
        assert_eq!(a.verify(&pk), Err(CertError::BadDigest(name)));
    }
    let a = resign(&|a| a.project_id = "x\ny".into());
    assert_eq!(a.verify(&pk), Err(CertError::BadProjectId));
}

#[test]
fn canonical_json_sorts_keys_and_nests() {
    let v: serde_json::Value =
        serde_json::from_str(r#"{"b":[{"z":1,"a":null}],"a":"x\"y","é":true}"#).unwrap();
    assert_eq!(
        canonical_json(&v),
        r#"{"a":"x\"y","b":[{"a":null,"z":1}],"é":true}"#
    );
}

#[test]
fn hex_helpers() {
    assert_eq!(hex_decode::<2>("0aff"), Some([0x0a, 0xff]));
    assert_eq!(hex_decode::<2>("0aFf"), None);
    assert_eq!(hex_decode::<2>("0a"), None);
    assert_eq!(hex_decode::<1>("zz"), None);
}

// ---- ServeSection extensions (Phase 2 package A) ----

mod serve {
    use super::super::{ChildState, ServeSection, ServeVia};
    use super::super::{DEFAULT_IDLE_STOP_SECS, DEFAULT_RESTART_MAX, DEFAULT_RESTART_WINDOW_SECS};

    #[test]
    fn old_serve_sections_load_with_defaults() {
        let s: ServeSection = toml::from_str("via = \"user-daemon\"\nidle_stop_secs = 5").unwrap();
        assert_eq!(s.restart_max(), DEFAULT_RESTART_MAX);
        assert_eq!(s.restart_window_secs(), DEFAULT_RESTART_WINDOW_SECS);
        assert_eq!(s.idle_stop_secs(), 5);
        assert!(s.kernel_version.is_none() && s.kernel_sha.is_none());
        let empty: ServeSection = toml::from_str("").unwrap();
        assert_eq!(empty, ServeSection::default());
        assert_eq!(empty.idle_stop_secs(), 0);
    }

    #[test]
    fn unset_fields_do_not_appear_when_serialised() {
        let text = toml::to_string(&ServeSection::default()).unwrap();
        for key in ["restart_max", "restart_window_secs", "kernel_version", "kernel_sha", "idle_stop"] {
            assert!(!text.contains(key), "{key} leaked into {text}");
        }
    }

    #[test]
    fn child_kernel_section_round_trips() {
        let src = "via = \"child-kernel\"\nidle_stop_secs = 0\nrestart_max = 3\nrestart_window_secs = 30\nkernel_version = \"0.8.2\"\nkernel_sha = \"abc\"";
        let s: ServeSection = toml::from_str(src).unwrap();
        assert_eq!(s.via, ServeVia::ChildKernel);
        assert_eq!((s.restart_max(), s.restart_window_secs(), s.idle_stop_secs()), (3, 30, 0));
        let back: ServeSection = toml::from_str(&toml::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn idle_stop_defaults_to_1800_only_for_child_kernels() {
        let child: ServeSection = toml::from_str("via = \"child-kernel\"").unwrap();
        assert_eq!(child.idle_stop_secs(), DEFAULT_IDLE_STOP_SECS);
        assert_eq!(DEFAULT_IDLE_STOP_SECS, 1800);
        let user: ServeSection = toml::from_str("via = \"user-daemon\"").unwrap();
        assert_eq!(user.idle_stop_secs(), 0);
    }

    #[test]
    fn child_state_names_match_state_json() {
        let names: Vec<String> = [
            ChildState::Stopped,
            ChildState::Starting,
            ChildState::Running,
            ChildState::IdleStopping,
            ChildState::Failed,
        ]
        .iter()
        .map(|s| serde_json::to_value(s).unwrap().as_str().unwrap().to_owned())
        .collect();
        assert_eq!(names, ["stopped", "starting", "running", "idle-stopping", "failed"]);
    }
}
