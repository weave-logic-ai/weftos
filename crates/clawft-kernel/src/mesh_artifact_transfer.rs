//! Serve and fetch for the artifact piece protocol (mesh-placement-11).
//!
//! One [`MeshStream`] carries a client-driven exchange:
//!
//! ```text
//! fetcher                              holder
//!   meta_request(key)        0x0B  ->
//!                            <-  0x0C  meta(descriptor) | reject
//!                            <-  0x0B  announce(have)
//!   announce(have)           0x0B  ->
//!   request(id, [i..])       0x0B  ->
//!                            <-  0x0C  piece(i, offset, block)... | no_piece(i)
//! ```
//!
//! Every piece is hash-checked before it reaches `ArtifactStore`; a bad
//! piece is chained (`artifact.piece_rejected`) and requested again. When
//! all pieces are held the whole-content hash is checked. A peer whose
//! descriptor fails that check lied about the content: its descriptor and
//! unshared pieces are discarded, the peer is dropped and the next peer is
//! tried. The fetch outcome is chained once (`artifact.fetch`), listing any
//! rejected descriptors.
//!
//! v1 fetches from one peer at a time, walking a [`PeerSet`] in order.
//! Card 25 plugs a multi-source [`PieceScheduler`] and parallel peers into
//! the same types.

use std::collections::{BTreeMap, HashSet};

use crate::mesh::{MeshError, MeshStream};
use crate::mesh_artifact::{ArtifactExchange, ExchangeError};
pub use crate::mesh_artifact_peers::{
    FetchError, FetchOutcome, PeerLink, PeerSet, PieceScheduler, SequentialScheduler, ServeStats,
};
use crate::mesh_artifact_wire::{ArtifactDescriptor, ArtifactKey, ArtifactMsg, Bitfield};

