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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::mesh_discovery::MeshPeerEvent;

/// Notices kept for replay (oldest dropped first).
pub const MAX_LOGGED: usize = 1024;
/// Largest log file read at start.
const MAX_LOG_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// The ordered notices this node has verified, and where they are saved.
#[derive(Default)]
pub(super) struct NoticeLog {
    entries: Vec<SignedRevocation>,
    file: Option<PathBuf>,
}

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
            if signer_ok && verify_revocation(&s, &self.anchors).is_ok() {
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
        if let Some(path) = log.file.clone()
            && let Err(e) = serde_json::to_vec(&log.entries)
                .map_err(std::io::Error::other)
                .and_then(|b| write_atomic(&path, &b))
        {
            tracing::warn!(path = %path.display(), error = %e, "revocation log not saved");
        }
    }

    /// The notices kept for replay, oldest first.
    pub fn logged(&self) -> Vec<SignedRevocation> {
        self.log.lock().unwrap_or_else(|p| p.into_inner()).entries.clone()
    }

    /// Send every logged notice to `peer`, oldest first. The first
    /// [`NOTICE_BURST`] go at once; the rest are paced at the rate the
    /// receiver accepts so a long log is not rate-limited away. Returns how
    /// many were sent.
    pub async fn replay_to(&self, peer: &str) -> usize {
        let mut sent = 0usize;
        for signed in self.logged() {
            let Ok(value) = serde_json::to_value(&signed) else {
                continue;
            };
            if sent as f64 >= NOTICE_BURST {
                tokio::time::sleep(Duration::from_secs_f64(1.0 / NOTICES_PER_SEC)).await;
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

    /// Replay the log to every peer that joins or recovers, for as long as
    /// the exchange lives. No-op outside a tokio runtime.
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
                let peer = match ev {
                    MeshPeerEvent::Joined { node_id, .. } | MeshPeerEvent::Recovered { node_id, .. } => node_id,
                    _ => continue,
                };
                let Some(x) = weak.upgrade() else { break };
                tokio::spawn(async move {
                    x.replay_to(&peer).await;
                });
            }
        });
    }
}
