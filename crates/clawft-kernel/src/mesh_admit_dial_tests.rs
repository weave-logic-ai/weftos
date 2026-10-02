//! P3-K1: outbound hello. Two in-process runtimes in `enforce` mode over
//! Noise admit each other, and a dialer with no identity is refused.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use clawft_types::config::MeshAdmissionMode;

use super::*;
use crate::error::KernelResult;
use crate::ipc::{KernelMessage, MessageTarget};
use crate::mesh::MeshTransport;
use crate::mesh_delivery::{LocalDelivery, PeerCtx};
use crate::mesh_ipc::{MeshIpcEnvelope, Scope};
use crate::mesh_noise::{noise_static_public, NoiseConfig, NoisePattern};
use crate::mesh_runtime::MeshRuntime;
use crate::mesh_serve::{connect_seeds, serve_listener};
use crate::revocation::RevocationList;

const GENESIS: [u8; 32] = [7u8; 32];

#[derive(Default)]
struct Recorder(Mutex<Vec<(String, bool)>>);

#[async_trait]
impl LocalDelivery for Recorder {
    async fn deliver(&self, from: &PeerCtx, _: Option<&Scope>, msg: KernelMessage) -> KernelResult<()> {
        if let MessageTarget::Topic(t) = msg.target {
            self.0.lock().unwrap().push((t, from.node_verified));
        }
        Ok(())
    }
}

struct Node {
    rt: Arc<MeshRuntime>,
    rec: Arc<Recorder>,
    id: String,
    addr: String,
    noise: Arc<NoiseConfig>,
    identity: Arc<DialIdentity>,
    _task: tokio::task::JoinHandle<()>,
}

async fn node(seed: u8, mode: MeshAdmissionMode) -> Node {
    let key = SigningKey::from_bytes(&[seed; 32]);
    let id = node_id_from_pubkey(&key.verifying_key().to_bytes());
    let rec = Arc::new(Recorder::default());
    let mut rt = MeshRuntime::new(id.clone());
    rt.set_local_delivery(rec.clone());
    let rt = Arc::new(rt);
    let rev = Arc::new(RevocationList::new(tempfile::tempdir().unwrap().keep().join("r.json")));
    let gate = Arc::new(CryptoGate::new(GENESIS, rev, Arc::new(OpenVerdicts), mode));
    let kp = snow::Builder::new("Noise_XX_25519_ChaChaPoly_SHA256".parse().unwrap())
        .generate_keypair()
        .unwrap();
    let noise = Arc::new(NoiseConfig {
        pattern: NoisePattern::XX,
        local_private_key: kp.private.try_into().unwrap(),
        remote_static_key: None,
    });
    assert_eq!(noise_static_public(&noise.local_private_key).unwrap().to_vec(), kp.public);
    let listener = crate::mesh_tcp::TcpTransport.listen("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let task = tokio::spawn(serve_listener(
        Arc::clone(&rt),
        listener,
        Some(Arc::clone(&noise)),
        "tcp",
        "x",
        gate,
    ));
    let identity = Arc::new(DialIdentity {
        key,
        genesis: GENESIS,
        platform: "test".into(),
        capabilities: vec![],
    });
    Node { rt, rec, id, addr, noise, identity, _task: task }
}

/// Push `topic` from `from` to the peer it dialled at `to_addr`.
async fn push(from: &Node, to: &Node, topic: &str) {
    for _ in 0..100 {
        let msg = KernelMessage::text(0, MessageTarget::Topic(topic.into()), "x");
        let env = MeshIpcEnvelope::new(from.id.clone(), to.id.clone(), msg);
        if from.rt.send_to_peer(&to.addr, env).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("dial to {} never registered", to.addr);
}

async fn got(n: &Node, topic: &str) -> Option<bool> {
    for _ in 0..150 {
        if let Some((_, v)) = n.rec.0.lock().unwrap().iter().find(|(t, _)| t == topic) {
            return Some(*v);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

#[tokio::test]
async fn two_enforce_nodes_admit_each_other_over_noise() {
    let a = node(1, MeshAdmissionMode::Enforce).await;
    let b = node(2, MeshAdmissionMode::Enforce).await;
    connect_seeds(&a.rt, &[b.addr.clone()], "tcp", Some(a.noise.clone()), Some(a.identity.clone()));
    connect_seeds(&b.rt, &[a.addr.clone()], "tcp", Some(b.noise.clone()), Some(b.identity.clone()));
    push(&a, &b, "a.to.b").await;
    push(&b, &a, "b.to.a").await;
    assert_eq!(got(&b, "a.to.b").await, Some(true), "B must serve A as a verified peer");
    assert_eq!(got(&a, "b.to.a").await, Some(true), "A must serve B as a verified peer");
}

#[tokio::test]
async fn enforce_refuses_a_dialer_that_sends_no_hello() {
    let a = node(1, MeshAdmissionMode::Enforce).await;
    let b = node(2, MeshAdmissionMode::Enforce).await;
    connect_seeds(&a.rt, &[b.addr.clone()], "tcp", Some(a.noise.clone()), None);
    push(&a, &b, "no.hello").await;
    assert_eq!(got(&b, "no.hello").await, None);
}

#[tokio::test]
async fn observe_serves_a_dialer_that_sends_no_hello() {
    let a = node(1, MeshAdmissionMode::Observe).await;
    let b = node(2, MeshAdmissionMode::Observe).await;
    connect_seeds(&a.rt, &[b.addr.clone()], "tcp", Some(a.noise.clone()), None);
    push(&a, &b, "legacy").await;
    assert_eq!(got(&b, "legacy").await, Some(false));
}
