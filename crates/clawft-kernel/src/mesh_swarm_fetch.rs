//! Multi-source swarm fetch (ADR-099 section 6, card mesh-placement-25).
//!
//! [`ArtifactExchange::swarm_fetch`] pulls one artifact from several
//! holders at once, on top of the card 11 piece protocol:
//!
//! - **Sources.** Candidates are ranked by trust tier, then locality, then
//!   measured link speed ([`order_peers`]); up to `max_sources` are dialed. The first peer that
//!   answers fixes the descriptor; every other source must present the same
//!   artifact id.
//! - **Scheduling.** One worker per peer. Each worker asks the shared
//!   [`PiecePicker`] for the rarest piece its peer holds that nobody else is
//!   fetching, requests it, verifies its hash and stores it. A faster peer
//!   simply asks again sooner, so work follows measured speed.
//! - **Reliability.** A worker whose peer drops, times out or lacks the
//!   piece hands its pieces back and another peer picks them up; a lost
//!   source is replaced from the remaining candidates. A peer that serves a
//!   corrupt piece is banned (`artifact.piece_rejected`, `artifact.peer_ban`)
//!   and its piece is fetched elsewhere.
//! - **Bandwidth.** Received bytes are paced by the node's download cap.
//! - **Limits.** A descriptor with a piece size over `max_piece_size`, a total
//!   over `max_artifact_bytes`, or one that does not fit the caller's
//!   [`Expect`] (exact size from a signed manifest) is refused before any
//!   piece is requested. At most one piece is buffered per source, so memory
//!   is bounded by `max_sources x max_piece_size`.
//! - **Verification.** When every piece is held, the whole-content hash is
//!   checked ([`ArtifactExchange::promote`]). A verified artifact that a
//!   verified manifest allows is seeded from then on (`artifact.seed`).
//!
//! The outcome is chained once as `artifact.fetch`, like the single-source
//! fetch, with the sources, rejected and lost peers added.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::mesh::{MeshError, MeshStream};
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_peers::{FetchError, FetchOutcome};
use crate::mesh_artifact_transfer::{PeerError, send};
use crate::mesh_artifact_types::{ArtifactDescriptor, ArtifactId, ArtifactKey, Bitfield};
use crate::mesh_artifact_wire::ArtifactMsg;
use crate::mesh_swarm_picker::{PeerCandidate, Pick, PiecePicker, order_peers};

/// Opens a stream to a peer's [`ArtifactExchange::serve`] loop.
#[async_trait]
pub trait PeerDialer: Send + Sync + 'static {
    /// Connect to `peer_id`.
    async fn dial(&self, peer_id: &str) -> Result<Box<dyn MeshStream>, MeshError>;
}

/// What the caller already knows about the artifact, from a signed manifest.
/// A peer's descriptor that does not fit is refused before any piece is
/// requested.
#[derive(Debug, Clone, Default)]
pub struct Expect {
    /// Exact total size.
    pub size: Option<u64>,
    /// Largest acceptable total size (when the exact size is not known).
    pub max_size: Option<u64>,
    /// Exact artifact id (piece-list root), when something pins it.
    pub root: Option<ArtifactId>,
}

/// Per-fetch settings.
#[derive(Debug, Clone, Default)]
pub struct SwarmFetchOptions {
    /// LAN of the local node (`net.lan` fact): same-LAN peers rank first
    /// within a trust tier.
    pub local_lan: Option<String>,
    /// What the caller knows about the artifact.
    pub expect: Expect,
}

/// Fetch attempts in one call: after a peer's content fails verification the
/// peer is banned and the next candidates are tried.
const MAX_ATTEMPTS: usize = 3;

pub(crate) struct Session {
    pub(crate) peer: String,
    pub(crate) stream: Box<dyn MeshStream>,
    pub(crate) has: Bitfield,
}

#[derive(Default)]
struct Totals {
    pieces: u32,
    bytes: u64,
    rejected: u32,
    sources: Vec<String>,
    written: HashSet<[u8; 32]>,
    /// `(peer, why)` for sources lost mid-fetch.
    lost: Vec<(String, String)>,
    banned: Vec<String>,
}

