//! The mesh side of the stable inference address in a daemon that owns its
//! mesh connections (collapsed or legacy mode): adverts and forwarded
//! requests travel as control messages on [`INFER_TOPIC`], handed to the
//! hub with the connection's own [`PeerCtx`].
//!
//! Everything that decides trust comes from that context, never from the
//! payload: the sender of an advert is `ctx.peer_id`, the standing of a peer
//! is the grant its connection holds ([`PeerCtx::grant`]), and a served
//! request is checked against it by [`serve_infer`]. The hub never runs in
//! service mode: there the daemon is handed deliveries as an unverified peer
//! and holds no grant for anyone (see `mesh_local_sink`), so nothing could be
//! served or ingested safely.
//!
//! Each forwarded request is one exchange keyed by `(peer, id)`; the
//! exchange is presented to [`forward_remote`] and [`serve_infer`] as a
//! [`MeshStream`], so the forwarding code is the same one the tests drive
//! over in-memory streams.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::{Value, json};
use tokio::sync::{Semaphore, mpsc};

use super::mesh_forward::{InferPeer, ServeGate, serve_infer};
use super::table::PlacementTable;
use super::types::{MeshDialer, ProxyAudit, ProxyError, ProxyLimits, qualifies};
use super::upstream::Upstream;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh::{MeshError, MeshStream};
use crate::mesh_admit::{Grant, hex_decode, hex_encode};
use crate::mesh_delivery::PeerCtx;
use crate::mesh_framing::{FrameType, MeshFrame};
use crate::mesh_runtime::{INFER_TOPIC, MeshRuntime, PeerControlSink};
use crate::mesh_service_adv::ServiceAdvertisement;

/// Responses buffered per exchange before the exchange is cancelled.
const RESP_BUFFER: usize = 64;
/// Requests being taken in at once, over all peers.
const INTAKE: usize = 32;

type Pending = Arc<DashMap<(String, u64), mpsc::Sender<Vec<u8>>>>;

/// Adverts, forwarded requests and their responses between peers.
pub struct InferHub {
    rt: Arc<MeshRuntime>,
    table: OnceLock<Arc<PlacementTable>>,
    upstream: Arc<Upstream>,
    gate: Arc<ServeGate>,
    audit: Option<Arc<dyn ProxyAudit>>,
    grants: DashMap<String, Grant>,
    pending: Pending,
    next_id: AtomicU64,
    intake: Arc<Semaphore>,
    limits: ProxyLimits,
    me: OnceLock<std::sync::Weak<Self>>,
}

fn msg(value: Value) -> KernelMessage {
    KernelMessage::new(
        0,
        MessageTarget::Topic(INFER_TOPIC.to_string()),
        MessagePayload::Json(value),
    )
}

fn mesh_err(e: impl std::fmt::Display) -> MeshError {
    MeshError::Transport(e.to_string())
}

/// `[len][type][payload]` -> `(type, payload)`.
fn split_frame(data: &[u8]) -> Result<MeshFrame, MeshError> {
    if data.len() < 5 {
        return Err(mesh_err("short frame"));
    }
    MeshFrame::decode(&data[4..])
}

impl InferHub {
    /// A hub over `rt`. Call [`attach`](Self::attach) with the table built
    /// around it, then [`install`](Self::install).
    pub fn new(
        rt: Arc<MeshRuntime>,
        upstream: Arc<Upstream>,
        gate: Arc<ServeGate>,
        audit: Option<Arc<dyn ProxyAudit>>,
    ) -> Arc<Self> {
        let limits = upstream.limits().clone();
        let hub = Arc::new(Self {
            rt,
            table: OnceLock::new(),
            upstream,
            gate,
            audit,
            grants: DashMap::new(),
            pending: Arc::new(DashMap::new()),
            next_id: AtomicU64::new(1),
            intake: Arc::new(Semaphore::new(INTAKE)),
            limits,
            me: OnceLock::new(),
        });
        let _ = hub.me.set(Arc::downgrade(&hub));
        hub
    }

    /// The placement table this hub serves and feeds.
    pub fn attach(&self, table: Arc<PlacementTable>) {
        let _ = self.table.set(table);
    }

    /// Register as the runtime's control sink for [`INFER_TOPIC`].
    pub fn install(self: &Arc<Self>) {
        self.rt.set_control_sink(INFER_TOPIC, self.clone());
    }

    fn table(&self) -> Option<&Arc<PlacementTable>> {
        self.table.get()
    }

    fn note(&self, kind: &str, payload: Value) {
        if let Some(a) = &self.audit {
            a.record(kind, payload);
        }
    }