#[derive(Debug, thiserror::Error)]
enum PeerError {
    #[error("{0}")]
    Mesh(#[from] MeshError),
    #[error("{0}")]
    Wire(#[from] crate::mesh_artifact_wire::WireError),
    #[error("{0}")]
    Local(#[from] ExchangeError),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("peer refused: {0}")]
    Refused(String),
    #[error("timed out waiting for peer")]
    Timeout,
}

#[derive(Default)]
struct Progress {
    pieces: u32,
    bytes: u64,
    rejected: u32,
    sources: Vec<String>,
    /// Descriptor of the attempt in flight (pending until verified).
    current: Option<ArtifactDescriptor>,
    /// `(peer, artifact id, reason)` for descriptors that failed the
    /// whole-content check.
    bad_descriptors: Vec<(String, String, String)>,
    /// Piece blobs this fetch created in the store: the only blobs a
    /// failed promotion may roll back.
    written: HashSet<[u8; 32]>,
}

async fn send(stream: &mut dyn MeshStream, msg: &ArtifactMsg) -> Result<(), PeerError> {
    stream.send(&msg.to_wire()?).await?;
    Ok(())
}

impl ArtifactExchange {
    async fn recv(&self, stream: &mut dyn MeshStream) -> Result<ArtifactMsg, PeerError> {
        let raw = tokio::time::timeout(self.config().recv_timeout, stream.recv())
            .await
            .map_err(|_| PeerError::Timeout)??;
        Ok(ArtifactMsg::from_wire(&raw)?)
    }

    /// Serve one peer until it closes the stream. Only pieces of servable
    /// artifacts ([`Self::is_servable`]) are sent. An oversize or
    /// malformed frame ends the session with an error, and so does a peer
    /// that sends nothing for `serve_idle_timeout`.
    pub async fn serve(
        &self,
        stream: &mut dyn MeshStream,
        peer: &str,
    ) -> Result<ServeStats, ExchangeError> {
        let mut stats = ServeStats::default();
        loop {
            let idle = self.config().serve_idle_timeout;
            let raw = match tokio::time::timeout(idle, stream.recv()).await {
                Ok(Ok(raw)) => raw,
                Ok(Err(MeshError::ConnectionClosed)) => return Ok(stats),
                Ok(Err(e)) => return Err(ExchangeError::Io(e.to_string())),
                Err(_) => {
                    let _ = stream.close().await;
                    return Err(ExchangeError::Io(format!(
                        "peer {peer} idle for {idle:?}; serve session closed"
                    )));
                }
            };
            let msg = match ArtifactMsg::from_wire(&raw) {
                Ok(m) => m,
                Err(e) => {
                    let _ = stream.close().await;
                    return Err(e.into());
                }
            };
            if let Err(e) = self.serve_one(stream, peer, msg, &mut stats).await {
                let _ = stream.close().await;
                return Err(match e {
                    PeerError::Local(l) => l,
                    other => ExchangeError::Io(other.to_string()),
                });
            }
        }
    }

    /// Serve one frame already read from `stream`, for sessions that carry
    /// the piece protocol beside other traffic on one stream (the
    /// `workload.ctl` fetch-before-load, mesh-placement-12). Same rules as
    /// [`Self::serve`]: only servable artifacts are sent.
    pub async fn serve_frame(
        &self,
        stream: &mut dyn MeshStream,
        peer: &str,
        raw: &[u8],
        stats: &mut ServeStats,
    ) -> Result<(), ExchangeError> {
        let msg = ArtifactMsg::from_wire(raw)?;
        self.serve_one(stream, peer, msg, stats)
            .await
            .map_err(|e| match e {
                PeerError::Local(l) => l,
                other => ExchangeError::Io(other.to_string()),
            })
    }

    async fn serve_one(
        &self,
        stream: &mut dyn MeshStream,
        peer: &str,
        msg: ArtifactMsg,
        stats: &mut ServeStats,
    ) -> Result<(), PeerError> {
        match msg {
            ArtifactMsg::MetaRequest { key } => match self.servable(&key) {
                Some(d) => {
                    let id = d.id();
                    let have = self.have(&id).unwrap_or_else(|| Bitfield::new(0));
                    send(stream, &ArtifactMsg::Meta { descriptor: d }).await?;
                    send(stream, &ArtifactMsg::Announce { id, have }).await
                }
                None => {
                    let reason = "not available or not servable".to_string();
                    send(stream, &ArtifactMsg::Reject { key, reason }).await
                }
            },
            ArtifactMsg::Announce { id, have } => {
                self.note_peer_have(id, peer, have);
                Ok(())
            }
            ArtifactMsg::Request { id, pieces } => {
                let key = ArtifactKey::Root(id);
                let Some(d) = self.servable(&key) else {
                    let reason = "not available or not servable".to_string();
                    return send(stream, &ArtifactMsg::Reject { key, reason }).await;
                };
                for index in pieces {
                    let Ok(data) = self.load_piece(&d, index) else {
                        send(stream, &ArtifactMsg::NoPiece { id, index }).await?;
                        continue;
                    };
                    self.chain_serve_once(&d, peer);
                    for (n, block) in data.chunks(self.config().block_size).enumerate() {
                        let offset = (n * self.config().block_size) as u64;
                        let data = block.to_vec();
                        send(
                            stream,
                            &ArtifactMsg::Piece {
                                id,
                                index,
                                offset,
                                data,
                            },
                        )
                        .await?;
                    }
                    stats.pieces_served.push(index);
                    stats.bytes_served += data.len() as u64;
                }
                Ok(())
            }
            other => Err(PeerError::Protocol(format!(
                "unexpected {:?} frame from fetcher",
                other.frame_type()
            ))),
        }
    }

    fn servable(&self, key: &ArtifactKey) -> Option<ArtifactDescriptor> {
        self.resolve(key).filter(|d| self.is_servable(d))
    }

    /// Fetch `key` from `peers` with the v1 [`SequentialScheduler`].
    pub async fn fetch(
        &self,
        peers: &mut PeerSet,
        key: ArtifactKey,
    ) -> Result<FetchOutcome, FetchError> {
        self.fetch_with(peers, key, &mut SequentialScheduler).await
    }

    /// Fetch `key`, trying each live peer in turn until every piece is
    /// held and the whole content verifies. Pieces already held are never
    /// requested, so a repeated call resumes an interrupted transfer. A
    /// peer whose pieces do not assemble to its declared content is dropped
    /// and its descriptor discarded. The outcome (verified or failed) is
    /// chained as `artifact.fetch`.
    pub async fn fetch_with(
        &self,
        peers: &mut PeerSet,
        key: ArtifactKey,
        scheduler: &mut dyn PieceScheduler,
    ) -> Result<FetchOutcome, FetchError> {
        let mut progress = Progress::default();
        let mut last_error = String::from("no peers");
        let mut last_peer = String::new();
        let mut verify_failed = false;
        let mut done = self.resolve(&key).filter(|d| self.is_complete(d));
        for link in peers.links.iter_mut().filter(|l| !l.dead) {
            if done.is_some() {
                break;
            }
            last_peer = link.peer_id.clone();
            let before = (progress.pieces, progress.bytes);
            let err = match self.fetch_from(link, key, scheduler, &mut progress).await {
                Ok(d) if self.have_of(&d).is_complete() => {
                    match self.promote(&d, &progress.written) {
                        Ok(()) => {
                            done = Some(d);
                            continue;
                        }
                        Err(e) => {
                            // The liar's pieces were discarded: do not count them.
                            (progress.pieces, progress.bytes) = before;
                            progress.sources.retain(|p| *p != link.peer_id);
                            progress.current = None;
                            progress.bad_descriptors.push((
                                link.peer_id.clone(),
                                d.id().to_string(),
                                e.to_string(),
                            ));
                            verify_failed = true;
                            PeerError::Local(e)
                        }
                    }
                }
                Ok(_) => {
                    verify_failed = false;
                    continue; // this peer has no more of what is missing
                }
                Err(e) => {
                    verify_failed = false;
                    e
                }
            };
            last_error = err.to_string();
            if !matches!(err, PeerError::Refused(_)) {
                link.dead = true;
                let _ = link.stream.close().await;
            }
        }
        let Some(d) = done else {
            let missing = progress
                .current
                .as_ref()
                .map_or(0, |d| d.piece_count() - self.have_of(d).count());
            let err = self.fetch_failed(&key, &progress, &last_peer, last_error, missing);
            return Err(match err {
                FetchError::Incomplete { last_error, .. } if verify_failed => {
                    FetchError::Verification(last_error)
                }
                other => other,
            });
        };
        let id = d.id();
        self.materialize(&id)?;
        self.chain_fetch(serde_json::json!({
            "artifact_id": id.to_string(),
            "content_hash": d.content_hex(),
            "source_peer": progress.sources.last().cloned().unwrap_or_default(),
            "sources": progress.sources,
            "bytes": progress.bytes,
            "total_size": d.total_size,
            "pieces_fetched": progress.pieces,
            "pieces_rejected": progress.rejected,
            "descriptors_rejected": rejected_json(&progress),
            "result": "verified",
            "node": self.node_id(),
        }));
        Ok(FetchOutcome {
            id,
            descriptor: d,
            pieces_fetched: progress.pieces,
            bytes_fetched: progress.bytes,
            pieces_rejected: progress.rejected,
            sources: progress.sources,
        })
    }

    fn is_complete(&self, d: &ArtifactDescriptor) -> bool {
        self.have(&d.id()).is_some_and(|h| h.is_complete())
    }

    fn fetch_failed(
        &self,
        key: &ArtifactKey,
        progress: &Progress,
        last_peer: &str,
        last_error: String,
        missing: u32,
    ) -> FetchError {
        self.chain_fetch(serde_json::json!({
            "key": key.to_string(),
            "artifact_id": progress.current.as_ref().map(|d| d.id().to_string()),
            "source_peer": last_peer,
            "sources": progress.sources,
            "bytes": progress.bytes,
            "pieces_fetched": progress.pieces,
            "pieces_rejected": progress.rejected,
            "missing_pieces": missing,
            "descriptors_rejected": rejected_json(progress),
            "result": "failed",
            "error": last_error,
            "node": self.node_id(),
        }));
        FetchError::Incomplete {
            missing,
            last_error,
        }
    }

    async fn fetch_from(
        &self,
        link: &mut PeerLink,
        key: ArtifactKey,
        scheduler: &mut dyn PieceScheduler,
        progress: &mut Progress,
    ) -> Result<ArtifactDescriptor, PeerError> {
        let stream = link.stream.as_mut();
        let peer = link.peer_id.clone();
        send(stream, &ArtifactMsg::MetaRequest { key }).await?;
        let d = match self.recv(stream).await? {
            ArtifactMsg::Meta { descriptor } => descriptor,
            ArtifactMsg::Reject { reason, .. } => return Err(PeerError::Refused(reason)),
            other => return Err(unexpected(&other)),
        };
        let matches_key = match key {
            ArtifactKey::Root(id) => d.id() == id,
            ArtifactKey::Content(h) => d.content_hash == h,
        };
        if !matches_key {
            return Err(PeerError::Protocol(format!(
                "descriptor does not match {key}"
            )));
        }
        // Pending only: the content hash is the peer's claim until the
        // assembled pieces prove it (see `promote`).
        let id = self.note_pending(&d)?;
        progress.current = Some(d.clone());
        let mut peer_has = match self.recv(stream).await? {
            ArtifactMsg::Announce { id: aid, have }
                if aid == id && have.len() == d.piece_count() =>
            {
                have
            }
            other => return Err(unexpected(&other)),
        };
        let mine = self.have_of(&d);
        send(stream, &ArtifactMsg::Announce { id, have: mine }).await?;

        let mut strikes: BTreeMap<u32, u32> = BTreeMap::new();
        loop {
            let mut need = self.have_of(&d);
            for (&i, &n) in &strikes {
                if n >= self.config().max_piece_retries {
                    need.set(i, true); // given up on this piece from this peer
                }
            }
            let batch = scheduler.next_batch(&need, &peer_has, self.config().request_window);
            if batch.is_empty() {
                return Ok(d);
            }
            send(
                stream,
                &ArtifactMsg::Request {
                    id,
                    pieces: batch.clone(),
                },
            )
            .await?;
            for index in batch {
                match self.recv_piece(stream, &d, index).await? {
                    Some(data) => {
                        let hash = *blake3::hash(&data).as_bytes();
                        if hash != d.pieces[index as usize] {
                            progress.rejected += 1;
                            *strikes.entry(index).or_default() += 1;
                            self.chain_piece_rejected(&id, index, &peer, "hash mismatch");
                            continue;
                        }
                        if self.store_piece(&data, &hash)? {
                            progress.written.insert(hash);
                        }
                        progress.pieces += 1;
                        progress.bytes += data.len() as u64;
                        if !progress.sources.contains(&peer) {
                            progress.sources.push(peer.clone());
                        }
                    }
                    None => peer_has.set(index, false),
                }
            }
        }
    }

    /// Receive all blocks of piece `index` (in offset order), or `None` if
    /// the peer answers `no_piece`.
    async fn recv_piece(
        &self,
        stream: &mut dyn MeshStream,
        d: &ArtifactDescriptor,
        index: u32,
    ) -> Result<Option<Vec<u8>>, PeerError> {
        let want = d.piece_len(index) as usize;
        let mut buf: Vec<u8> = Vec::new();
        while buf.len() < want {
            match self.recv(stream).await? {
                ArtifactMsg::Piece {
                    id,
                    index: i,
                    offset,
                    data,
                } if id == d.id() && i == index && offset == buf.len() as u64 => {
                    if buf.len() + data.len() > want {
                        return Err(PeerError::Protocol("piece longer than descriptor".into()));
                    }
                    // Grow with the bytes actually received: a descriptor
                    // claiming a huge piece cannot make us pre-allocate it.
                    buf.extend_from_slice(&data);
                }
                ArtifactMsg::NoPiece { id, index: i } if id == d.id() && i == index => {
                    return Ok(None);
                }
                other => return Err(unexpected(&other)),
            }
        }
        Ok(Some(buf))
    }
}

fn rejected_json(progress: &Progress) -> serde_json::Value {
    progress
        .bad_descriptors
        .iter()
        .map(|(peer, id, reason)| {
            serde_json::json!({ "peer": peer, "artifact_id": id, "reason": reason })
        })
        .collect()
}

fn unexpected(msg: &ArtifactMsg) -> PeerError {
    let what = match msg {
        ArtifactMsg::Reject { reason, .. } => return PeerError::Refused(reason.clone()),
        ArtifactMsg::MetaRequest { .. } => "meta_request",
        ArtifactMsg::Announce { .. } => "announce",
        ArtifactMsg::Request { .. } => "request",
        ArtifactMsg::Meta { .. } => "meta",
        ArtifactMsg::Piece { .. } => "piece",
        ArtifactMsg::NoPiece { .. } => "no_piece",
    };
    PeerError::Protocol(format!("unexpected {what} message"))
}