struct Shared {
    picker: PiecePicker,
    totals: Totals,
}

fn lock(m: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// How one attempt ended, short of success.
enum Attempt {
    /// Pieces assembled but the content did not verify: the sessions' peers
    /// all vouched for a descriptor that lied.
    Unverified { error: String, peers: Vec<String> },
    /// Could not complete (no peers, all lost).
    Failed(FetchError),
}

impl ArtifactExchange {
    /// Refuse a descriptor that exceeds the node's caps or does not fit what
    /// the caller expects, before any piece is requested.
    pub(crate) fn check_descriptor(
        &self,
        d: &ArtifactDescriptor,
        expect: &Expect,
    ) -> Result<(), PeerError> {
        let cfg = self.config();
        let refuse = |why: String| Err(PeerError::Protocol(format!("descriptor refused: {why}")));
        if d.piece_size > cfg.max_piece_size {
            return refuse(format!(
                "piece size {} exceeds the accepted {}",
                d.piece_size, cfg.max_piece_size
            ));
        }
        if d.total_size > cfg.max_artifact_bytes {
            return refuse(format!(
                "total size {} exceeds the accepted {}",
                d.total_size, cfg.max_artifact_bytes
            ));
        }
        if let Some(max) = expect.max_size
            && d.total_size > max
        {
            return refuse(format!("total size {} exceeds the expected maximum {max}", d.total_size));
        }
        if let Some(size) = expect.size
            && d.total_size != size
        {
            return refuse(format!("total size {} is not the expected {size}", d.total_size));
        }
        if let Some(root) = expect.root
            && d.id() != root
        {
            return refuse("artifact id is not the expected root".into());
        }
        Ok(())
    }

    pub(crate) async fn open_session_for_lookup(
        &self,
        dialer: &dyn PeerDialer,
        peer: &str,
        key: ArtifactKey,
    ) -> Result<(Session, ArtifactDescriptor), String> {
        self.open_session(dialer, peer, key, &Expect::default()).await
    }

    /// Dial `peer` and ask for `key`: its descriptor and the pieces it holds.
    async fn open_session(
        &self,
        dialer: &dyn PeerDialer,
        peer: &str,
        key: ArtifactKey,
        expect: &Expect,
    ) -> Result<(Session, ArtifactDescriptor), String> {
        let mut stream = tokio::time::timeout(self.config().recv_timeout, dialer.dial(peer))
            .await
            .map_err(|_| "dial timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let result = self.handshake(stream.as_mut(), key, expect).await;
        match result {
            Ok((d, has)) => Ok((
                Session {
                    peer: peer.to_string(),
                    stream,
                    has,
                },
                d,
            )),
            Err(e) => {
                let _ = stream.close().await;
                Err(e.to_string())
            }
        }
    }

    async fn handshake(
        &self,
        stream: &mut dyn MeshStream,
        key: ArtifactKey,
        expect: &Expect,
    ) -> Result<(ArtifactDescriptor, Bitfield), PeerError> {
        send(stream, &ArtifactMsg::MetaRequest { key }).await?;
        let d = match self.recv(stream).await? {
            ArtifactMsg::Meta { descriptor } => descriptor,
            ArtifactMsg::Reject { reason, .. } => return Err(PeerError::Refused(reason)),
            other => return Err(PeerError::Protocol(format!("unexpected {other:?}"))),
        };
        let matches_key = match key {
            ArtifactKey::Root(id) => d.id() == id,
            ArtifactKey::Content(h) => d.content_hash == h,
        };
        if !matches_key {
            return Err(PeerError::Protocol(format!("descriptor does not match {key}")));
        }
        d.validate().map_err(PeerError::Wire)?;
        self.check_descriptor(&d, expect)?;
        let id = d.id();
        let has = match self.recv(stream).await? {
            ArtifactMsg::Announce { id: aid, have } if aid == id && have.len() == d.piece_count() => {
                have
            }
            other => return Err(PeerError::Protocol(format!("unexpected {other:?}"))),
        };
        Ok((d, has))
    }

    /// Fetch `key` from several holders at once (see the module docs).
    /// Pieces already held are not requested again, so a repeated call
    /// resumes. `candidates` are the peers that may hold it (from
    /// [`crate::mesh_swarm_lookup`]); banned peers are skipped.
    ///
    /// A fetch by content hash trusts nobody's descriptor: when the pieces a
    /// peer's descriptor describes do not assemble to the content hash, that
    /// peer and the sources that vouched for the same descriptor are banned
    /// and the fetch carries on with the remaining candidates (up to three
    /// attempts), so one liar cannot block it or get honest peers blamed.
    pub async fn swarm_fetch(
        self: &Arc<Self>,
        dialer: Arc<dyn PeerDialer>,
        candidates: Vec<PeerCandidate>,
        key: ArtifactKey,
        opts: &SwarmFetchOptions,
    ) -> Result<FetchOutcome, FetchError> {
        if let Some(d) = self.resolve(&key).filter(|d| self.is_complete(d)) {
            return Ok(FetchOutcome {
                id: d.id(),
                descriptor: d,
                pieces_fetched: 0,
                bytes_fetched: 0,
                pieces_rejected: 0,
                sources: Vec::new(),
            });
        }
        let ordered = order_peers(
            candidates,
            opts.local_lan.as_deref(),
            &self.swarm.links,
            &|p| self.is_banned(p),
        );
        let mut queue: VecDeque<PeerCandidate> = ordered.into();
        let mut dialed = 0usize;
        let mut last_unverified = None;
        for _ in 0..MAX_ATTEMPTS {
            match self.swarm_attempt(&dialer, &mut queue, key, opts, &mut dialed).await {
                Ok(out) => return Ok(out),
                Err(Attempt::Failed(e)) => {
                    return Err(match last_unverified {
                        Some(msg) => FetchError::Verification(msg),
                        None => e,
                    });
                }
                Err(Attempt::Unverified { error, peers }) => {
                    for p in &peers {
                        self.ban_peer(p, &format!("vouched for content that does not verify as {key}"));
                    }
                    last_unverified = Some(error);
                }
            }
        }
        let msg = last_unverified.unwrap_or_default();
        self.chain_fetch(serde_json::json!({
            "key": key.to_string(),
            "result": "failed",
            "error": msg,
            "peers_dialed": dialed,
            "swarm": true,
            "node": self.node_id(),
        }));
        Err(FetchError::Verification(msg))
    }

    async fn swarm_attempt(
        self: &Arc<Self>,
        dialer: &Arc<dyn PeerDialer>,
        queue: &mut VecDeque<PeerCandidate>,
        key: ArtifactKey,
        opts: &SwarmFetchOptions,
        dialed: &mut usize,
    ) -> Result<FetchOutcome, Attempt> {
        let mut last_error = String::from("no candidate peers");

        // The first peer that answers fixes the descriptor.
        let mut first = None;
        while let Some(c) = queue.pop_front() {
            if self.is_banned(&c.peer_id) {
                continue;
            }
            *dialed += 1;
            match self.open_session(dialer.as_ref(), &c.peer_id, key, &opts.expect).await {
                Ok(x) => {
                    first = Some(x);
                    break;
                }
                Err(e) => last_error = format!("{}: {e}", c.peer_id),
            }
        }
        let Some((s0, d)) = first else {
            return Err(Attempt::Failed(self.swarm_failed(
                &key,
                None,
                &Totals::default(),
                *dialed,
                0,
                last_error,
            )));
        };
        let id = self.note_pending(&d).map_err(|e| Attempt::Failed(e.into()))?;
        let mut session_peers = vec![s0.peer.clone()];
        let mut picker = PiecePicker::new(self.have_of(&d));
        picker.add_peer(&s0.peer, s0.has.clone());
        let shared = Arc::new(Mutex::new(Shared {
            picker,
            totals: Totals::default(),
        }));
        let notify = Arc::new(Notify::new());
        let max = self.config().max_sources;
        let mut set: JoinSet<()> = JoinSet::new();
        let mut active = 0usize;
        let spawn = |set: &mut JoinSet<()>, s: Session| {
            let (ex, sh, n, d) = (self.clone(), shared.clone(), notify.clone(), d.clone());
            set.spawn(async move { run_worker(ex, sh, n, d, s).await });
        };
        spawn(&mut set, s0);
        active += 1;

        loop {
            // Fill up to `max_sources` sessions from the remaining candidates.
            while active < max && !lock(&shared).picker.is_complete() {
                let Some(c) = queue.pop_front() else { break };
                if self.is_banned(&c.peer_id) {
                    continue;
                }
                *dialed += 1;
                let expect = Expect {
                    root: Some(id),
                    ..opts.expect.clone()
                };
                match self.open_session(dialer.as_ref(), &c.peer_id, ArtifactKey::Root(id), &expect).await {
                    Ok((s, _)) => {
                        session_peers.push(s.peer.clone());
                        lock(&shared).picker.add_peer(&s.peer, s.has.clone());
                        spawn(&mut set, s);
                        active += 1;
                    }
                    Err(e) => {
                        last_error = format!("{}: {e}", c.peer_id);
                        lock(&shared).totals.lost.push((c.peer_id, e));
                    }
                }
            }
            if set.join_next().await.is_none() {
                break;
            }
            active -= 1;
            if lock(&shared).picker.is_complete() {
                break;
            }
        }
        set.shutdown().await;

        let g = lock(&shared);
        let totals = &g.totals;
        if !g.picker.is_complete() {
            let missing = g.picker.missing();
            return Err(Attempt::Failed(self.swarm_failed(
                &key,
                Some(&d),
                totals,
                *dialed,
                missing,
                last_error,
            )));
        }
        if let Err(e) = self.promote(&d, &totals.written) {
            // `promote` discarded the descriptor and the pieces this fetch wrote.
            return Err(Attempt::Unverified {
                error: e.to_string(),
                peers: session_peers,
            });
        }
        self.materialize(&id).map_err(|e| Attempt::Failed(e.into()))?;
        self.chain_fetch(serde_json::json!({
            "artifact_id": id.to_string(),
            "content_hash": d.content_hex(),
            "source_peer": totals.sources.last().cloned().unwrap_or_default(),
            "sources": totals.sources,
            "bytes": totals.bytes,
            "total_size": d.total_size,
            "pieces_fetched": totals.pieces,
            "pieces_rejected": totals.rejected,
            "peers_dialed": *dialed,
            "peers_lost": totals.lost.iter().map(|(p, w)| serde_json::json!({"peer": p, "why": w})).collect::<Vec<_>>(),
            "peers_banned": totals.banned,
            "result": "verified",
            "swarm": true,
            "node": self.node_id(),
        }));
        Ok(FetchOutcome {
            id,
            descriptor: d,
            pieces_fetched: totals.pieces,
            bytes_fetched: totals.bytes,
            pieces_rejected: totals.rejected,
            sources: totals.sources.clone(),
        })
    }

    fn swarm_failed(
        &self,
        key: &ArtifactKey,
        d: Option<&ArtifactDescriptor>,
        totals: &Totals,
        dialed: usize,
        missing: u32,
        last_error: String,
    ) -> FetchError {
        self.chain_fetch(serde_json::json!({
            "key": key.to_string(),
            "artifact_id": d.map(|d| d.id().to_string()),
            "sources": totals.sources,
            "bytes": totals.bytes,
            "pieces_fetched": totals.pieces,
            "pieces_rejected": totals.rejected,
            "missing_pieces": missing,
            "peers_dialed": dialed,
            "peers_banned": totals.banned,
            "result": "failed",
            "error": last_error,
            "swarm": true,
            "node": self.node_id(),
        }));
        FetchError::Incomplete {
            missing,
            last_error,
        }
    }
}

/// One peer's worker: request the picker's next piece, verify, store, repeat.
async fn run_worker(
    ex: Arc<ArtifactExchange>,
    shared: Arc<Mutex<Shared>>,
    notify: Arc<Notify>,
    d: ArtifactDescriptor,
    mut s: Session,
) {
    let id = d.id();
    loop {
        // Subscribe before looking, so a change between the look and the wait
        // is not missed.
        let wake = notify.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        let pick = {
            let mut g = lock(&shared);
            if g.picker.is_complete() {
                break;
            }
            g.picker.pick(&s.peer)
        };
        let index = match pick {
            Pick::Nothing => break,
            Pick::Wait => {
                wake.await;
                continue;
            }
            Pick::Piece(i) => i,
        };
        let started = Instant::now();
        let got = request_piece(&ex, s.stream.as_mut(), &d, index).await;
        let mut g = lock(&shared);
        match got {
            Ok(Some(data)) => {
                let hash = *blake3::hash(&data).as_bytes();
                if hash != d.pieces[index as usize] {
                    g.totals.rejected += 1;
                    g.picker.release(index);
                    let banned = ex.report_corrupt_piece(&id, index, &s.peer);
                    if banned {
                        g.picker.remove_peer(&s.peer);
                        g.totals.banned.push(s.peer.clone());
                        notify.notify_waiters();
                        break;
                    }
                    notify.notify_waiters();
                    continue;
                }
                match ex.store_piece(&data, &hash) {
                    Ok(created) => {
                        if created {
                            g.totals.written.insert(hash);
                        }
                    }
                    Err(e) => {
                        g.picker.remove_peer(&s.peer);
                        g.totals.lost.push((s.peer.clone(), e.to_string()));
                        notify.notify_waiters();
                        break;
                    }
                }
                g.picker.complete(index);
                g.totals.pieces += 1;
                g.totals.bytes += data.len() as u64;
                if !g.totals.sources.contains(&s.peer) {
                    g.totals.sources.push(s.peer.clone());
                }
                ex.swarm
                    .links
                    .record(&s.peer, data.len() as u64, started.elapsed().as_secs_f64());
                notify.notify_waiters();
            }
            Ok(None) => {
                g.picker.peer_lacks(&s.peer, index);
                notify.notify_waiters();
            }
            Err(e) => {
                g.picker.remove_peer(&s.peer);
                g.totals.lost.push((s.peer.clone(), e.to_string()));
                notify.notify_waiters();
                break;
            }
        }
    }
    let _ = s.stream.close().await;
    // Wake the others: this peer's holdings and in-flight pieces changed.
    notify.notify_waiters();
}

/// Ask for piece `index` and read its blocks (`None` when the peer answers
/// `no_piece`), pacing received bytes by the node's download cap.
async fn request_piece(
    ex: &ArtifactExchange,
    stream: &mut dyn MeshStream,
    d: &ArtifactDescriptor,
    index: u32,
) -> Result<Option<Vec<u8>>, PeerError> {
    let id = d.id();
    send(
        stream,
        &ArtifactMsg::Request {
            id,
            pieces: vec![index],
        },
    )
    .await?;
    let want = d.piece_len(index) as usize;
    let mut buf: Vec<u8> = Vec::new();
    while buf.len() < want {
        match ex.recv(stream).await? {
            ArtifactMsg::Piece {
                id: pid,
                index: i,
                offset,
                data,
            } if pid == id && i == index && offset == buf.len() as u64 => {
                if buf.len() + data.len() > want {
                    return Err(PeerError::Protocol("piece longer than descriptor".into()));
                }
                ex.swarm.bandwidth.download(data.len()).await;
                buf.extend_from_slice(&data);
            }
            ArtifactMsg::NoPiece { id: pid, index: i } if pid == id && i == index => {
                return Ok(None);
            }
            ArtifactMsg::Reject { reason, .. } => return Err(PeerError::Refused(reason)),
            other => return Err(PeerError::Protocol(format!("unexpected {other:?}"))),
        }
    }
    Ok(Some(buf))
}
