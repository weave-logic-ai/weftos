//! Tests for mesh admission (P3-K1): hello verification, the gate, and
//! the listener behaviour over real TCP with Noise and plaintext peers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::revocation::RevocationList;
use clawft_types::config::MeshAdmissionMode;
use crate::error::KernelResult;
use crate::ipc::{KernelMessage, MessageTarget};
use crate::mesh::MeshTransport;
use crate::mesh_delivery::{LocalDelivery, PeerCtx};
use crate::mesh_ipc::{MeshIpcEnvelope, Scope};
use crate::mesh_noise::{EncryptedChannel, NoiseChannel, NoiseConfig, NoisePattern, PassthroughChannel};
use crate::mesh_runtime::MeshRuntime;
use crate::mesh_serve::{screen_frame, serve_listener, Active};

const GENESIS: [u8; 32] = [7u8; 32];

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn noise_keypair() -> snow::Keypair {
    snow::Builder::new("Noise_XX_25519_ChaChaPoly_SHA256".parse().unwrap())
        .generate_keypair()
        .unwrap()
}

fn caps(c: &[&str]) -> Vec<String> {
    c.iter().map(|s| (*s).to_owned()).collect()
}

// ── pure verification ────────────────────────────────────────────

fn hello_for(k: &SigningKey, hash: &[u8], stat: &[u8], genesis: &[u8; 32], ts: u64) -> AdmitHello {
    AdmitHello::sign(k, hash, stat, genesis, ts, "linux", vec![])
}

fn binding<'a>(hash: &'a [u8], stat: &'a [u8]) -> ChannelBinding<'a> {
    ChannelBinding { handshake_hash: hash, remote_static: Some(stat) }
}

#[test]
fn verify_accepts_a_good_hello() {
    let k = key(1);
    let (h, s) = ([1u8; 32], [2u8; 32]);
    let v = hello_for(&k, &h, &s, &GENESIS, now()).verify(Some(&binding(&h, &s)), now()).unwrap();
    assert_eq!(v.node_id, node_id_from_pubkey(&k.verifying_key().to_bytes()));
    assert_eq!(v.genesis_hash, GENESIS);
}

#[test]
fn verify_rejects_bad_signature() {
    let (h, s) = ([1u8; 32], [2u8; 32]);
    let mut hello = hello_for(&key(1), &h, &s, &GENESIS, now());
    // Valid signature from a different key over the same bytes.
    hello.sig = hello_for(&key(9), &h, &s, &GENESIS, now()).sig;
    assert_eq!(hello.verify(Some(&binding(&h, &s)), now()), Err(HelloFailure::BadSignature));
}

#[test]
fn verify_rejects_node_id_not_derived_from_pubkey() {
    let (h, s) = ([1u8; 32], [2u8; 32]);
    let mut hello = hello_for(&key(1), &h, &s, &GENESIS, now());
    hello.node_id = node_id_from_pubkey(&key(2).verifying_key().to_bytes());
    assert_eq!(hello.verify(Some(&binding(&h, &s)), now()), Err(HelloFailure::NodeIdMismatch));
}

#[test]
fn verify_rejects_a_hello_signed_for_another_session() {
    let s = [2u8; 32];
    let hello = hello_for(&key(1), &[1u8; 32], &s, &GENESIS, now());
    let other = [3u8; 32];
    assert_eq!(hello.verify(Some(&binding(&other, &s)), now()), Err(HelloFailure::BadSignature));
}

#[test]
fn verify_rejects_static_key_mismatch() {
    let h = [1u8; 32];
    let hello = hello_for(&key(1), &h, &[2u8; 32], &GENESIS, now());
    assert_eq!(hello.verify(Some(&binding(&h, &[4u8; 32])), now()), Err(HelloFailure::StaticMismatch));
}

