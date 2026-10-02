use clawft_mesh_local::cert::UserCert;
use clawft_mesh_local::framing::{write_frame, FrameError, FrameReader};
use clawft_mesh_local::proto::*;
use clawft_mesh_local::{InjectedPeer, PeerIdentity, Principal};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

fn rt(f: &Frame) -> Frame {
    let s = serde_json::to_string(f).unwrap();
    assert!(!s.contains('\n'));
    serde_json::from_str(&s).unwrap()
}

fn cert() -> UserCert {
    UserCert::issue(&SigningKey::from_bytes(&[1; 32]), [2; 32], 1, 1000, 60)
}

fn all_messages() -> Vec<Message> {
    let pb = ProjectBinding { project_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(), project_pubkey: [3; 32], cert_sig: [4; 64] };
    vec![
        Message::Hello { proto_min: 1, proto_max: 2, features: vec!["a".into()], role: Role::Admin, build_sha: "s".into(), exe: "/x".into(), pid: 7, client_nonce: [8; 32] },
        Message::HelloAck(HelloAck { proto: 1, features: vec![], node_id: "n".into(), machine_pubkey: [5; 32], service_build_sha: "s".into(), deprecated_below: Some(1), uid: 501, challenge: [6; 32], machine_sig: [7; 64] }),
        Message::Error(ErrorBody::new(ErrorKind::BindPending, "m", "r")),
        Message::Register(RegisterReq { user_pubkey: [2; 32], sig: [9; 64], addresses: Addresses { user_id: "u".into(), projects: vec![pb.clone()] }, topic_prefixes: vec!["p".into()], capabilities: vec!["c".into()], version: "1".into(), build_sha: "s".into(), accept_from: vec![] }),
        Message::RegisterAck(RegisterAck { user_id: "u".into(), cert: cert(), accepted: Accepted { addresses: vec!["a".into()], topic_prefixes: vec![] }, rejected: vec![Rejected { what: "w".into(), reason: "r".into() }], bind: BindState::Pending }),
        Message::Renew {},
        Message::Cert { cert: cert() },
        Message::AddressAdd(pb.clone()),
        Message::AddressRemove(pb),
        Message::Subscribe { prefix: "p".into() },
        Message::Unsubscribe { prefix: "p".into() },
        Message::Ack {},
        Message::Send { dest: "weft://local/_/_".into(), message: json!({"k": [1, 2]}), request_id: Some("r".into()) },
        Message::Deliver(Deliver { source_node: "n".into(), source_cert: Some(cert()), scope: Scope { user_id: "u".into(), project_id: None }, envelope_id: "e".into(), message: json!(null) }),
        Message::VerdictRequest(VerdictRequest { subject: VerdictSubject::PeerAdmit, peer: PeerInfo { node_id: "n".into(), pubkey: "k".into(), platform: "p".into(), capabilities: vec![], genesis_hash: "g".into(), chain_seq: 3 }, topic: None }),
        Message::VerdictReply { allow: true, ttl_s: 300, reason: "r".into(), rule_hash: "h".into() },
        Message::JournalHead {},
        Message::JournalHeadReply { seq: 1, hash: "h".into(), ts: 2, sig: "s".into() },
        Message::Status {}, Message::PeersList {}, Message::FactsGet {},
        Message::Reply { data: json!({"a": 1}) },
        Message::BindingsList {},
        Message::BindApprove { uid: 1, user_id: None },
        Message::BindRevoke { uid: 1, reason: "r".into() },
        Message::BindRebind { uid: 1, user_pubkey: Some("k".into()) },
        Message::PeerRevoke { node_id: "n".into(), reason: "r".into() },
        Message::PeerUnrevoke { node_id: "n".into() },
        Message::PolicySet { admission: Some("enforce".into()), cluster_owner_uid: Some(1000) },
        Message::Ping {}, Message::Pong {}, Message::Bye {},
    ]
}

#[test]
fn every_message_round_trips_with_and_without_id() {
    for m in all_messages() {
        for id in [None, Some(42)] {
            let f = Frame { id, msg: m.clone() };
            assert_eq!(rt(&f), f, "{m:?}");
        }
    }
}

