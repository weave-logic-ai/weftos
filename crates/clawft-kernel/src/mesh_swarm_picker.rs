//! Piece and peer selection for swarm fetches (ADR-099 section 6, card
//! mesh-placement-25). Pure logic, no I/O.
//!
//! - [`PiecePicker`] hands each peer the **rarest** piece it holds that is
//!   still needed and not already requested from someone else. Rarity is
//!   the number of live peers that hold the piece; ties go to the lowest
//!   index so a fetch is reproducible. When a peer is lost its in-flight
//!   pieces return to the pool and its holdings stop counting.
//! - [`order_peers`] ranks fetch candidates: same LAN as the local node
//!   first, then by measured link speed (peers never measured rank as an
//!   assumed default, so they get tried), then by id.
//! - [`LinkStats`] keeps a smoothed bytes-per-second figure per peer from
//!   the transfers this node actually performed. It is locally measured:
//!   a peer's own claims about its link are never used.

use std::collections::{BTreeMap, HashMap};

use dashmap::DashMap;

use crate::mesh_artifact_types::Bitfield;

/// What the picker tells one peer's worker to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Request this piece from the peer.
    Piece(u32),
    /// Nothing free for this peer now, but pieces it holds are in flight
    /// elsewhere and may come back: wait for a change.
    Wait,
    /// The peer holds nothing still needed: done with it.
    Nothing,
}

/// Which pieces are held, in flight and available where.
#[derive(Debug, Clone)]
pub struct PiecePicker {
    have: Bitfield,
    in_flight: BTreeMap<u32, String>,
    peers: HashMap<String, Bitfield>,
}

impl PiecePicker {
    /// Picker over `have` (pieces already held locally).
    pub fn new(have: Bitfield) -> Self {
        Self {
            have,
            in_flight: BTreeMap::new(),
            peers: HashMap::new(),
        }
    }

    /// A peer joined with this holding.
    pub fn add_peer(&mut self, peer: &str, has: Bitfield) {
        self.peers.insert(peer.to_string(), has);
    }

    /// A peer is gone: its holdings stop counting and its in-flight pieces
    /// return to the pool.
    pub fn remove_peer(&mut self, peer: &str) {
        self.peers.remove(peer);
        self.in_flight.retain(|_, p| p != peer);
    }

    /// The peer says it cannot supply piece `i` after all.
    pub fn peer_lacks(&mut self, peer: &str, i: u32) {
        if let Some(b) = self.peers.get_mut(peer) {
            b.set(i, false);
        }
        if self.in_flight.get(&i).is_some_and(|p| p == peer) {
            self.in_flight.remove(&i);
        }
    }

    /// Live peers holding piece `i`.
    pub fn availability(&self, i: u32) -> usize {
        self.peers.values().filter(|b| b.get(i)).count()
    }

    /// Next piece for `peer`, claiming it (see [`Pick`]).
    pub fn pick(&mut self, peer: &str) -> Pick {
        let Some(has) = self.peers.get(peer) else {
            return Pick::Nothing;
        };
        let mut best: Option<(usize, u32)> = None;
        let mut blocked = false;
        for i in self.have.missing() {
            if !has.get(i) {
                continue;
            }
            if self.in_flight.contains_key(&i) {
                blocked = true;
                continue;
            }
            let key = (self.availability(i), i);
            if best.is_none_or(|b| key < b) {
                best = Some(key);
            }
        }
        match best {
            Some((_, i)) => {
                self.in_flight.insert(i, peer.to_string());
                Pick::Piece(i)
            }
            None if blocked => Pick::Wait,
            None => Pick::Nothing,
        }
    }

    /// Piece `i` arrived and verified.
    pub fn complete(&mut self, i: u32) {
        self.have.set(i, true);
        self.in_flight.remove(&i);
    }

    /// Piece `i` was not delivered: back to the pool.
    pub fn release(&mut self, i: u32) {
        self.in_flight.remove(&i);
    }

    /// True when every piece is held.
    pub fn is_complete(&self) -> bool {
        self.have.is_complete()
    }

    /// Pieces still missing.
    pub fn missing(&self) -> u32 {
        self.have.len() - self.have.count()
    }

    /// Missing pieces no live peer holds.
    pub fn unavailable(&self) -> usize {
        self.have.missing().filter(|&i| self.availability(i) == 0).count()
    }
}

/// A peer that may hold an artifact.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerCandidate {
    /// Peer node id.
    pub peer_id: String,
    /// LAN the peer advertises (`net.lan` fact), for locality.
    pub lan_id: Option<String>,
}

impl PeerCandidate {
    /// Candidate with no locality information.
    pub fn new(peer_id: impl Into<String>) -> Self {
        Self {
            peer_id: peer_id.into(),
            lan_id: None,
        }
    }

    /// Candidate on `lan`.
    pub fn on_lan(mut self, lan: impl Into<String>) -> Self {
        self.lan_id = Some(lan.into());
        self
    }
}

/// Link speed assumed for a peer this node never transferred from, so it
/// is tried rather than starved (bytes per second).
pub const ASSUMED_LINK_BPS: f64 = 10.0 * 1024.0 * 1024.0;

/// Rank `candidates`: same LAN as `local_lan` first, then faster measured
/// link, then peer id. Banned peers are dropped.
pub fn order_peers(
    candidates: Vec<PeerCandidate>,
    local_lan: Option<&str>,
    links: &LinkStats,
    is_banned: &dyn Fn(&str) -> bool,
) -> Vec<PeerCandidate> {
    let mut v: Vec<(bool, f64, PeerCandidate)> = candidates
        .into_iter()
        .filter(|c| !is_banned(&c.peer_id))
        .map(|c| {
            let same_lan = matches!((local_lan, c.lan_id.as_deref()), (Some(a), Some(b)) if a == b);
            let bps = links.bytes_per_sec(&c.peer_id).unwrap_or(ASSUMED_LINK_BPS);
            (same_lan, bps, c)
        })
        .collect();
    v.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.total_cmp(&a.1))
            .then(a.2.peer_id.cmp(&b.2.peer_id))
    });
    v.into_iter().map(|(_, _, c)| c).collect()
}

/// Smoothed per-peer throughput measured by this node.
#[derive(Debug, Default)]
pub struct LinkStats {
    peers: DashMap<String, f64>,
}

/// Weight of the newest sample in the smoothed figure.
const EWMA_ALPHA: f64 = 0.3;

impl LinkStats {
    /// Record `bytes` received from `peer` in `secs` seconds.
    pub fn record(&self, peer: &str, bytes: u64, secs: f64) {
        if bytes == 0 || secs <= 0.0 {
            return;
        }
        let sample = bytes as f64 / secs;
        self.peers
            .entry(peer.to_string())
            .and_modify(|v| *v = EWMA_ALPHA * sample + (1.0 - EWMA_ALPHA) * *v)
            .or_insert(sample);
    }

    /// Smoothed bytes per second from `peer`, if ever measured.
    pub fn bytes_per_sec(&self, peer: &str) -> Option<f64> {
        self.peers.get(peer).map(|v| *v)
    }
}

#[cfg(test)]
#[path = "mesh_swarm_picker_tests.rs"]
mod tests;
