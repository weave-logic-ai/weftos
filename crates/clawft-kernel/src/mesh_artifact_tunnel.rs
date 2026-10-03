//! Artifact streams over machine-mesh deliveries (ADR-106 section 5.4).
//!
//! The daemon holds the [`ArtifactExchange`] but not the mesh connections:
//! the machine mesh service owns those and hands the daemon `deliver` frames
//! and takes its `send`s. This module carries the piece protocol across that
//! seam. A [`TunnelStream`] is a [`MeshStream`] whose bytes travel as
//! `mesh.artifact.tunnel` messages addressed to a peer node:
//!
//! - **Dial** ([`TunnelDialer`], the production [`PeerDialer`]): opens a
//!   session id and speaks the fetcher side.
//! - **Serve** (inbound): the first `req` frame of an unknown session starts
//!   [`ArtifactExchange::serve_as`] with a [`ServePeer`] built from the
//!   service-stamped origin. Only a verified `node` peer is served; the
//!   redistribution policy still decides what.
//!
//! Replies are accepted only from the peer that was dialled and only while
//! that peer is a verified node, so an unadmitted sender cannot inject
//! frames into a fetch. Frames larger than [`CHUNK`] are split and
//! reassembled (a mesh-local line is capped at 1 MiB).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use dashmap::DashMap;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::error::KernelResult;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh::{MeshError, MeshStream};
use crate::mesh_admit::PeerClass;
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_types::MAX_ARTIFACT_FRAME;
use crate::mesh_delivery::PeerCtx;
use crate::mesh_swarm_fetch::PeerDialer;
use crate::mesh_swarm_state::ServePeer;
use crate::workload_pkg::codec::hex_encode;

/// Topic of tunnel frames.
pub const TOPIC_TUNNEL: &str = "mesh.artifact.tunnel";
/// Raw bytes per tunnel message.
pub const CHUNK: usize = 192 * 1024;
/// Most serve sessions from one peer.
pub const MAX_SERVE_PER_PEER: usize = 8;
/// Most serve sessions in all.
pub const MAX_SERVE_TOTAL: usize = 64;
/// Most dialled sessions open at once.
pub const MAX_DIAL_TOTAL: usize = 64;
/// Frames queued per session before the sender is dropped.
const SESSION_QUEUE: usize = 32;

/// Sends one kernel message to a peer node through the machine mesh. The
/// daemon implements it with its service link; tests with an in-memory net.
#[async_trait]
pub trait PeerSender: Send + Sync + 'static {
    /// Deliver `msg` to `node_id`.
    async fn send_to_node(&self, node_id: &str, msg: KernelMessage) -> KernelResult<()>;
}

/// Which side of the session a frame was sent by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Dir {
    /// The fetcher (the dialler) to the holder.
    Req,
    /// The holder back to the fetcher.
    Rsp,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TunnelFrame {
    v: u32,
    sid: String,
    dir: Dir,
    /// Chunk index within one stream frame.
    #[serde(default)]
    part: u32,
    /// Last chunk of the frame.
    #[serde(default)]
    last: bool,
    /// The stream was closed by the sender.
    #[serde(default)]
    close: bool,
    /// Chunk bytes, hex.
    #[serde(default)]
    data: String,
}

/// Counters, all monotonic.
#[derive(Debug, Default)]
pub struct TunnelCounters {
    /// Frames refused because the peer is not a verified node.
    pub refused_unverified: AtomicU64,
    /// Frames refused for a limit or a malformed shape.
    pub refused_other: AtomicU64,
    /// Serve sessions started.
    pub served_sessions: AtomicU64,
}

struct Session {
    tx: mpsc::Sender<Vec<u8>>,
    partial: Mutex<Vec<u8>>,
    next_part: Mutex<u32>,
}

/// Both ends of the artifact tunnel for one node.
pub struct ArtifactTunnel {
    exchange: Arc<ArtifactExchange>,
    sender: Arc<dyn PeerSender>,
    /// Sessions this node serves, by (peer, sid).
    serving: DashMap<(String, String), Arc<Session>>,
    /// Sessions this node dialled, by (peer, sid).
    dialled: DashMap<(String, String), Arc<Session>>,
    /// Counters.
    pub counters: TunnelCounters,
}

