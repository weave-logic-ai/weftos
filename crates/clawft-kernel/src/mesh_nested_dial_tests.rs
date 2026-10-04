//! D10 regression probes: a pinned seed address/name is not authenticated identity.
use super::*;
use crate::ipc::{KernelMessage, MessageTarget};
use crate::mesh::MeshTransport;
use crate::mesh_admit::{CryptoGate, DialIdentity, OpenVerdicts};
use crate::mesh_delivery::{LocalDelivery, PeerCtx};
use crate::mesh_ipc::Scope;
use crate::mesh_noise::NoisePattern;
use crate::mesh_runtime::MeshAuthentication;
use clawft_types::config::MeshAdmissionMode;
use ed25519_dalek::SigningKey;
use std::sync::Mutex;
use std::time::Duration;

const GENESIS: [u8; 32] = [44; 32];
#[derive(Default)]
struct Received(Mutex<Vec<bool>>);
#[async_trait::async_trait]
impl LocalDelivery for Received {
    async fn deliver(
        &self,
        peer: &PeerCtx,
        _: Option<&Scope>,
        _: KernelMessage,
    ) -> crate::error::KernelResult<()> {
        self.0.lock().unwrap().push(peer.node_verified);
        Ok(())
    }
}

fn scratch() -> tempfile::TempDir {
    let path = std::env::var_os("D10_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/test-runs/d10")
        });
    std::fs::create_dir_all(&path).unwrap();
    tempfile::tempdir_in(path).unwrap()
}

fn identity(seed: u8) -> Arc<DialIdentity> {
    Arc::new(DialIdentity {
        key: SigningKey::from_bytes(&[seed; 32]),
        genesis: GENESIS,
        platform: "d10-test".into(),
        capabilities: Vec::new(),
    })
}
fn noise(seed: u8) -> Arc<NoiseConfig> {
    Arc::new(NoiseConfig {
        pattern: NoisePattern::XX,
        local_private_key: [seed; 32],
        remote_static_key: None,
    })
}
fn id(key: &DialIdentity) -> String {
    crate::node_id_from_pubkey(&key.key.verifying_key().to_bytes())
}
fn gate(path: &std::path::Path) -> Arc<dyn AdmissionGate> {
    Arc::new(CryptoGate::new(
        GENESIS,
        Arc::new(crate::revocation::RevocationList::new(
            path.join("revocations.json"),
        )),
        Arc::new(OpenVerdicts),
        MeshAdmissionMode::Enforce,
    ))
}
fn runtime(
    key: &Arc<DialIdentity>,
    noise: &Arc<NoiseConfig>,
    gate: Arc<dyn AdmissionGate>,
    rec: &Arc<Received>,
) -> Arc<MeshRuntime> {
    let mut rt = MeshRuntime::new(id(key));
    rt.set_local_delivery(rec.clone());
    let rt = Arc::new(rt);
    rt.set_enforcing(true);
    assert!(rt.set_authentication(MeshAuthentication {
        gate,
        identity: key.clone(),
        noise_static: noise_static_public(&noise.local_private_key).unwrap(),
        require_authenticated_seeds: true
    }));
    rt
}

#[tokio::test]
async fn nested_seed_requires_reciprocal_proof_before_routing() {
    // Missing hello, another key claiming the configured ID, a valid hello for
    // another identity, and a copied identity bound to another Noise static key.
    // Under the old AllowAll dialer the subsequent matching envelope was delivered.
    for attack in 0..4 {
        let files = scratch();
        let a = identity(11);
        let b = identity(12);
        let wrong = identity(13);
        let na = noise(21);
        let nb = noise(22);
        let rec = Arc::new(Received::default());
        let rt = runtime(&a, &na, gate(files.path()), &rec);
        let expected = id(&b);
        let mut listener = crate::mesh_tcp::TcpTransport
            .listen("127.0.0.1:0")
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let expected_server = expected.clone();
        let local_id = id(&a);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut channel = NoiseChannel::respond(stream, &nb).await.unwrap();
            let intro = channel.recv_encrypted().await.unwrap();
            assert!(AdmitHello::parse_frame(&intro).unwrap().is_ok());
            let envelope = MeshIpcEnvelope::new(
                expected_server.clone(),
                local_id,
                KernelMessage::text(0, MessageTarget::Topic("forged".into()), "must not route"),
            )
            .to_bytes()
            .unwrap();
            let frame = match attack {
                0 => envelope.clone(),
                1 => {
                    let mut hello = wrong.hello(
                        channel.handshake_hash(),
                        &noise_static_public(&nb.local_private_key).unwrap(),
                        unix_now(),
                    );
                    hello.node_id = expected_server;
                    hello.to_bytes()
                }
                2 => wrong
                    .hello(
                        channel.handshake_hash(),
                        &noise_static_public(&nb.local_private_key).unwrap(),
                        unix_now(),
                    )
                    .to_bytes(),
                _ => b
                    .hello(
                        channel.handshake_hash(),
                        &noise_static_public(&[99; 32]).unwrap(),
                        unix_now(),
                    )
                    .to_bytes(),
            };
            channel.send_encrypted(&frame).await.unwrap();
            if attack != 0 {
                let _ = channel.send_encrypted(&envelope).await;
            }
            // Keep the socket alive long enough to expose the old pump delivery.
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        tokio::time::timeout(
            Duration::from_secs(3),
            dial_seed_once(
                &rt,
                &addr,
                Some(&expected),
                "tcp",
                Some(na),
                Some(a),
                Duration::from_millis(50),
            ),
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert!(
            rec.0.lock().unwrap().is_empty(),
            "attack {attack} delivered an unauthenticated envelope"
        );
        assert!(
            rt.peer_ids().is_empty(),
            "attack {attack} left an authenticated-looking route"
        );
    }
}

#[tokio::test]
async fn nested_seed_reciprocal_admission_is_verified_and_bidirectional() {
    let files_a = scratch();
    let files_b = scratch();
    let a = identity(31);
    let b = identity(32);
    let na = noise(41);
    let nb = noise(42);
    let ra = Arc::new(Received::default());
    let rb = Arc::new(Received::default());
    let ga = gate(files_a.path());
    let gb = gate(files_b.path());
    let rta = runtime(&a, &na, ga, &ra);
    let rtb = runtime(&b, &nb, gb.clone(), &rb);
    let listener = crate::mesh_tcp::TcpTransport
        .listen("127.0.0.1:0")
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(serve_listener(
        rtb.clone(),
        listener,
        Some(nb),
        "tcp",
        "test",
        gb,
    ));
    let dials = connect_seeds(
        &rta,
        &[format!("{addr}#{}", id(&b))],
        "tcp",
        Some(na),
        Some(a.clone()),
    );
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        while !rta.peer_ids().contains(&id(&b)) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        rta.send_to_peer(
            &id(&b),
            MeshIpcEnvelope::new(
                id(&a),
                id(&b),
                KernelMessage::text(0, MessageTarget::Topic("forward".into()), "x"),
            ),
        )
        .await
        .unwrap();
        while rb.0.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        rtb.send_to_peer(
            &id(&a),
            MeshIpcEnvelope::new(
                id(&b),
                id(&a),
                KernelMessage::text(0, MessageTarget::Topic("return".into()), "x"),
            ),
        )
        .await
        .unwrap();
        while ra.0.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(ra.0.lock().unwrap().iter().all(|v| *v));
        assert!(rb.0.lock().unwrap().iter().all(|v| *v));
    })
    .await;
    for dial in dials {
        dial.abort();
    }
    server.abort();
    result.expect("reciprocal authenticated traffic never became routable");
}