    /// Send this node's adverts for every exposed role to each connected
    /// peer that is allowed to use it and qualifies. Called periodically;
    /// receivers expire adverts they stop hearing.
    pub async fn announce(&self, now_secs: u64) {
        // A hello to every connected peer lets each side learn the standing
        // the other's connection holds (grants are only known from traffic).
        for peer in self.rt.peer_ids() {
            let _ = self.rt.route_to_remote(&peer, msg(json!({"t": "hello"}))).await;
        }
        let Some(table) = self.table() else { return };
        for ad in table.advertisements(now_secs) {
            let Some(role) = ad.name.strip_prefix(super::table::SERVICE_PREFIX) else {
                continue;
            };
            for peer in self.rt.peer_ids() {
                let ok = table.mesh_peer_allowed(role, &peer)
                    && self.grants.get(&peer).is_some_and(|g| qualifies(&g));
                if !ok {
                    continue;
                }
                let m = msg(json!({"t": "advert", "ad": ad}));
                if let Err(e) = self.rt.route_to_remote(&peer, m).await {
                    tracing::debug!(peer, error = %e, "infer advert send failed");
                }
            }
        }
    }

    /// Peers this node currently holds a qualifying grant for.
    pub fn qualifying_peers(&self) -> Vec<String> {
        let live = self.rt.peer_ids();
        let mut v: Vec<String> = self
            .grants
            .iter()
            .filter(|e| qualifies(e.value()) && live.contains(e.key()))
            .map(|e| e.key().clone())
            .collect();
        v.sort();
        v
    }

    fn serve(self: &Arc<Self>, ctx: &PeerCtx, id: u64, data: &str) {
        let Some(table) = self.table().cloned() else { return };
        // Cheap standing checks before any work or memory is committed.
        let grant = ctx.grant();
        if !grant.as_ref().is_some_and(qualifies) || !table.peer_listed_any(&ctx.peer_id) {
            self.note(
                "infer.mesh.failed",
                json!({"peer": ctx.peer_id, "why": "standing", "standing": grant.as_ref().map(qualifies)}),
            );
            self.spawn_refusal(ctx.peer_id.clone(), id);
            return;
        }
        let Ok(permit) = self.intake.clone().try_acquire_owned() else {
            self.spawn_refusal(ctx.peer_id.clone(), id);
            return;
        };
        let Some(payload) = hex_decode(data) else {
            self.spawn_refusal(ctx.peer_id.clone(), id);
            return;
        };
        let frame = MeshFrame {
            frame_type: FrameType::InferRequest,
            payload,
        };
        let Ok(first) = frame.encode() else { return };
        let hub = self.clone();
        let peer = InferPeer {
            node_id: ctx.peer_id.clone(),
            grant,
        };
        tokio::spawn(async move {
            let mut stream = ServeStream {
                rt: hub.rt.clone(),
                peer: peer.node_id.clone(),
                id,
                first: Some(first),
            };
            let _ = serve_infer(
                &mut stream,
                &peer,
                table.as_ref(),
                &hub.upstream,
                &hub.gate,
                hub.audit.as_deref(),
            )
            .await;
            drop(permit);
        });
    }

    fn spawn_refusal(self: &Arc<Self>, peer: String, id: u64) {
        let rt = self.rt.clone();
        tokio::spawn(async move {
            let payload = super::wire::encode_resp(&super::wire::Resp::Error("refused".into()));
            let m = msg(json!({"t": "resp", "id": id, "data": hex_encode(&payload)}));
            let _ = rt.route_to_remote(&peer, m).await;
        });
    }
}

impl PeerControlSink for InferHub {
    fn on_peer_control(&self, ctx: &PeerCtx, _conn: u64, payload: &Value) -> Vec<Value> {
        // The standing this connection holds, as of this message.
        let first_contact = match ctx.grant() {
            Some(g) => self.grants.insert(ctx.peer_id.clone(), g).is_none(),
            None => {
                self.grants.remove(&ctx.peer_id);
                false
            }
        };
        let Some(kind) = payload.get("t").and_then(Value::as_str) else {
            return Vec::new();
        };
        match kind {
            // Answer a first hello so both sides learn at once.
            "hello" if first_contact => return vec![json!({"t": "hello"})],
            "hello" => {}
            "advert" => {
                let Some(table) = self.table() else { return Vec::new() };
                if let Some(ad) = payload
                    .get("ad")
                    .and_then(|a| serde_json::from_value::<ServiceAdvertisement>(a.clone()).ok())
                {
                    // The sender is the connection's peer, never a field.
                    table.ingest_advertisement(&ctx.peer_id, &ad);
                }
            }
            "req" => {
                let (Some(id), Some(data)) = (
                    payload.get("id").and_then(Value::as_u64),
                    payload.get("data").and_then(Value::as_str),
                ) else {
                    return Vec::new();
                };
                // `serve` needs an Arc<Self>; the sink is only ever held as one.
                if let Some(me) = self.self_arc() {
                    me.serve(ctx, id, data);
                }
            }
            "resp" => {
                let (Some(id), Some(data)) = (
                    payload.get("id").and_then(Value::as_u64),
                    payload.get("data").and_then(Value::as_str),
                ) else {
                    return Vec::new();
                };
                let key = (ctx.peer_id.clone(), id);
                if let Some(tx) = self.pending.get(&key).map(|e| e.clone()) {
                    let delivered = hex_decode(data).is_some_and(|b| tx.try_send(b).is_ok());
                    if !delivered {
                        // Garbage, or the exchange is not draining: cancel it.
                        self.pending.remove(&key);
                    }
                }
            }
            _ => {}
        }
        Vec::new()
    }
}