#[test]
fn verify_rejects_skewed_timestamps_both_ways() {
    let (h, s) = ([1u8; 32], [2u8; 32]);
    for ts in [now() - MAX_SKEW_SECS - 5, now() + MAX_SKEW_SECS + 5] {
        let hello = hello_for(&key(1), &h, &s, &GENESIS, ts);
        assert_eq!(hello.verify(Some(&binding(&h, &s)), now()), Err(HelloFailure::ClockSkew));
    }
}

#[test]
fn verify_needs_a_handshake_binding() {
    let hello = hello_for(&key(1), &[1u8; 32], &[2u8; 32], &GENESIS, now());
    assert_eq!(hello.verify(None, now()), Err(HelloFailure::NoHandshakeBinding));
}

#[test]
fn parse_frame_tells_hello_from_envelope_and_garbage() {
    let hello = hello_for(&key(1), &[1u8; 32], &[2u8; 32], &GENESIS, now());
    assert!(matches!(AdmitHello::parse_frame(&hello.to_bytes()), Some(Ok(_))));
    let msg = KernelMessage::text(0, MessageTarget::Topic("t".into()), "x");
    let env = MeshIpcEnvelope::new("a".into(), "b".into(), msg).to_bytes().unwrap();
    assert!(AdmitHello::parse_frame(&env).is_none());
    assert!(AdmitHello::parse_frame(&[0x01, 0x02]).is_none());
    let broken = format!(r#"{{"kind":"{HELLO_KIND}","v":1}}"#);
    assert!(matches!(AdmitHello::parse_frame(broken.as_bytes()), Some(Err(HelloFailure::Malformed(_)))));
}

// ── gate policy ──────────────────────────────────────────────────

struct Counting(AtomicUsize, Verdict);

#[async_trait]
impl VerdictSource for Counting {
    async fn verdict(&self, _: &VerdictRequest) -> Verdict {
        self.0.fetch_add(1, Ordering::SeqCst);
        self.1.clone()
    }
}

fn verified(seed: u8, genesis: [u8; 32], capabilities: Vec<String>) -> VerifiedHello {
    let pk = key(seed).verifying_key().to_bytes();
    VerifiedHello {
        node_id: node_id_from_pubkey(&pk),
        pubkey: pk,
        platform: "linux".into(),
        capabilities,
        genesis_hash: genesis,
        ts: now(),
    }
}

fn gate(mode: MeshAdmissionMode, v: Verdict) -> (CryptoGate, Arc<Counting>, Arc<RevocationList>) {
    let dir = tempfile::tempdir().unwrap().keep();
    let rev = Arc::new(RevocationList::new(dir.join("revoked.json")));
    let src = Arc::new(Counting(AtomicUsize::new(0), v));
    (CryptoGate::new(GENESIS, rev.clone(), src.clone(), mode), src, rev)
}

const NOISE_CTX: AdmitContext = AdmitContext { class: PeerClass::Node, channel: ChannelKind::Noise };

#[tokio::test]
async fn wrong_genesis_is_refused_under_enforce_and_recorded() {
    let (g, ..) = gate(MeshAdmissionMode::Enforce, Verdict::Permit);
    let r = g.admit(&verified(1, [9u8; 32], vec![]), &NOISE_CTX).await;
    assert!(matches!(r, Admission::Refuse(Refusal { code: "wrong_genesis", .. })));
    assert_eq!(g.records().len(), 1);
    assert!(!g.records()[0].admitted);
}

#[tokio::test]
async fn revoked_peer_is_refused() {
    let (g, _, rev) = gate(MeshAdmissionMode::Enforce, Verdict::Permit);
    let v = verified(1, GENESIS, vec![]);
    rev.revoke_host(&v.node_id, "test");
    let r = g.admit(&v, &NOISE_CTX).await;
    assert!(matches!(r, Admission::Refuse(Refusal { code: "revoked", .. })));
}

#[tokio::test]
async fn observe_admits_and_records_the_would_be_refusal() {
    let (g, ..) = gate(MeshAdmissionMode::Observe, Verdict::Permit);
    match g.admit(&verified(1, [9u8; 32], vec![]), &NOISE_CTX).await {
        Admission::Admit(grant) => {
            assert_eq!(grant.observed.unwrap().code, "wrong_genesis");
            assert!(!grant.trust_scope, "an observed failure must not earn scope trust");
        }
        other => panic!("observe must admit: {other:?}"),
    }
    let rec = g.records();
    assert_eq!((rec.len(), rec[0].admitted), (1, true));
}

#[tokio::test]
async fn verdict_deny_refuses_under_enforce_but_not_observe() {
    let (e, ..) = gate(MeshAdmissionMode::Enforce, Verdict::Deny("no".into()));
    let r = e.admit(&verified(1, GENESIS, vec![]), &NOISE_CTX).await;
    assert!(matches!(r, Admission::Refuse(Refusal { code: "verdict_denied", .. })));
    let (o, ..) = gate(MeshAdmissionMode::Observe, Verdict::Deny("no".into()));
    assert!(matches!(o.admit(&verified(1, GENESIS, vec![]), &NOISE_CTX).await, Admission::Admit(_)));
}

#[tokio::test]
async fn verdicts_are_cached_per_id_and_key() {
    let (g, src, _) = gate(MeshAdmissionMode::Enforce, Verdict::Permit);
    let v = verified(1, GENESIS, vec![]);
    g.admit(&v, &NOISE_CTX).await;
    g.admit(&v, &NOISE_CTX).await;
    assert_eq!(src.0.load(Ordering::SeqCst), 1);
    // Same node id, different key: must not ride the cached permit.
    let mut impostor = verified(2, GENESIS, vec![]);
    impostor.node_id = v.node_id.clone();
    g.admit(&impostor, &NOISE_CTX).await;
    assert_eq!(src.0.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn leaf_class_is_limited_only_under_enforce() {
    let leaf = verified(1, GENESIS, caps(&[CAP_LEAF]));
    let ctx = AdmitContext { class: leaf.class(), channel: ChannelKind::Noise };
    let (e, ..) = gate(MeshAdmissionMode::Enforce, Verdict::Permit);
    let Admission::Admit(g) = e.admit(&leaf, &ctx).await else { panic!("leaf admitted") };
    assert_eq!((g.limits, g.trust_scope), (PeerLimits::Leaf, false));
    let (o, ..) = gate(MeshAdmissionMode::Observe, Verdict::Permit);
    let Admission::Admit(g) = o.admit(&leaf, &ctx).await else { panic!("leaf admitted") };
    assert_eq!(g.limits, PeerLimits::None);
}

#[tokio::test]
async fn unsigned_peers_enforce_refuses_observe_records() {
    let ctx = AdmitContext { class: PeerClass::Legacy, channel: ChannelKind::Passthrough };
    let (e, ..) = gate(MeshAdmissionMode::Enforce, Verdict::Permit);
    assert!(matches!(e.admit_unverified(&HelloFailure::Missing, &ctx).await, Admission::Refuse(_)));
    let (o, ..) = gate(MeshAdmissionMode::Observe, Verdict::Permit);
    assert!(matches!(o.admit_unverified(&HelloFailure::Missing, &ctx).await, Admission::Admit(_)));
    assert_eq!(o.records()[0].code, "hello_missing");
    let (off, ..) = gate(MeshAdmissionMode::Off, Verdict::Permit);
    assert!(matches!(off.admit_unverified(&HelloFailure::Missing, &ctx).await, Admission::Admit(_)));
    assert!(off.records().is_empty());
}

// ── screen_frame (post-admission rules) ──────────────────────────

fn env_bytes(src: &str, topic: &str, src_scope: Option<Scope>) -> Vec<u8> {
    let msg = KernelMessage::text(0, MessageTarget::Topic(topic.into()), "x");
    let mut env = MeshIpcEnvelope::new(src.into(), "node-b".into(), msg);
    env.src_scope = src_scope;
    env.to_bytes().unwrap()
}

fn scope() -> Scope {
    Scope { user_id: "a".repeat(32), project_id: None }
}

#[test]
fn src_scope_is_kept_only_for_verified_peers() {
    let bytes = env_bytes("n", "t", Some(scope()));
    let trusted = Active { bound: Some("n".into()), limits: PeerLimits::None, trust_scope: true, admitted: true, class: PeerClass::Node, remote_static: None };
    let kept = screen_frame(bytes.clone(), &trusted).unwrap();
    assert!(MeshIpcEnvelope::from_bytes(&kept).unwrap().src_scope.is_some());

    let untrusted = Active { bound: None, limits: PeerLimits::None, trust_scope: false, admitted: false, class: PeerClass::Legacy, remote_static: None };
    let stripped = screen_frame(bytes, &untrusted).unwrap();
    assert!(MeshIpcEnvelope::from_bytes(&stripped).unwrap().src_scope.is_none());
}

#[test]
fn source_node_must_match_the_bound_id() {
    let act = Active { bound: Some("n".into()), limits: PeerLimits::None, trust_scope: false, admitted: false, class: PeerClass::Legacy, remote_static: None };
    assert!(screen_frame(env_bytes("n", "t", None), &act).is_some());
    assert!(screen_frame(env_bytes("evil", "t", None), &act).is_none());
}

// ── listener, end to end over TCP ────────────────────────────────

#[derive(Default)]
struct Recorder {
    got: Mutex<Vec<String>>,
    verified: Mutex<Vec<(String, bool)>>,
}

#[async_trait]
impl LocalDelivery for Recorder {
    async fn deliver(&self, from: &PeerCtx, _: Option<&Scope>, msg: KernelMessage) -> KernelResult<()> {
        if let MessageTarget::Topic(t) = msg.target {
            self.verified.lock().unwrap().push((t.clone(), from.node_verified));
            self.got.lock().unwrap().push(t);
        }
        Ok(())
    }
}

struct Server {
    rt: Arc<MeshRuntime>,
    addr: String,
    rec: Arc<Recorder>,
    task: tokio::task::JoinHandle<()>,
    noise_pub: Vec<u8>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn server(gate: Arc<dyn AdmissionGate>, noise: bool) -> Server {
    let rec = Arc::new(Recorder::default());
    let mut rt = MeshRuntime::new("node-b".into());
    rt.set_local_delivery(rec.clone());
    let transport = crate::mesh_tcp::TcpTransport;
    let listener = transport.listen("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let kp = noise_keypair();
    let cfg = noise.then(|| {
        Arc::new(NoiseConfig {
            pattern: NoisePattern::XX,
            local_private_key: kp.private.clone().try_into().unwrap(),
            remote_static_key: None,
        })
    });
    let rt = Arc::new(rt);
    let task = tokio::spawn(serve_listener(Arc::clone(&rt), listener, cfg, "tcp", "x", gate));
    Server { rt, addr, rec, task, noise_pub: kp.public }
}

struct Client {
    ch: Box<dyn EncryptedChannel>,
    static_pub: Vec<u8>,
}

impl Client {
    async fn connect(addr: &str, noise: bool) -> Self {
        let stream = crate::mesh_tcp::TcpTransport.connect(addr).await.unwrap();
        let kp = noise_keypair();
        if noise {
            let cfg = NoiseConfig {
                pattern: NoisePattern::XX,
                local_private_key: kp.private.clone().try_into().unwrap(),
                remote_static_key: None,
            };
            let ch = NoiseChannel::initiate(stream, &cfg).await.unwrap();
            Self { ch: Box::new(ch), static_pub: kp.public }
        } else {
            Self { ch: Box::new(PassthroughChannel::new(stream)), static_pub: vec![] }
        }
    }

    fn hello(&self, k: &SigningKey, genesis: &[u8; 32], capabilities: Vec<String>) -> AdmitHello {
        let hash = self.ch.handshake_hash().expect("noise session").to_vec();
        AdmitHello::sign(k, &hash, &self.static_pub, genesis, now(), "linux", capabilities)
    }

    async fn send(&mut self, bytes: &[u8]) {
        let _ = self.ch.send_encrypted(bytes).await;
    }

    async fn publish(&mut self, src: &str, topic: &str) {
        self.send(&env_bytes(src, topic, None)).await;
    }

    async fn closed(&mut self) -> bool {
        matches!(
            tokio::time::timeout(Duration::from_secs(3), self.ch.recv_encrypted()).await,
            Ok(Err(_))
        )
    }
}

async fn wait_for(rec: &Recorder, topic: &str) -> bool {
    for _ in 0..150 {
        if rec.got.lock().unwrap().iter().any(|t| t == topic) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

fn crypto(mode: MeshAdmissionMode) -> Arc<CryptoGate> {
    Arc::new(gate(mode, Verdict::Permit).0)
}

fn id_of(k: &SigningKey) -> String {
    node_id_from_pubkey(&k.verifying_key().to_bytes())
}

#[tokio::test]
async fn noise_peer_with_valid_hello_is_served_and_source_bound() {
    let g = crypto(MeshAdmissionMode::Enforce);
    let srv = server(g.clone(), true).await;
    let k = key(1);
    let mut c = Client::connect(&srv.addr, true).await;
    c.send(&c.hello(&k, &GENESIS, vec![]).to_bytes()).await;
    c.publish(&id_of(&k), "t.ok").await;
    assert!(wait_for(&srv.rec, "t.ok").await);
    // Spoofed source_node after admission is dropped; a later valid frame
    // proves the earlier one was processed (and discarded).
    c.publish("someone-else", "t.spoof").await;
    c.publish(&id_of(&k), "t.after").await;
    assert!(wait_for(&srv.rec, "t.after").await);
    assert!(!srv.rec.got.lock().unwrap().iter().any(|t| t == "t.spoof"));
    assert!(g.records().is_empty());
}

#[tokio::test]
async fn replayed_hello_on_a_second_session_is_refused() {
    let srv = server(crypto(MeshAdmissionMode::Enforce), true).await;
    let k = key(1);
    let mut first = Client::connect(&srv.addr, true).await;
    let captured = first.hello(&k, &GENESIS, vec![]).to_bytes();
    first.send(&captured).await;
    first.publish(&id_of(&k), "t.first").await;
    assert!(wait_for(&srv.rec, "t.first").await);

    let mut second = Client::connect(&srv.addr, true).await;
    second.send(&captured).await;
    second.publish(&id_of(&k), "t.replayed").await;
    assert!(second.closed().await, "replayed hello must close the session");
    assert!(!srv.rec.got.lock().unwrap().iter().any(|t| t == "t.replayed"));
}

#[tokio::test]
async fn enforce_refuses_wrong_genesis_and_observe_serves_it() {
    let k = key(1);
    let enforce = server(crypto(MeshAdmissionMode::Enforce), true).await;
    let mut c = Client::connect(&enforce.addr, true).await;
    c.send(&c.hello(&k, &[9u8; 32], vec![]).to_bytes()).await;
    c.publish(&id_of(&k), "t.x").await;
    assert!(c.closed().await);
    assert!(enforce.rec.got.lock().unwrap().is_empty());

    let og = crypto(MeshAdmissionMode::Observe);
    let observe = server(og.clone(), true).await;
    let mut c = Client::connect(&observe.addr, true).await;
    c.send(&c.hello(&k, &[9u8; 32], vec![]).to_bytes()).await;
    c.publish(&id_of(&k), "t.x").await;
    assert!(wait_for(&observe.rec, "t.x").await);
    assert_eq!(og.records()[0].code, "wrong_genesis");
}

#[tokio::test]
async fn envelope_before_hello_is_not_a_bypass_under_enforce() {
    let srv = server(crypto(MeshAdmissionMode::Enforce), true).await;
    let k = key(1);
    let mut c = Client::connect(&srv.addr, true).await;
    c.publish(&id_of(&k), "t.early").await;
    c.send(&c.hello(&k, &GENESIS, vec![]).to_bytes()).await;
    assert!(c.closed().await);
    assert!(srv.rec.got.lock().unwrap().is_empty());
}

#[tokio::test]
async fn enforce_limits_leaf_publishes_to_its_substrate_prefix() {
    let srv = server(crypto(MeshAdmissionMode::Enforce), true).await;
    let k = key(3);
    let id = id_of(&k);
    let mut c = Client::connect(&srv.addr, true).await;
    c.send(&c.hello(&k, &GENESIS, caps(&[CAP_LEAF])).to_bytes()).await;
    c.publish(&id, "kernel.admin").await;
    c.publish(&id, &format!("substrate/{}/sensor", "someone-else")).await;
    c.publish(&id, &format!("substrate/{id}/sensor")).await;
    assert!(wait_for(&srv.rec, &format!("substrate/{id}/sensor")).await);
    assert_eq!(srv.rec.got.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn plaintext_peers_enforce_refuses_observe_records_off_serves() {
    let k = key(1);
    // enforce: even a hello cannot help, there is no handshake to bind.
    let e = crypto(MeshAdmissionMode::Enforce);
    let srv = server(e.clone(), false).await;
    let mut c = Client::connect(&srv.addr, false).await;
    c.publish(&id_of(&k), "t.plain").await;
    assert!(c.closed().await);
    assert!(srv.rec.got.lock().unwrap().is_empty());

    // observe: legacy plaintext (ESP32-style) is served and recorded.
    let o = crypto(MeshAdmissionMode::Observe);
    let srv = server(o.clone(), false).await;
    let mut c = Client::connect(&srv.addr, false).await;
    c.publish("esp32-leaf", "t.plain").await;
    assert!(wait_for(&srv.rec, "t.plain").await);
    assert_eq!(o.records()[0].code, "hello_missing");

    // AllowAll and mode off: untouched behaviour.
    for g in [Arc::new(AllowAll) as Arc<dyn AdmissionGate>, crypto(MeshAdmissionMode::Off)] {
        let srv = server(g, false).await;
        let mut c = Client::connect(&srv.addr, false).await;
        c.publish("esp32-leaf", "t.plain").await;
        assert!(wait_for(&srv.rec, "t.plain").await);
    }
}

#[tokio::test]
async fn plaintext_hello_is_consumed_and_recorded_under_observe() {
    let o = crypto(MeshAdmissionMode::Observe);
    let srv = server(o.clone(), false).await;
    let mut c = Client::connect(&srv.addr, false).await;
    let hello = AdmitHello::sign(&key(1), &[0u8; 32], &[], &GENESIS, now(), "linux", vec![]);
    c.send(&hello.to_bytes()).await;
    c.publish("whoever", "t.plain").await;
    assert!(wait_for(&srv.rec, "t.plain").await);
    assert_eq!(o.records()[0].code, "no_handshake_binding");
}

#[tokio::test]
async fn unsigned_noise_peer_enforce_refuses_observe_serves() {
    let srv = server(crypto(MeshAdmissionMode::Enforce), true).await;
    let mut c = Client::connect(&srv.addr, true).await;
    c.publish("legacy", "t.x").await;
    assert!(c.closed().await);
    assert!(!srv.noise_pub.is_empty());

    let srv = server(crypto(MeshAdmissionMode::Observe), true).await;
    let mut c = Client::connect(&srv.addr, true).await;
    c.publish("legacy", "t.x").await;
    assert!(wait_for(&srv.rec, "t.x").await);
}

#[path = "mesh_admit_fix_tests.rs"]
mod fix;
