//! The licence exchange over the machine mesh service (ADR-106 phase 3,
//! service mode).
//!
//! In service mode the daemon has no mesh connections of its own. The
//! service consumes `mesh.cog.binding`, `mesh.cog.grant` and `mesh.cog.sync`
//! as control topics of its runtime and hands those from a licensed peer
//! (verified by admission, class `node`) to the reserved-topic owner's
//! registration, stamped `AdmittedPeer`. [`ServiceLicenceLinks`] is the
//! daemon's end:
//!
//! - **Inbound.** [`ServiceLicenceLinks::deliver`] takes the three topics out
//!   of the daemon's delivery path (the cog mesh router calls it) and gives
//!   them to the exchange's sinks with the stamped [`PeerCtx`], exactly as the
//!   kernel runtime would. A delivery that is not from a licensed peer is
//!   dropped and counted. Replies go back through the service link.
//! - **Outbound.** Floods and sync requests go out as sends through the
//!   service link ([`PeerSender`]); the service lets only the owner's
//!   registration send reserved topics.
//! - **Peers.** The service's own view ([`PeerDirectory`], `peers.list`):
//!   connected peers, and which of them are licensed. It is refreshed
//!   periodically; a peer that becomes licensed (in the view, or by a first
//!   stamped delivery) raises a `Joined` event so the exchange syncs with it
//!   at once.
//!
//! Per-connection token buckets key on a hash of the peer id: in service mode
//! the daemon sees no connection, and only a verified id reaches a sink.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;

use super::links::LicenceLinks;
use crate::error::KernelResult;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::mesh_artifact_tunnel::{PeerSender, licensed_peer};
use crate::mesh_delivery::PeerCtx;
use crate::mesh_discovery::{MeshPeerEvent, MeshPeerEventBus};
use crate::mesh_runtime::{COG_BINDING_TOPIC, COG_GRANT_TOPIC, COG_SYNC_TOPIC, PeerControlSink};

/// The licence control topics carried through the service.
pub const LICENCE_TOPICS: [&str; 3] = [COG_BINDING_TOPIC, COG_GRANT_TOPIC, COG_SYNC_TOPIC];

/// How often the daemon asks the service who is connected.
pub const PEER_REFRESH: Duration = Duration::from_secs(10);

/// Reply sends in flight at once; past it a reply is dropped and counted
/// (the peer's next sync asks again).
pub const MAX_REPLY_SENDS: usize = 64;

/// Reply sends in flight to one peer, inside [`MAX_REPLY_SENDS`].
pub const MAX_REPLY_SENDS_PER_PEER: usize = 4;

/// True when `msg` is on one of [`LICENCE_TOPICS`].
pub fn is_licence_topic(msg: &KernelMessage) -> bool {
    matches!(&msg.target, MessageTarget::Topic(t) if LICENCE_TOPICS.contains(&t.as_str()))
}

/// The service's peer view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerSnapshot {
    /// Peers with a route.
    pub connected: Vec<String>,
    /// Peers whose route admission verified with class `node`.
    pub licensed: Vec<String>,
    /// Whether the asking daemon holds the service's reserved licence topics
    /// (`None`: the service did not say).
    pub reserved_holder: Option<bool>,
    /// The uid that holds the reserved licence topics on this machine, when
    /// the service names one (where a non-holder sends its operator).
    pub reserved_holder_uid: Option<u32>,
}

/// Who the machine mesh service says is connected.
#[async_trait]
pub trait PeerDirectory: Send + Sync + 'static {
    /// The current view, or why it could not be read (link down).
    async fn peers(&self) -> Result<PeerSnapshot, String>;
}

/// Counters, all monotonic.
#[derive(Debug, Default)]
pub struct ServiceLinksCounters {
    /// Licence deliveries handed to a sink.
    pub delivered: AtomicU64,
    /// Licence deliveries from anything but a licensed peer (dropped).
    pub refused_unverified: AtomicU64,
    /// Licence deliveries with no sink installed (no exchange over this path).
    pub unhandled: AtomicU64,
    /// Peer view refreshes that failed (the view is cleared).
    pub refresh_failed: AtomicU64,
    /// Peer view refreshes that succeeded.
    pub refreshed: AtomicU64,
    /// Sync requests not answered (and not recorded as served) because the
    /// reply caps were full; the requester re-asks.
    pub replies_dropped: AtomicU64,
}