#[test]
fn golden_wire_examples() {
    let hello = Frame::new(Message::Hello { proto_min: 1, proto_max: 1, features: vec![], role: Role::User, build_sha: "abc".into(), exe: "weaver".into(), pid: 9, client_nonce: [0; 32] });
    assert_eq!(
        serde_json::to_string(&hello).unwrap(),
        r#"{"t":"hello","proto_min":1,"proto_max":1,"features":[],"role":"user","build_sha":"abc","exe":"weaver","pid":9,"client_nonce":"0000000000000000000000000000000000000000000000000000000000000000"}"#
    );
    let pong = Frame::with_id(5, Message::Ping {});
    assert_eq!(serde_json::to_string(&pong).unwrap(), r#"{"id":5,"t":"ping"}"#);
    let ack = Frame::with_id(1, Message::Ack {});
    assert_eq!(serde_json::to_string(&ack).unwrap(), r#"{"id":1,"t":"ack"}"#);
    let err = Frame::new(Message::Error(ErrorBody::new(ErrorKind::AddressInUse, "held", "stop the other daemon")));
    assert_eq!(
        serde_json::to_string(&err).unwrap(),
        r#"{"t":"error","kind":"address_in_use","message":"held","remedy":"stop the other daemon"}"#
    );
    let dotted = serde_json::to_value(Message::VerdictReply { allow: false, ttl_s: 1, reason: String::new(), rule_hash: String::new() }).unwrap();
    assert_eq!(dotted["t"], "verdict.reply");
    let ha = serde_json::to_value(Message::HelloAck(HelloAck { proto: 1, features: vec![], node_id: "n".into(), machine_pubkey: [0xab; 32], service_build_sha: String::new(), deprecated_below: None, uid: 1, challenge: [0; 32], machine_sig: [0; 64] })).unwrap();
    assert_eq!(ha["machine_pubkey"], "ab".repeat(32));
    assert_eq!(ha["challenge"], "0".repeat(64));
    assert!(ha.get("deprecated_below").is_none());
}

#[test]
fn unknown_fields_and_unknown_types_are_tolerated() {
    let f: Frame = serde_json::from_str(r#"{"t":"hello","proto_min":1,"proto_max":3,"role":"user","client_nonce":"0000000000000000000000000000000000000000000000000000000000000000","future_field":{"x":1}}"#).unwrap();
    assert!(matches!(f.msg, Message::Hello { proto_max: 3, .. }));
    let f: Frame = serde_json::from_str(r#"{"id":9,"t":"from.the.future","a":1}"#).unwrap();
    assert_eq!(f, Frame { id: Some(9), msg: Message::Unknown });
    let f: Frame = serde_json::from_str(r#"{"t":"error","kind":"brand_new_kind","message":"m"}"#).unwrap();
    match f.msg {
        Message::Error(e) => {
            assert_eq!(e.kind, ErrorKind::Unknown);
            assert!(!e.kind.is_fatal());
            assert_eq!(e.remedy, "");
        }
        m => panic!("{m:?}"),
    }
    let f: Frame = serde_json::from_str(r#"{"t":"verdict.request","subject":"new.subject","peer":{"node_id":"n","pubkey":"k"}}"#).unwrap();
    assert!(matches!(f.msg, Message::VerdictRequest(VerdictRequest { subject: VerdictSubject::Unknown, .. })));
    // Missing required field and bad hex are errors, not panics.
    assert!(serde_json::from_str::<Frame>(r#"{"t":"hello","proto_min":1}"#).is_err());
    assert!(serde_json::from_str::<Frame>(r#"{"t":"hello_ack","proto":1,"node_id":"n","machine_pubkey":"XY","uid":1,"challenge":"00"}"#).is_err());
    assert!(serde_json::from_str::<Frame>(r#"{"no_type":true}"#).is_err());
}

#[test]
fn version_negotiation_matrix() {
    // (service, client, expected)
    let cases: [((u32, u32), (u32, u32), Option<u32>); 9] = [
        ((1, 1), (1, 1), Some(1)),
        ((1, 3), (1, 5), Some(3)),   // newer client, service caps it
        ((1, 3), (1, 2), Some(2)),   // older client
        ((2, 4), (1, 2), Some(2)),   // overlap at one point
        ((3, 4), (1, 2), None),      // client too old
        ((1, 2), (3, 4), None),      // client too new
        ((1, 2), (2, 2), Some(2)),
        ((1, 2), (3, 1), None),      // inverted client range
        ((2, 1), (1, 2), None),      // inverted service range
    ];
    for (s, c, want) in cases {
        assert_eq!(negotiate(s, c).ok(), want, "service {s:?} client {c:?}");
    }
    let m = negotiate((1, 1), (2, 3)).unwrap_err();
    assert_eq!(m.remedy(), "restart the service after `weaver update`");
    let m = negotiate((3, 4), (1, 2)).unwrap_err();
    assert_eq!(m.remedy(), "update the user daemon");
    let body = ErrorBody::proto_mismatch(&m);
    assert_eq!(body.kind, ErrorKind::ProtoMismatch);
    assert!(body.kind.is_fatal());
    assert_eq!(body.data.as_ref().unwrap()["service"]["min"], 3);
    assert_eq!(body.data.as_ref().unwrap()["client"]["max"], 2);
    assert_eq!(negotiate_service(PROTO_MIN, PROTO_MAX + 5).unwrap(), PROTO_MAX);
    assert!(negotiate_service(PROTO_MAX + 1, PROTO_MAX + 2).is_err());
}

#[test]
fn feature_intersection_ignores_unknowns() {
    let a: Vec<String> = ["x", "y", "y", "z"].map(String::from).to_vec();
    let b: Vec<String> = ["z", "x", "q"].map(String::from).to_vec();
    assert_eq!(negotiate_features(&a, &b), vec!["x".to_string(), "z".to_string()]);
    assert!(negotiate_features(&a, &[]).is_empty());
}

#[test]
fn register_signature_binds_challenge_uid_and_node() {
    let key = SigningKey::from_bytes(&[11; 32]);
    let (challenge, node, uid) = ([1u8; 32], "n".repeat(32), Principal::Uid(501));
    let sig = key.sign(&register_signing_bytes(&challenge, &uid, &node)).to_bytes();
    let req = RegisterReq { user_pubkey: key.verifying_key().to_bytes(), sig, addresses: Addresses { user_id: "u".into(), projects: vec![] }, topic_prefixes: vec![], capabilities: vec![], version: String::new(), build_sha: String::new(), accept_from: vec![] };
    assert!(verify_register_sig(&req, &challenge, &uid, &node));
    assert!(!verify_register_sig(&req, &[2u8; 32], &uid, &node), "other challenge");
    assert!(!verify_register_sig(&req, &challenge, &Principal::Uid(502), &node), "other uid");
    assert!(!verify_register_sig(&req, &challenge, &uid, &"m".repeat(32)), "other node");
    let mut forged = req.clone();
    forged.user_pubkey = SigningKey::from_bytes(&[12; 32]).verifying_key().to_bytes();
    assert!(!verify_register_sig(&forged, &challenge, &uid, &node), "other key");
    let b = register_signing_bytes(&challenge, &uid, &node);
    assert!(b.starts_with(b"weftos/mesh-local/register/v1\0"));
    assert_eq!(b[62], 1, "uid tag");
    assert_eq!(&b[63..65], &4u16.to_be_bytes());
    assert_eq!(&b[65..69], &501u32.to_be_bytes());
    assert_eq!(&b[69..71], &32u16.to_be_bytes(), "node id is length prefixed");
}

#[tokio::test]
async fn framing_reader_limits_blank_lines_and_eof() {
    // Two frames, a blank line between them, clean EOF after.
    let data = b"{\"t\":\"ping\"}\n\n  \n{\"id\":2,\"t\":\"pong\"}\n".to_vec();
    let mut r = FrameReader::new(&data[..]);
    assert_eq!(r.read_frame().await.unwrap().unwrap().msg, Message::Ping {});
    assert_eq!(r.read_frame().await.unwrap().unwrap(), Frame::with_id(2, Message::Pong {}));
    assert!(r.read_frame().await.unwrap().is_none());

    // Truncated final line.
    let mut r = FrameReader::new(&b"{\"t\":\"ping\"}"[..]);
    assert!(matches!(r.read_frame().await, Err(FrameError::Truncated)));

    // Garbage.
    let mut r = FrameReader::new(&b"not json\n"[..]);
    assert!(matches!(r.read_frame().await, Err(FrameError::Json(_))));

    // Over the limit without ever finding a newline: rejected without
    // buffering the rest.
    let big = vec![b'a'; 5000];
    let mut r = FrameReader::with_limit(&big[..], 1024);
    assert!(matches!(r.read_frame().await, Err(FrameError::LineTooLong)));
    // Exactly at the limit is allowed, one over is not.
    let obj = format!("{{\"t\":\"ping\",\"pad\":\"{}\"}}", "x".repeat(100));
    let mut line = obj.clone().into_bytes();
    line.push(b'\n');
    let mut r = FrameReader::with_limit(&line[..], obj.len());
    assert!(r.read_frame().await.unwrap().is_some());
    let mut r = FrameReader::with_limit(&line[..], obj.len() - 1);
    assert!(matches!(r.read_frame().await, Err(FrameError::LineTooLong)));
}

#[tokio::test]
async fn framing_deadline_and_write_limit() {
    let (a, _keep_open) = tokio::io::duplex(64);
    let mut r = FrameReader::new(a);
    let e = r.read_frame_within(std::time::Duration::from_millis(30)).await;
    assert!(matches!(e, Err(FrameError::Deadline)));

    let mut sink = Vec::new();
    let huge = Frame::new(Message::Reply { data: Value::String("x".repeat(2 << 20)) });
    assert!(matches!(write_frame(&mut sink, &huge).await, Err(FrameError::LineTooLong)));
    write_frame(&mut sink, &Frame::new(Message::Ping {})).await.unwrap();
    assert_eq!(sink, b"{\"t\":\"ping\"}\n");
}

#[test]
fn injected_peer_and_principal_bytes() {
    let p = InjectedPeer::uid(1000);
    assert_eq!(p.principal().unwrap(), Principal::Uid(1000));
    assert_eq!(Principal::Sid("S-1-5".into()).signing_bytes(), b"\x02\x00\x05S-1-5");
    assert_eq!(Principal::Uid(1000).signing_bytes(), [1, 0, 4, 0, 0, 3, 0xe8]);
}

#[tokio::test]
#[cfg(unix)]
async fn real_peer_cred_is_this_process() {
    let (a, _b) = tokio::net::UnixStream::pair().unwrap();
    let creds = clawft_mesh_local::peer::UnixPeer::from_stream(&a).unwrap().credentials().unwrap();
    assert_eq!(creds.pid, Some(std::process::id()));
    assert!(matches!(creds.principal, Principal::Uid(_)));
    assert!(creds.gid.is_some());
}

#[test]
fn register_bytes_cannot_collide_across_principal_kinds_or_boundaries() {
    let c = [1u8; 32];
    let uid = register_signing_bytes(&c, &Principal::Uid(0x7369_6401), "n");
    let sid = register_signing_bytes(&c, &Principal::Sid("sid\u{1}".into()), "n");
    assert_ne!(uid, sid);
    // Moving a byte between principal and node id changes the bytes.
    let a = register_signing_bytes(&c, &Principal::Sid("ab".into()), "c");
    let b = register_signing_bytes(&c, &Principal::Sid("a".into()), "bc");
    assert_ne!(a, b);
}

#[test]
fn hello_proof_binds_nonce_challenge_and_key() {
    let key = SigningKey::from_bytes(&[5; 32]);
    let pk = key.verifying_key().to_bytes();
    let (nonce, challenge) = ([1u8; 32], [2u8; 32]);
    let sig = key.sign(&hello_signing_bytes(&nonce, &challenge, &pk, 501)).to_bytes();
    let ack = HelloAck { proto: 1, features: vec![], node_id: String::new(), machine_pubkey: pk, service_build_sha: String::new(), deprecated_below: None, uid: 501, challenge, machine_sig: sig };
    assert!(verify_hello_proof(&ack, &nonce));
    assert!(!verify_hello_proof(&ack, &[9u8; 32]), "old nonce");
    assert!(!verify_hello_proof(&HelloAck { uid: 502, ..ack.clone() }, &nonce), "other uid");
    assert!(!verify_hello_proof(&HelloAck { challenge: [3; 32], ..ack.clone() }, &nonce), "other challenge");
    assert!(!verify_hello_proof(&HelloAck { machine_pubkey: SigningKey::from_bytes(&[6; 32]).verifying_key().to_bytes(), ..ack }, &nonce), "other key");
    assert!(hello_signing_bytes(&nonce, &challenge, &pk, 501).starts_with(b"weftos/mesh-local/hello/v1\0"));
}

#[test]
fn reply_class_and_service_id_namespace() {
    assert!(is_reply_class(&Message::Ack {}));
    assert!(is_reply_class(&Message::Pong {}));
    assert!(is_reply_class(&Message::Cert { cert: cert() }));
    assert!(!is_reply_class(&Message::Ping {}));
    assert!(!is_reply_class(&Message::Status {}));
    assert!(!is_reply_class(&Message::Unknown));
    assert_eq!(SERVICE_ID_FLAG, 1u64 << 63);
}

/// Golden vector shared with the kernel's `node_id_from_pubkey` test: the same
/// pubkey and expected id must appear there verbatim.
///   pubkey  = bytes 0x00..=0x1f
///   node id = 630dcd2966c4336691125448bbb25b4f   (sha256(pubkey)[..16], hex)
#[test]
fn node_id_golden_vector() {
    let pk: [u8; 32] = std::array::from_fn(|i| i as u8);
    assert_eq!(clawft_mesh_local::node_id_from_pubkey(&pk), "630dcd2966c4336691125448bbb25b4f");
}
