//! Mesh accept loop and seed-connect loop (P3-K0).
//!
//! Moved out of `boot.rs` unchanged in behaviour so a machine mesh
//! service can reuse them. [`serve_listener`] owns the per-connection
//! Noise responder handshake and the bidirectional pump;
//! [`connect_seeds`] dials the configured seed peers as Noise initiator.

use std::sync::Arc;

use crate::mesh::{MeshTransport, TransportListener};
use crate::mesh_noise::{EncryptedChannel, NoiseChannel, NoiseConfig, PassthroughChannel};
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
/// the listener cannot report its bound address.
pub async fn serve_listener(
    runtime: Arc<MeshRuntime>,
    mut listener: Box<dyn TransportListener>,
    noise: Option<Arc<NoiseConfig>>,
    transport_name: &str,
    listen_addr: &str,
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

    loop {
        match listener.accept().await {
            Ok((stream, peer_addr)) => {
                let rt = Arc::clone(&runtime);
                let nc = noise.clone();
                tokio::spawn(async move {
                    serve_connection(rt, stream, peer_addr, nc).await;
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "mesh accept error");
            }
        }
    }
}

/// One accepted connection: optional Noise responder handshake, then the
/// bidirectional pump until either side closes.
async fn serve_connection(
    rt: Arc<MeshRuntime>,
    stream: Box<dyn crate::mesh::MeshStream>,
    peer_addr: std::net::SocketAddr,
    nc: Option<Arc<NoiseConfig>>,
) {
    tracing::info!(
        peer = %peer_addr,
        noise = nc.is_some(),
        "mesh peer connected"
    );

    // Optionally wrap in Noise encryption.
    let mut channel: Box<dyn EncryptedChannel> = match &nc {
        Some(cfg) => match NoiseChannel::respond(stream, cfg).await {
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

    // Outbound channel: the kernel pushes frames into `out_tx` (via
    // `MeshRuntime::send_to_peer`) and this task drains `out_rx` back
    // through the encrypted stream. This is what lets the topic forwarder
    // in `A2ARouter` deliver pushes to inbound leaf peers that subscribed
    // via `mesh.subscribe`.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

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
    loop {
        tokio::select! {
            inbound = channel.recv_encrypted() => match inbound {
                Ok(data) => {
                    if let Err(e) = rt.handle_incoming_from(&data, out_tx.clone()).await {
                        tracing::debug!(error = %e, "mesh message handling error");
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
}

/// Dial each seed peer in its own task (Noise initiator when `noise` is
/// set) and register it with `runtime`. Returns immediately.
pub fn connect_seeds(
    runtime: &Arc<MeshRuntime>,
    seed_peers: &[String],
    transport_name: &str,
    noise: Option<Arc<NoiseConfig>>,
) {
    for peer_addr in seed_peers {
        let addr = peer_addr.clone();
        let rt = Arc::clone(runtime);
        let transport_name = transport_name.to_owned();
        let nc = noise.clone();
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
    use crate::mesh_delivery::LocalDelivery;
    use crate::mesh_ipc::{MeshIpcEnvelope, Scope};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        got: Mutex<Vec<(Option<Scope>, KernelMessage)>>,
    }

    #[async_trait::async_trait]
    impl LocalDelivery for Recorder {
        async fn deliver(&self, scope: Option<&Scope>, msg: KernelMessage) -> KernelResult<()> {
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