#[derive(Default)]
struct View {
    connected: Vec<String>,
    licensed: HashSet<String>,
}

/// The licence exchange's mesh in service mode.
pub struct ServiceLicenceLinks {
    sender: Arc<dyn PeerSender>,
    directory: Arc<dyn PeerDirectory>,
    sinks: DashMap<String, Arc<dyn PeerControlSink>>,
    view: RwLock<View>,
    events: MeshPeerEventBus,
    replies: Arc<tokio::sync::Semaphore>,
    per_peer: DashMap<String, Arc<tokio::sync::Semaphore>>,
    per_peer_cap: usize,
    holder: tokio::sync::watch::Sender<Option<bool>>,
    holder_uid: RwLock<Option<u32>>,
    /// Counters.
    pub counters: ServiceLinksCounters,
}

impl ServiceLicenceLinks {
    /// Links that send through `sender` and read peers from `directory`.
    pub fn new(sender: Arc<dyn PeerSender>, directory: Arc<dyn PeerDirectory>) -> Arc<Self> {
        Self::with_reply_caps(sender, directory, MAX_REPLY_SENDS, MAX_REPLY_SENDS_PER_PEER)
    }

    /// [`Self::new`] with explicit reply caps (in flight overall, per peer).
    pub fn with_reply_caps(
        sender: Arc<dyn PeerSender>,
        directory: Arc<dyn PeerDirectory>,
        total: usize,
        per_peer: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            sender,
            directory,
            sinks: DashMap::new(),
            view: RwLock::new(View::default()),
            events: MeshPeerEventBus::new(),
            replies: Arc::new(tokio::sync::Semaphore::new(total.max(1))),
            per_peer: DashMap::new(),
            per_peer_cap: per_peer.max(1),
            holder: tokio::sync::watch::channel(None).0,
            holder_uid: RwLock::new(None),
            counters: ServiceLinksCounters::default(),
        })
    }

    /// Whether this daemon holds the service's reserved licence topics, as
    /// of the last refresh (`None`: unknown, the last refresh failed or the
    /// service did not say).
    pub fn holder(&self) -> Option<bool> {
        *self.holder.borrow()
    }

    /// The uid the service named as the licence holder at the last refresh.
    pub fn holder_uid(&self) -> Option<u32> {
        *self.holder_uid.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Changes of [`Self::holder`].
    pub fn subscribe_holder(&self) -> tokio::sync::watch::Receiver<Option<bool>> {
        self.holder.subscribe()
    }

    fn set_holder(&self, v: Option<bool>) {
        self.holder.send_if_modified(|h| {
            let changed = *h != v;
            *h = v;
            changed
        });
    }

    /// A reply slot for `peer`: one of the global and one of its own.
    fn reserve(&self, peer: &str) -> Option<(tokio::sync::OwnedSemaphorePermit, tokio::sync::OwnedSemaphorePermit)> {
        let mine = self
            .per_peer
            .entry(peer.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(self.per_peer_cap)))
            .clone();
        let p = mine.try_acquire_owned().ok()?;
        let g = self.replies.clone().try_acquire_owned().ok()?;
        Some((g, p))
    }

    /// Read the service's peer view; raise `Joined` for each peer that became
    /// licensed since the last refresh.
    pub async fn refresh(&self) -> Result<(), String> {
        let snap = match self.directory.peers().await {
            Ok(s) => s,
            Err(e) => {
                // Link down or the service gone: nobody is known to be
                // licensed until the next good view (no floods meanwhile).
                *self.view.write().unwrap_or_else(|p| p.into_inner()) = View::default();
                self.set_holder(None);
                self.counters.refresh_failed.fetch_add(1, Ordering::Relaxed);
                return Err(e);
            }
        };
        self.counters.refreshed.fetch_add(1, Ordering::Relaxed);
        self.set_holder(snap.reserved_holder);
        *self.holder_uid.write().unwrap_or_else(|p| p.into_inner()) = snap.reserved_holder_uid;
        let licensed: HashSet<String> = snap.licensed.into_iter().collect();
        let joined: Vec<String> = {
            let mut v = self.view.write().unwrap_or_else(|p| p.into_inner());
            let joined = licensed.difference(&v.licensed).cloned().collect();
            v.connected = snap.connected;
            v.licensed = licensed;
            joined
        };
        for node_id in joined {
            self.events.emit(MeshPeerEvent::Joined { node_id, address: None, platform: None, verified: true });
        }
        Ok(())
    }

    /// [`Self::refresh`] now and every `every` while the links are alive.
    pub fn spawn_refresh(self: &Arc<Self>, every: Duration) -> Option<tokio::task::JoinHandle<()>> {
        let handle = tokio::runtime::Handle::try_current().ok()?;
        let w = Arc::downgrade(self);
        Some(handle.spawn(async move {
            loop {
                let Some(me) = w.upgrade() else { break };
                if let Err(e) = me.refresh().await {
                    tracing::debug!(error = %e, "licence peer view not refreshed");
                }
                drop(me);
                tokio::time::sleep(every).await;
            }
        }))
    }

    /// Take a licence delivery. Returns false when `msg` is not on a licence
    /// topic (the caller passes it on); true when it was consumed, handled or
    /// not.
    pub async fn deliver(&self, from: &PeerCtx, msg: KernelMessage) -> bool {
        let MessageTarget::Topic(topic) = &msg.target else { return false };
        if !LICENCE_TOPICS.contains(&topic.as_str()) {
            return false;
        }
        if !licensed_peer(from) {
            self.counters.refused_unverified.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        let Some(sink) = self.sinks.get(topic.as_str()).map(|s| s.clone()) else {
            self.counters.unhandled.fetch_add(1, Ordering::Relaxed);
            return true;
        };
        let MessagePayload::Json(payload) = &msg.payload else { return true };
        // The service vouched for this peer as a licensed node just now. A
        // peer first seen this way counts as joined (the exchange syncs with
        // it), as it would at the next refresh.
        let new = self
            .view
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .licensed
            .insert(from.peer_id.clone());
        if new {
            self.events.emit(MeshPeerEvent::Joined {
                node_id: from.peer_id.clone(),
                address: None,
                platform: None,
                verified: true,
            });
        }
        // A sync request is answered only with a reply slot in hand, taken
        // before the exchange sees it: a request that could not be answered
        // is not recorded as served, so the requester's retry is.
        let mut slot = None;
        if topic.as_str() == COG_SYNC_TOPIC && payload.get("op").and_then(|o| o.as_str()) == Some("request") {
            match self.reserve(&from.peer_id) {
                Some(s) => slot = Some(s),
                None => {
                    self.counters.replies_dropped.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            }
        }
        self.counters.delivered.fetch_add(1, Ordering::Relaxed);
        // Replies go out on their own tasks: this runs on the link's one
        // delivery worker, and a slow peer must not hold up the others.
        for reply in sink.on_peer_control(from, conn_of(&from.peer_id), payload) {
            let Some(permit) = slot.take().or_else(|| self.reserve(&from.peer_id)) else {
                self.counters.replies_dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let m = KernelMessage::new(0, MessageTarget::Topic(topic.clone()), MessagePayload::Json(reply));
            let (sender, peer, topic) = (self.sender.clone(), from.peer_id.clone(), topic.clone());
            tokio::spawn(async move {
                if let Err(e) = sender.send_to_node(&peer, m).await {
                    tracing::debug!(%peer, %topic, error = %e, "licence reply failed");
                }
                drop(permit);
            });
        }
        true
    }
}

/// A stable per-peer bucket key (never 0, which means "no connection").
fn conn_of(peer: &str) -> u64 {
    let h = blake3::hash(peer.as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&h.as_bytes()[..8]);
    u64::from_le_bytes(b) | 1
}

#[async_trait]
impl LicenceLinks for ServiceLicenceLinks {
    fn set_control_sink(&self, topic: &str, sink: Arc<dyn PeerControlSink>) {
        self.sinks.entry(topic.to_owned()).or_insert(sink);
    }

    fn peer_ids(&self) -> Vec<String> {
        self.view.read().unwrap_or_else(|p| p.into_inner()).connected.clone()
    }

    fn peer_licensed(&self, peer: &str) -> bool {
        self.view.read().unwrap_or_else(|p| p.into_inner()).licensed.contains(peer)
    }

    async fn route_to_remote(&self, peer: &str, msg: KernelMessage) -> KernelResult<()> {
        self.sender.send_to_node(peer, msg).await
    }

    fn subscribe_peer_events(&self) -> Option<tokio::sync::broadcast::Receiver<MeshPeerEvent>> {
        Some(self.events.subscribe())
    }
}