impl InferHub {
    fn self_arc(&self) -> Option<Arc<Self>> {
        self.me.get().and_then(std::sync::Weak::upgrade)
    }
}

#[async_trait]
impl MeshDialer for InferHub {
    fn standing(&self, node_id: &str) -> Option<Grant> {
        if !self.rt.peer_ids().iter().any(|p| p == node_id) {
            return None;
        }
        self.grants.get(node_id).map(|g| g.clone())
    }

    async fn dial(&self, node_id: &str) -> Result<Box<dyn MeshStream>, ProxyError> {
        if !self.is_admitted(node_id) {
            return Err(ProxyError::Refused(format!("peer {node_id} is not admitted")));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(RESP_BUFFER);
        self.pending.insert((node_id.to_string(), id), tx);
        Ok(Box::new(ClientStream {
            rt: self.rt.clone(),
            peer: node_id.to_string(),
            id,
            rx,
            pending: self.pending.clone(),
            max_hex: self.limits.max_mesh_request_body.saturating_mul(2) + 4096,
        }))
    }
}

/// Server side of one exchange: the request frame was handed in, response
/// frames go out as control messages.
struct ServeStream {
    rt: Arc<MeshRuntime>,
    peer: String,
    id: u64,
    first: Option<Vec<u8>>,
}

#[async_trait]
impl MeshStream for ServeStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        let f = split_frame(data)?;
        if f.frame_type != FrameType::InferResponse {
            return Err(mesh_err("not a response frame"));
        }
        let m = msg(json!({"t": "resp", "id": self.id, "data": hex_encode(&f.payload)}));
        self.rt.route_to_remote(&self.peer, m).await.map_err(mesh_err)
    }
    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        self.first.take().ok_or(MeshError::ConnectionClosed)
    }
    async fn close(&mut self) -> Result<(), MeshError> {
        Ok(())
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }
}

/// Consumer side of one exchange.
struct ClientStream {
    rt: Arc<MeshRuntime>,
    peer: String,
    id: u64,
    rx: mpsc::Receiver<Vec<u8>>,
    pending: Pending,
    max_hex: usize,
}

#[async_trait]
impl MeshStream for ClientStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        let f = split_frame(data)?;
        if f.frame_type != FrameType::InferRequest {
            return Err(mesh_err("not a request frame"));
        }
        let hex = hex_encode(&f.payload);
        if hex.len() > self.max_hex {
            return Err(mesh_err("request too large for the mesh path"));
        }
        let m = msg(json!({"t": "req", "id": self.id, "data": hex}));
        self.rt.route_to_remote(&self.peer, m).await.map_err(mesh_err)
    }
    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        let payload = self.rx.recv().await.ok_or(MeshError::ConnectionClosed)?;
        MeshFrame {
            frame_type: FrameType::InferResponse,
            payload,
        }
        .encode()
    }
    async fn close(&mut self) -> Result<(), MeshError> {
        self.pending.remove(&(self.peer.clone(), self.id));
        Ok(())
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }
}

impl Drop for ClientStream {
    fn drop(&mut self) {
        self.pending.remove(&(self.peer.clone(), self.id));
    }
}

#[cfg(test)]
pub(super) struct Probe(Pending);

#[cfg(test)]
impl Probe {
    pub(super) fn pending_len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
impl InferHub {
    /// Open an exchange (as `dial` does) and keep it alive for inspection.
    pub(super) async fn dial_for_test(&self, node: &str) -> TestExchange {
        let s = self.dial(node).await.unwrap();
        TestExchange { _s: s, probe: Probe(self.pending.clone()) }
    }
}

#[cfg(test)]
pub(super) struct TestExchange {
    _s: Box<dyn MeshStream>,
    probe: Probe,
}

#[cfg(test)]
impl TestExchange {
    pub(super) fn pending_len(&self) -> usize {
        self.probe.pending_len()
    }
}
