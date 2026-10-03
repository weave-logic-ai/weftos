//! Reconcile-on-rejoin for revocation notices (ADR-099 section 7).
//!
//! A notice is flooded once, so a node that was down or partitioned when it
//! crossed the mesh never sees it. Every verified notice is therefore kept in
//! an ordered, bounded, persisted log, and whenever a peer joins or recovers
//! (`MeshPeerEvent::Joined` / `Recovered`) its neighbour replays the log to
//! it. Replay is safe because notices are signed and idempotent: the
//! receiver verifies each one as it would a fresh one, a notice it already
//! applied is dropped before it costs any budget, and a new one is applied
//! (revocation list, eviction, forced unload of what it runs) and forwarded
//! like any other. Revocations are only ever added, so a replayed old notice
//! cannot undo anything.
//!
//! Bounded: only peers admission verified are replayed to, at most one replay
//! per peer runs at a time, a peer is replayed to at most once per
//! [`REPLAY_COOLDOWN`] (a flapping link cannot make this node a signature-verify
//! amplifier), and a replay stops when the peer leaves. Only notices whose
//! subject is *still revoked here* are kept or replayed: lifting a revocation
//! (`unrevoke`) drops its notice, and a notice signed by a key since revoked
//! is dropped too, so a lifted revocation does not come back through this log.
//! Peers that still hold the old notice can re-send it; lifting a revocation
//! everywhere means lifting it (and clearing `revocation-notices.json`) on
//! every node.

use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::mesh_discovery::MeshPeerEvent;

/// Notices kept for replay (oldest dropped first).
pub const MAX_LOGGED: usize = 1024;
/// A peer is replayed to at most once per this long.
pub const REPLAY_COOLDOWN: Duration = Duration::from_secs(60);
/// Largest log file read at start.
const MAX_LOG_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// The ordered notices this node has verified, and where they are saved.
#[derive(Default)]
pub(super) struct NoticeLog {
    pub(super) entries: Vec<SignedRevocation>,
    file: Option<PathBuf>,
}

/// One peer's replay bookkeeping.
pub(super) struct ReplayState {
    running: bool,
    started: Instant,
    cancel: Arc<AtomicBool>,
}

pub(super) type Replays = HashMap<String, ReplayState>;

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

impl RevocationExchange {
    /// Keep the replay log in `path` (loaded now, saved after every notice).
    /// Entries that no longer verify (a key since unpinned or revoked) are
    /// dropped, so a replay never carries a notice peers would refuse.
    /// Returns how many were restored. A missing file starts empty; a
    /// malformed or oversized one is ignored (and overwritten on the next
    /// notice).
    pub fn with_log_file(&self, path: impl Into<PathBuf>) -> usize {
        let path = path.into();
        let loaded: Vec<SignedRevocation> = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|m| m.file_type().is_file() && m.len() <= MAX_LOG_FILE_BYTES)
            .and_then(|_| std::fs::read(&path).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut kept = Vec::new();
        for s in loaded {
            let signer_ok = s
                .public_key
                .as_slice()
                .try_into()
                .map(|pk: [u8; 32]| {
                    !self.list.is_subject_revoked(
                        RevocationKind::SignerKey,
                        &crate::workload_pkg::codec::hex_encode(&pk),
                    )
                })
                .unwrap_or(false);
            if signer_ok && self.is_current(&s) && verify_revocation(&s, &self.anchors).is_ok() {
                // Known from now on: replayed copies cost no budget here.
                let key = Self::notice_key(&s);
                let mut g = self.seen.lock().unwrap_or_else(|p| p.into_inner());
                if g.0.insert(key) {
                    g.1.push_back(key);
                }
                kept.push(s);
            }
        }
        let n = kept.len();
        let mut log = self.log.lock().unwrap_or_else(|p| p.into_inner());
        log.entries = kept;
        log.file = Some(path);
        n
    }

    /// The notice still describes something this node holds revoked, and its
    /// signer has not been revoked since.
    fn is_current(&self, s: &SignedRevocation) -> bool {
        let Ok(n) = serde_json::from_str::<RevocationNotice>(&s.payload) else {
            return false;
        };
        let signer_revoked = <[u8; 32]>::try_from(s.public_key.as_slice()).is_ok_and(|pk| {
            self.list
                .is_subject_revoked(RevocationKind::SignerKey, &crate::workload_pkg::codec::hex_encode(&pk))
        });
        !signer_revoked && self.list.is_subject_revoked(n.kind, &n.id)
    }

    /// Drop the logged notices for one subject (call when it is lifted) and
    /// save. Returns how many were dropped.
    pub fn forget_subject(&self, kind: RevocationKind, id: &str) -> usize {
        let Ok(id) = kind.normalize(id) else { return 0 };
        let mut log = self.log.lock().unwrap_or_else(|p| p.into_inner());
        let before = log.entries.len();
        log.entries.retain(|e| {
            serde_json::from_str::<RevocationNotice>(&e.payload).is_ok_and(|n| !(n.kind == kind && n.id == id))
        });
        let dropped = before - log.entries.len();
        if dropped > 0 {
            Self::save_log(&log);
        }
        dropped
    }

