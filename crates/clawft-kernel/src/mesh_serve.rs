//! Mesh accept loop and seed-connect loop (P3-K0).
//!
//! Moved out of `boot.rs` unchanged in behaviour so a machine mesh
//! service can reuse them. [`serve_listener`] owns the per-connection
//! Noise responder handshake and the bidirectional pump;
//! [`connect_seeds`] dials the configured seed peers as Noise initiator.

use std::sync::Arc;

use crate::ipc::MessageTarget;
use crate::mesh::{MeshTransport, TransportListener};
use crate::mesh_admit::{
    AdmitContext, AdmitHello, Admission, DialIdentity, AdmissionGate, ChannelBinding, ChannelKind, HelloFailure,
    PeerClass, PeerLimits, Refusal,
};
use crate::mesh_assess::{AssessmentEnvelope, AssessmentTransport};
use crate::mesh_delivery::PeerCtx;
use crate::mesh_ipc::MeshIpcEnvelope;
use crate::mesh_limits::{IpSlot, Limits, HANDSHAKE_TIMEOUT, MAX_CONNECTIONS};
use crate::mesh_noise::{
    noise_static_public, EncryptedChannel, NoiseChannel, NoiseConfig, PassthroughChannel,
};
use crate::mesh_runtime::MeshRuntime;

/// Build the transport for a `kernel.mesh.transport` name.
///
/// `seed_peer` is set when selecting for an outbound seed connection; it
/// only changes the wording of the QUIC-unavailable fallback log.
pub fn transport_for(name: &str, seed_peer: Option<&str>) -> Box<dyn MeshTransport> {
    match name {
        "ws" | "websocket" => Box::new(crate::mesh_ws::WsTransport),
        #[cfg(feature = "quic")]
        "quic" => Box::new(crate::mesh_quic::QuicTransport),
        #[cfg(not(feature = "quic"))]
        "quic" => {
            match seed_peer {
                Some(peer) => tracing::error!(
                    peer = %peer,
                    "seed peer uses quic but binary built without `quic` feature; falling back to tcp"
                ),
                None => tracing::error!(
                    "mesh transport=quic requested but clawft-kernel built without `quic` feature; falling back to tcp"
                ),
            }
            Box::new(crate::mesh_tcp::TcpTransport)
        }
        _ => {
            let _ = seed_peer;
            Box::new(crate::mesh_tcp::TcpTransport)
        }
    }
}

/// Accept connections on `listener` forever, pumping each through
/// `runtime`. Spawn this on a task; it never returns.
///
/// `listen_addr` is only the fallback for the "listener started" log when
/// the listener cannot report its bound address. `gate` decides, on each
/// connection's first frame, whether the peer is admitted (P3-K1);
/// [`AllowAll`](crate::mesh_admit::AllowAll) keeps the pre-admission
/// behaviour.
pub async fn serve_listener(
    runtime: Arc<MeshRuntime>,
    listener: Box<dyn TransportListener>,
    noise: Option<Arc<NoiseConfig>>,
    transport_name: &str,
    listen_addr: &str,
    gate: Arc<dyn AdmissionGate>,
) {
    serve_listener_with(
        runtime,
        listener,
        noise,
        transport_name,
        listen_addr,
        gate,
        Limits::default(),
    )
    .await;
}

/// [`serve_listener`] with explicit [`Limits`] (tests use short ones).
pub async fn serve_listener_with(
    runtime: Arc<MeshRuntime>,
    mut listener: Box<dyn TransportListener>,
    noise: Option<Arc<NoiseConfig>>,
    transport_name: &str,
    listen_addr: &str,
    gate: Arc<dyn AdmissionGate>,
    limits: Limits,
) {
    let bind = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| listen_addr.to_owned());
    tracing::info!(
        transport = transport_name,
        addr = %bind,
        noise = noise.is_some(),
        "mesh listener started"
    );

    // Connection tasks live in a JoinSet so aborting this future (dropping
    // the set) also stops every connection it spawned, and a semaphore caps
    // concurrent connections.
    let mut conns = tokio::task::JoinSet::new();
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    let per_ip = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let strict = gate.strict();
    loop {
        while conns.try_join_next().is_some() {}
        match listener.accept().await {
            Ok((stream, peer_addr)) => {
                let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
                    tracing::warn!(peer = %peer_addr, max = MAX_CONNECTIONS,
                        "mesh connection cap reached, dropping");
                    continue;
                };
                let ip_slot = if strict {
                    let Some(slot) = IpSlot::acquire(&per_ip, peer_addr.ip(), limits.per_ip) else {
                        tracing::warn!(peer = %peer_addr, max = limits.per_ip,
                            "mesh per-IP connection cap reached, dropping");
                        continue;
                    };
                    Some(slot)
                } else {
                    None
                };
                let rt = Arc::clone(&runtime);
                let nc = noise.clone();
                let gate = Arc::clone(&gate);
                conns.spawn(async move {
                    serve_connection(rt, stream, peer_addr, nc, gate, limits).await;
                    drop(permit);
                    drop(ip_slot);
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "mesh accept error");
            }
        }
    }
}