fn verified_node(from: &PeerCtx) -> bool {
    from.node_verified && from.class == PeerClass::Node
}

/// Lower- or upper-case hex to bytes; `None` on odd length or a bad digit.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|p| {
            let hi = (p[0] as char).to_digit(16)?;
            let lo = (p[1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

fn new_sid() -> String {
    let mut b = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex_encode(&b)
}

impl ArtifactTunnel {
    /// A tunnel serving `exchange` and sending through `sender`.
    pub fn new(exchange: Arc<ArtifactExchange>, sender: Arc<dyn PeerSender>) -> Arc<Self> {
        Arc::new(Self {
            exchange,
            sender,
            serving: DashMap::new(),
            dialled: DashMap::new(),
            counters: TunnelCounters::default(),
        })
    }

    /// The production [`PeerDialer`] over this tunnel.
    pub fn dialer(self: &Arc<Self>) -> Arc<dyn PeerDialer> {
        Arc::new(TunnelDialer { tunnel: self.clone() })
    }

    /// Serve sessions open now.
    pub fn serving_sessions(&self) -> usize {
        self.serving.len()
    }

    /// Handle one inbound `mesh.artifact.tunnel` delivery. `from` is built
    /// from the service-stamped origin; anything but a verified node is
    /// dropped and counted.
    pub async fn on_message(self: &Arc<Self>, from: &PeerCtx, msg: KernelMessage) {
        if !verified_node(from) {
            self.counters.refused_unverified.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let MessagePayload::Json(v) = msg.payload else {
            self.counters.refused_other.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let Ok(f) = serde_json::from_value::<TunnelFrame>(v) else {
            self.counters.refused_other.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if f.v != 1 || f.sid.is_empty() || f.sid.len() > 32 || f.data.len() > CHUNK * 2 {
            self.counters.refused_other.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let key = (from.peer_id.clone(), f.sid.clone());
        if f.close {
            // Dropping the registry's handle closes the session's receiver,
            // which ends the serve loop or fails the fetch.
            match f.dir {
                Dir::Req => self.serving.remove(&key),
                Dir::Rsp => self.dialled.remove(&key),
            };
            return;
        }
        match f.dir {
            Dir::Req => self.inbound_req(key, f).await,
            Dir::Rsp => {
                if let Some(s) = self.dialled.get(&key).map(|s| s.clone()) {
                    Self::feed(&s, &f).await;
                } else {
                    self.counters.refused_other.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    async fn inbound_req(self: &Arc<Self>, key: (String, String), f: TunnelFrame) {
        let existing = self.serving.get(&key).map(|s| s.clone());
        let session = match existing {
            Some(s) => s,
            None => {
                let per_peer = self.serving.iter().filter(|e| e.key().0 == key.0).count();
                if per_peer >= MAX_SERVE_PER_PEER || self.serving.len() >= MAX_SERVE_TOTAL {
                    self.counters.refused_other.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let (tx, rx) = mpsc::channel(SESSION_QUEUE);
                let s = Arc::new(Session {
                    tx,
                    partial: Mutex::default(),
                    next_part: Mutex::default(),
                });
                self.serving.insert(key.clone(), s.clone());
                self.counters.served_sessions.fetch_add(1, Ordering::Relaxed);
                let me = self.clone();
                let (peer, sid) = key.clone();
                tokio::spawn(async move {
                    let mut stream = TunnelStream::new(me.clone(), peer.clone(), sid.clone(), Dir::Rsp, rx);
                    let who = ServePeer::verified(peer.clone());
                    let _ = me.exchange.serve_as(&mut stream, &who).await;
                    me.serving.remove(&(peer, sid));
                });
                s
            }
        };
        Self::feed(&session, &f).await;
    }

    /// Reassemble `f` into whole stream frames and queue them for the session.
    async fn feed(s: &Session, f: &TunnelFrame) {
        let Some(bytes) = hex_decode(&f.data) else { return };
        let whole = {
            let mut part = s.next_part.lock().unwrap_or_else(|p| p.into_inner());
            let mut buf = s.partial.lock().unwrap_or_else(|p| p.into_inner());
            if f.part != *part || buf.len() + bytes.len() > MAX_ARTIFACT_FRAME + 64 {
                buf.clear();
                *part = 0;
                return;
            }
            buf.extend_from_slice(&bytes);
            if f.last {
                *part = 0;
                Some(std::mem::take(&mut *buf))
            } else {
                *part += 1;
                None
            }
        };
        if let Some(w) = whole {
            let _ = s.tx.send(w).await;
        }
    }
}

struct TunnelDialer {
    tunnel: Arc<ArtifactTunnel>,
}

#[async_trait]
impl PeerDialer for TunnelDialer {
    async fn dial(&self, peer_id: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        let t = &self.tunnel;
        if t.dialled.len() >= MAX_DIAL_TOTAL {
            return Err(MeshError::Transport("too many artifact sessions open".into()));
        }
        let sid = new_sid();
        let (tx, rx) = mpsc::channel(SESSION_QUEUE);
        let s = Arc::new(Session {
            tx,
            partial: Mutex::default(),
            next_part: Mutex::default(),
        });
        t.dialled.insert((peer_id.to_owned(), sid.clone()), s);
        Ok(Box::new(TunnelStream::new(t.clone(), peer_id.to_owned(), sid, Dir::Req, rx)))
    }
}

/// One artifact session as a [`MeshStream`].
pub struct TunnelStream {
    tunnel: Arc<ArtifactTunnel>,
    peer: String,
    sid: String,
    /// The direction this end sends in.
    out: Dir,
    rx: mpsc::Receiver<Vec<u8>>,
    closed: bool,
}

impl TunnelStream {
    fn new(
        tunnel: Arc<ArtifactTunnel>,
        peer: String,
        sid: String,
        out: Dir,
        rx: mpsc::Receiver<Vec<u8>>,
    ) -> Self {
        Self { tunnel, peer, sid, out, rx, closed: false }
    }

    fn registry_remove(&self) {
        let key = (self.peer.clone(), self.sid.clone());
        match self.out {
            Dir::Req => self.tunnel.dialled.remove(&key),
            Dir::Rsp => self.tunnel.serving.remove(&key),
        };
    }

    async fn emit(&self, f: TunnelFrame) -> Result<(), MeshError> {
        let value = serde_json::to_value(&f).map_err(|e| MeshError::Io(e.to_string()))?;
        let msg = KernelMessage::new(
            0,
            MessageTarget::Topic(TOPIC_TUNNEL.into()),
            MessagePayload::Json(value),
        );
        self.tunnel
            .sender
            .send_to_node(&self.peer, msg)
            .await
            .map_err(|e| MeshError::Transport(e.to_string()))
    }
}

#[async_trait]
impl MeshStream for TunnelStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        if self.closed {
            return Err(MeshError::ConnectionClosed);
        }
        let n = data.chunks(CHUNK).count().max(1);
        for (i, chunk) in data.chunks(CHUNK).chain(data.is_empty().then_some(&[][..])).enumerate() {
            self.emit(TunnelFrame {
                v: 1,
                sid: self.sid.clone(),
                dir: self.out,
                part: i as u32,
                last: i + 1 == n,
                close: false,
                data: hex_encode(chunk),
            })
            .await?;
        }
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        self.rx.recv().await.ok_or(MeshError::ConnectionClosed)
    }

    async fn close(&mut self) -> Result<(), MeshError> {
        if std::mem::replace(&mut self.closed, true) {
            return Ok(());
        }
        self.registry_remove();
        let _ = self
            .emit(TunnelFrame {
                v: 1,
                sid: self.sid.clone(),
                dir: self.out,
                part: 0,
                last: true,
                close: true,
                data: String::new(),
            })
            .await;
        Ok(())
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }
}

impl Drop for TunnelStream {
    fn drop(&mut self) {
        if !self.closed {
            self.registry_remove();
        }
    }
}