    fn save_log(log: &NoticeLog) {
        if let Some(path) = &log.file
            && let Err(e) = serde_json::to_vec(&log.entries)
                .map_err(std::io::Error::other)
                .and_then(|b| write_atomic(path, &b))
        {
            tracing::warn!(path = %path.display(), error = %e, "revocation log not saved");
        }
    }

    /// Remember a verified notice for replay, once, and save the log.
    pub(super) fn log_notice(&self, signed: &SignedRevocation) {
        let mut log = self.log.lock().unwrap_or_else(|p| p.into_inner());
        if log.entries.contains(signed) {
            return;
        }
        log.entries.push(signed.clone());
        if log.entries.len() > MAX_LOGGED {
            log.entries.remove(0);
        }
        Self::save_log(&log);
    }

    /// The notices kept for replay, oldest first.
    /// Those no longer in force here (lifted, or signer revoked) are dropped
    /// first, and the log saved without them.
    pub fn logged(&self) -> Vec<SignedRevocation> {
        let mut log = self.log.lock().unwrap_or_else(|p| p.into_inner());
        let before = log.entries.len();
        log.entries.retain(|e| self.is_current(e));
        if log.entries.len() != before {
            Self::save_log(&log);
        }
        log.entries.clone()
    }

    /// Replays started so far (for tests and diagnostics).
    pub fn replays_started(&self) -> u64 {
        self.replays_started.load(Ordering::Relaxed)
    }

    /// Send every logged notice to `peer`, oldest first. The first
    /// [`NOTICE_BURST`] go at once; the rest are paced at the rate the
    /// receiver accepts so a long log is not rate-limited away. Returns how
    /// many were sent.
    pub async fn replay_to(&self, peer: &str) -> usize {
        self.replay_cancellable(peer, &AtomicBool::new(false)).await
    }

    async fn replay_cancellable(&self, peer: &str, cancel: &AtomicBool) -> usize {
        let mut sent = 0usize;
        for signed in self.logged() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let Ok(value) = serde_json::to_value(&signed) else {
                continue;
            };
            if sent as f64 >= NOTICE_BURST {
                tokio::time::sleep(Duration::from_secs_f64(1.0 / NOTICES_PER_SEC)).await;
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
            }
            let msg = KernelMessage::new(
                0,
                MessageTarget::Topic(REVOKE_TOPIC.to_string()),
                MessagePayload::Json(value),
            );
            if let Err(e) = self.runtime.route_to_remote(peer, msg).await {
                tracing::debug!(peer, error = %e, "revocation replay stopped");
                break;
            }
            sent += 1;
        }
        sent
    }

    /// Start a replay to `peer` unless one is running or one started within
    /// [`REPLAY_COOLDOWN`].
    fn start_replay(me: &Arc<Self>, peer: String) {
        let cancel = {
            let mut g = me.replays.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(r) = g.get(&peer)
                && (r.running || r.started.elapsed() < REPLAY_COOLDOWN)
            {
                return;
            }
            // Bounded like the other per-peer maps.
            if g.len() >= 1024 && !g.contains_key(&peer) {
                g.retain(|_, r| r.running);
            }
            let cancel = Arc::new(AtomicBool::new(false));
            g.insert(
                peer.clone(),
                ReplayState {
                    running: true,
                    started: Instant::now(),
                    cancel: cancel.clone(),
                },
            );
            cancel
        };
        me.replays_started.fetch_add(1, Ordering::Relaxed);
        let x = me.clone();
        tokio::spawn(async move {
            x.replay_cancellable(&peer, &cancel).await;
            if let Some(r) = x.replays.lock().unwrap_or_else(|p| p.into_inner()).get_mut(&peer) {
                r.running = false;
            }
        });
    }

    /// Replay the log to every verified peer that joins or recovers, for as
    /// long as the exchange lives; stop a replay when its peer leaves. No-op
    /// outside a tokio runtime.
    pub(super) fn spawn_rejoin_replay(me: &Arc<Self>) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut rx = me.runtime.subscribe_peer_events();
        let weak = Arc::downgrade(me);
        handle.spawn(async move {
            loop {
                let ev = match rx.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let Some(x) = weak.upgrade() else { break };
                match ev {
                    MeshPeerEvent::Joined { node_id, verified: true, .. }
                    | MeshPeerEvent::Recovered { node_id, verified: true, .. } => Self::start_replay(&x, node_id),
                    MeshPeerEvent::Left { node_id } => {
                        if let Some(r) = x.replays.lock().unwrap_or_else(|p| p.into_inner()).get(&node_id) {
                            r.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                    _ => {}
                }
            }
        });
    }
}