/// What admission granted this connection.
pub(crate) struct Active {
    /// Verified node id; later envelopes must carry it as `source_node`.
    pub(crate) bound: Option<String>,
    pub(crate) limits: PeerLimits,
    pub(crate) trust_scope: bool,
    /// Admitted = hello verified AND the gate enforced AND said Admit.
    /// `bound` alone is only key possession, not membership.
    pub(crate) admitted: bool,
    pub(crate) class: PeerClass,
    pub(crate) remote_static: Option<Vec<u8>>,
}

impl Active {
    /// The connection identity handed to the runtime and to delivery.
    fn peer_ctx(&self) -> PeerCtx {
        PeerCtx {
            peer_id: self.bound.clone().unwrap_or_default(),
            node_verified: self.admitted,
            class: self.class,
            remote_static: self.remote_static.clone(),
            src_scope: None,
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Run admission on a connection's first frame. Returns the terms and the
/// frame to process normally (`None` when the frame was the hello).
async fn admit_first_frame(
    channel: &dyn EncryptedChannel,
    kind: ChannelKind,
    gate: &dyn AdmissionGate,
    data: Vec<u8>,
) -> Result<(Active, Option<Vec<u8>>), Refusal> {
    let legacy = AdmitContext { class: PeerClass::Legacy, channel: kind };
    let binding = channel.handshake_hash().map(|h| ChannelBinding {
        handshake_hash: h,
        remote_static: channel.remote_static_key(),
    });
    let (decision, bound, rest) = match AdmitHello::parse_frame(&data) {
        None => (
            gate.admit_unverified(&HelloFailure::Missing, &legacy).await,
            None,
            Some(data),
        ),
        Some(Err(f)) => (gate.admit_unverified(&f, &legacy).await, None, None),
        Some(Ok(hello)) => match hello.verify(binding.as_ref(), unix_now()) {
            Ok(v) => {
                let ctx = AdmitContext { class: v.class(), channel: kind };
                (gate.admit(&v, &ctx).await, Some(v.node_id), None)
            }
            Err(f) => (gate.admit_unverified(&f, &legacy).await, None, None),
        },
    };
    match decision {
        Admission::Refuse(r) => Err(r),
        Admission::Admit(g) => Ok((
            Active {
                admitted: g.admitted && bound.is_some(),
                trust_scope: g.trust_scope && g.admitted && bound.is_some(),
                limits: g.limits,
                bound,
                class: g.class,
                remote_static: channel.remote_static_key().map(<[u8]>::to_vec),
            },
            rest,
        )),
    }
}

/// Apply post-admission rules to one frame. `None` drops it.
///
/// - `source_node` must equal the verified node id (envelopes and
///   assessment frames alike).
/// - Leaf limits: only `substrate/<own-id>/...` topics and `mesh.subscribe`.
/// - `src_scope` is stripped unless admission verified the peer.
pub(crate) fn screen_frame(data: Vec<u8>, act: &Active) -> Option<Vec<u8>> {
    match MeshIpcEnvelope::from_bytes(&data) {
        Ok(mut env) => {
            if let Some(id) = &act.bound {
                if &env.source_node != id {
                    tracing::warn!(claimed = %env.source_node, verified = %id,
                        "dropping envelope: source_node differs from admitted node id");
                    return None;
                }
            }
            if act.limits == PeerLimits::Leaf {
                let id = act.bound.as_deref().unwrap_or_default();
                let ok = matches!(&env.message.target, MessageTarget::Topic(t)
                    if t == "mesh.subscribe" || t.starts_with(&format!("substrate/{id}/")));
                if !ok {
                    tracing::warn!(node = id, "dropping leaf envelope outside substrate/<id>/");
                    return None;
                }
            }
            if env.src_scope.is_some() && !act.trust_scope {
                env.src_scope = None;
                return env.to_bytes().ok();
            }
            Some(data)
        }
        Err(_) => {
            if act.limits == PeerLimits::Leaf {
                return None;
            }
            if let Some(id) = &act.bound {
                let src = AssessmentTransport::try_extract_payload(&data)
                    .ok()
                    .flatten()
                    .and_then(|p| AssessmentEnvelope::from_bytes(&p).ok())
                    .map(|e| e.source_node);
                if src.is_some_and(|s| &s != id) {
                    tracing::warn!(verified = %id, "dropping assessment frame: source_node mismatch");
                    return None;
                }
            }
            Some(data)
        }
    }
}

/// One accepted connection: optional Noise responder handshake, admission
/// on the first frame, then the bidirectional pump until either side
/// closes.
async fn serve_connection(
    rt: Arc<MeshRuntime>,
    stream: Box<dyn crate::mesh::MeshStream>,
    peer_addr: std::net::SocketAddr,
    nc: Option<Arc<NoiseConfig>>,
    gate: Arc<dyn AdmissionGate>,
    limits: Limits,
) {
    tracing::info!(
        peer = %peer_addr,
        noise = nc.is_some(),
        "mesh peer connected"
    );

    // Optionally wrap in Noise encryption.
    let mut channel: Box<dyn EncryptedChannel> = match &nc {
        Some(cfg) => match tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            NoiseChannel::respond(stream, cfg),
        )
        .await
        .unwrap_or_else(|_| Err(crate::mesh::MeshError::Handshake("handshake timed out".into())))
        {
            Ok(ch) => {
                tracing::info!(peer = %peer_addr, "noise handshake complete");
                Box::new(ch)
            }
            Err(e) => {
                tracing::warn!(peer = %peer_addr, error = %e, "noise handshake failed, dropping");
                return;
            }
        },
        None => Box::new(PassthroughChannel::new(stream)),
    };
    let kind = if nc.is_some() { ChannelKind::Noise } else { ChannelKind::Passthrough };

    // Outbound channel: the kernel pushes frames into `out_tx` (via
    // `MeshRuntime::send_to_peer`) and this task drains `out_rx` back
    // through the encrypted stream. This is what lets the topic forwarder
    // in `A2ARouter` deliver pushes to inbound leaf peers that subscribed
    // via `mesh.subscribe`.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

    // Admission state. Nothing from the peer reaches the runtime until the
    // first frame has been through `admit_first_frame`.
    let mut active: Option<Active> = None;

    // Bidirectional loop. `handle_incoming_from` auto-registers the peer
    // by `envelope.source_node` on first arrival so the kernel can route
    // back.
    //
    // Cancel-safety (ADR-010 / WEFT-18): both racing futures must be
    // cancel-safe.
    // - `out_rx.recv()` — tokio mpsc, cancel-safe.
    // - `channel.recv_encrypted()` → `MeshStream::recv` — contract on
    //   `MeshStream` requires cancel-safe framing (`TcpMeshStream` keeps
    //   partial progress).
    // Losing either race must not drop a complete message already removed
    // from its source, and must not desync the TCP length-prefix stream.
    // Admission runs in the handler of a completed `recv`, never inside a
    // raced future.
    let strict = gate.strict();
    loop {
        // Strict gates bound how long a peer may stay silent: briefly before
        // the first frame, longer once admitted. Lenient gates keep the
        // pre-admission behaviour (no limit) so slow legacy leaves still work.
        let limit = strict
            .then(|| if active.is_none() { limits.first_frame } else { limits.idle });
        let timer = async {
            match limit {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = timer => {
                tracing::warn!(peer = %peer_addr, "mesh connection timed out, dropping");
                break;
            }
            inbound = channel.recv_encrypted() => match inbound {
                Ok(data) => {
                    let frame = match active {
                        Some(_) => Some(data),
                        None => match admit_first_frame(&*channel, kind, &*gate, data).await {
                            Ok((act, rest)) => {
                                tracing::info!(peer = %peer_addr,
                                    node = act.bound.as_deref().unwrap_or("-"), "mesh peer admitted");
                                active = Some(act);
                                rest
                            }
                            Err(r) => {
                                tracing::warn!(peer = %peer_addr, code = r.code, detail = %r.detail,
                                    "mesh peer refused, dropping");
                                break;
                            }
                        },
                    };
                    let Some(frame) = frame else { continue };
                    let Some(act) = active.as_ref() else { break };
                    if let Some(frame) = screen_frame(frame, act) {
                        let ctx = act.peer_ctx();
                        if let Err(e) = rt.handle_incoming_peer(&frame, out_tx.clone(), Some(&ctx)).await {
                            tracing::debug!(error = %e, "mesh message handling error");
                        }
                    }
                }
                Err(_) => break,
            },
            outbound = out_rx.recv() => match outbound {
                Some(data) => {
                    if channel.send_encrypted(&data).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
        }
    }
    // Drop our route if it still points at this connection (a verified
    // route must not outlive its connection).
    rt.disconnect_channel(&out_tx);
}

/// Dial each seed peer in its own task (Noise initiator when `noise` is
/// set) and register it with `runtime`. Returns immediately.
pub fn connect_seeds(
    runtime: &Arc<MeshRuntime>,
    seed_peers: &[String],
    transport_name: &str,
    noise: Option<Arc<NoiseConfig>>,
    identity: Option<Arc<DialIdentity>>,
) {
    for peer_addr in seed_peers {
        let addr = peer_addr.clone();
        let rt = Arc::clone(runtime);
        let transport_name = transport_name.to_owned();
        let nc = noise.clone();
        let identity = identity.clone();
        tokio::spawn(async move {
            let transport = transport_for(&transport_name, Some(&addr));

            match transport.connect(&addr).await {
                Ok(stream) => {
                    // Optionally wrap in Noise encryption (as initiator).
                    let mut channel: Box<dyn EncryptedChannel> = match &nc {
                        Some(cfg) => match NoiseChannel::initiate(stream, cfg).await {
                            Ok(ch) => {
                                tracing::info!(peer = %addr, "noise handshake complete (initiator)");
                                Box::new(ch)
                            }
                            Err(e) => {
                                tracing::warn!(peer = %addr, error = %e, "noise handshake failed");
                                return;
                            }
                        },
                        None => Box::new(PassthroughChannel::new(stream)),
                    };

                    // Introduce ourselves before anything else is sent. Only a
                    // Noise session has a handshake hash to bind the hello to;
                    // plaintext dials send nothing (the far side's gate decides
                    // whether that is acceptable).
                    if let (Some(id), Some(cfg), Some(hash)) =
                        (&identity, &nc, channel.handshake_hash().map(<[u8]>::to_vec))
                    {
                        match noise_static_public(&cfg.local_private_key) {
                            Some(stat) => {
                                let hello = id.hello(&hash, &stat, unix_now());
                                if channel.send_encrypted(&hello.to_bytes()).await.is_err() {
                                    tracing::warn!(peer = %addr, "failed to send admission hello");
                                    return;
                                }
                            }
                            None => tracing::warn!(peer = %addr, "cannot derive noise static key"),
                        }
                    }

                    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
                    let peer_id = addr.clone();
                    rt.add_peer(peer_id.clone(), tx);
                    tracing::info!(peer = %addr, noise = nc.is_some(), "connected to seed peer");

                    // Drain outbound queue through encrypted channel.
                    tokio::spawn(async move {
                        while let Some(data) = rx.recv().await {
                            if channel.send_encrypted(&data).await.is_err() {
                                break;
                            }
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(peer = %addr, error = %e, "failed to connect to seed peer");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::KernelResult;
    use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
    use crate::mesh_delivery::{LocalDelivery, PeerCtx};
    use crate::mesh_ipc::{MeshIpcEnvelope, Scope};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        got: Mutex<Vec<(Option<Scope>, KernelMessage)>>,
    }

    #[async_trait::async_trait]
    impl LocalDelivery for Recorder {
        async fn deliver(
            &self,
            _from: &PeerCtx,
            scope: Option<&Scope>,
            msg: KernelMessage,
        ) -> KernelResult<()> {
            self.got.lock().unwrap().push((scope.cloned(), msg));
            Ok(())
        }
    }

    #[tokio::test]
    async fn serve_listener_delivers_scoped_envelope_to_local_delivery() {
        let rec = Arc::new(Recorder::default());
        let mut rt = MeshRuntime::new("node-b".into());
        rt.set_local_delivery(rec.clone());
        let rt = Arc::new(rt);

        let transport = crate::mesh_tcp::TcpTransport;
        let listener = transport.listen("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(serve_listener(
            Arc::clone(&rt),
            listener,
            None,
            "tcp",
            "127.0.0.1:0",
            Arc::new(crate::mesh_admit::AllowAll),
        ));

        let mut client = transport.connect(&addr.to_string()).await.unwrap();
        let msg = KernelMessage::text(0, MessageTarget::Topic("t.scoped".into()), "hi");
        let mut env = MeshIpcEnvelope::new("node-a".into(), "node-b".into(), msg);
        env.dest_scope = Some(Scope {
            user_id: "a".repeat(32),
            project_id: None,
        });
        client.send(&env.to_bytes().unwrap()).await.unwrap();

        for _ in 0..100 {
            if !rec.got.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        server.abort();

        let got = rec.got.lock().unwrap();
        assert_eq!(got.len(), 1, "envelope not delivered");
        assert_eq!(got[0].0.as_ref().unwrap().user_id, "a".repeat(32));
        assert!(matches!(&got[0].1.payload, MessagePayload::Text(s) if s == "hi"));
        // The accept loop registered the sender by source_node.
        assert!(rt.peer_ids().contains(&"node-a".to_string()));
    }
}
