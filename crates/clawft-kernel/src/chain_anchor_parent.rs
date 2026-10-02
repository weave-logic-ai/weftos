//! `ChainAnchorBackend::Parent`: a project kernel anchors its chain head to
//! the user daemon (ADR-103 A7, Phase 2 package D).
//!
//! [`ParentAnchor`] builds a [`ProjectAnchorStmt`] for the project chain
//! head, signs it with the project key and hands it to a [`ParentTransport`]
//! (`project.anchor.submit` on the user daemon; the real link is wired by
//! packages G/H, tests stub it). On acceptance it appends
//! `project.anchored` to the project chain (the two-way link). Frequency
//! (300 s / 100 events, plus one on graceful shutdown) is the job of the
//! existing [`AnchoringController`](super::AnchoringController) wrapped
//! around this backend; [`ParentAnchor::anchor_head`] is the forced path
//! for shutdown.
//!
//! Crash and outage rules:
//! * The statement is written to `pending_path` (0600, atomic replace)
//!   BEFORE it is sent. A kill between the chain append and the anchor, or
//!   between the user daemon's acceptance and our `project.anchored`, leaves
//!   the pending file; [`ParentAnchor::retry_pending`] resends it and the
//!   user daemon answers an identical statement idempotently.
//! * Only the latest pending statement is kept and replayed: a later head
//!   covers earlier ones. `seq` increments per ACCEPTED statement, so every
//!   pending statement of an outage carries the same `seq`.
//! * Retries back off (5 s doubling to 300 s). A rejection that carries the
//!   parent's last accepted statement (lost acknowledgement) is adopted when
//!   it verifies under our own key, then the anchor is rebuilt on top of it.
//! * The last accepted statement is recovered from `project.anchored`
//!   events (each re-verified under the project key) at construction, so a
//!   restart continues the `seq` / `prev_anchor` chain.
//!
//! Key history: after `project.rekey` the chain holds statements signed by
//! the replaced key. A statement is trusted as history only when it verifies
//! under the current key or a key the user daemon reports as part of the
//! project's certificate history (rekeyed-out keys; never a key revoked for
//! compromise). At construction only the current key is known, so older
//! statements are skipped and the first submission is refused with the
//! parent's last statement plus that key history; [`ParentAnchor`] then
//! adopts it (see below) and the sequence continues at N + 1.
//!
//! Adoption (a lost acknowledgement, or the rekey case) requires that the
//! statement verifies, that it extends what we hold, and that OUR chain
//! really has `head_hash` at `head_seq`: a parent cannot make us record a
//! head we never produced. The acknowledgement itself ([`AnchorAck`]) is
//! authenticated by the transport only (the local unix socket to the user
//! daemon); nothing signs it.
//!
//! Transport contract: [`ParentTransport::submit`] must return within a
//! bounded time. It is run on a worker thread and abandoned after
//! [`ParentAnchorConfig::submit_timeout_secs`] (default 10 s, also for the
//! forced shutdown anchor), counting as unreachable; a late success of an
//! abandoned call is a lost acknowledgement and is handled as such. No lock
//! that readers use is held while a submission is in flight.
//!
//! The parent lost its state (no record, our `seq` is ahead): the parent
//! answers `anchor_seq` with no `last`; nothing is adopted and the failure
//! shows in [`ParentAnchor::last_error`]. A rewind of the anchor chain is an
//! owner decision. Restore `<manifests>/<id>.anchor.json` on the user side
//! from the `statement`, `user_seq` and `user_event_hash` of our last
//! `project.anchored` event (that is exactly the file's content), then
//! restart the user daemon so it re-appends the user-chain event.
//!
//! Honest limit: the user daemon attests "this key claimed head X at time
//! T"; it cannot verify X without subscribing to the project chain.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use chrono::{DateTime, Duration, Utc};
use clawft_types::project::canon::{hex_decode, hex_encode};
use clawft_types::project::cert::{ProjectAnchorStmt, key_id, ts};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

use super::ANCHOR_SOURCE;
use crate::chain::{AnchorReceipt, ChainAnchor, ChainEvent, ChainManager};

#[path = "chain_anchor_parent_types.rs"]
mod types;
pub use types::*;

#[derive(Debug, Clone)]
struct Accepted {
    stmt: ProjectAnchorStmt,
    ack: AnchorAck,
}

#[derive(Default)]
struct State {
    last: Option<Accepted>,
    failures: u32,
    retry_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    /// Retired keys the parent told us about (in memory only).
    learned: Vec<[u8; 32]>,
}

/// The `Parent` anchor backend.
pub struct ParentAnchor {
    chain: Arc<ChainManager>,
    key: SigningKey,
    project_id: String,
    cert_serial: Mutex<u64>,
    transport: Arc<dyn ParentTransport>,
    pending_path: PathBuf,
    cfg: ParentAnchorConfig,
    /// Serialises anchor operations; held across a submission.
    op: Mutex<()>,
    /// Short-lived: never held across a submission.
    state: Mutex<State>,
}

fn receipt_of(acc: &Accepted, now: DateTime<Utc>) -> AnchorReceipt {
    AnchorReceipt {
        hash: hex_decode::<32>(&acc.stmt.head_hash).unwrap_or([0u8; 32]),
        tx_id: format!("parent-{}", acc.ack.user_seq),
        anchored_at: now,
    }
}

impl ParentAnchor {
    /// Build over the project `chain`. Recovers the last accepted statement
    /// (signed by the current key) from the chain and drops a pending
    /// statement that is already covered.
    pub fn new(
        chain: Arc<ChainManager>,
        key: SigningKey,
        project_id: impl Into<String>,
        cert_serial: u64,
        transport: Arc<dyn ParentTransport>,
        pending_path: impl Into<PathBuf>,
        cfg: ParentAnchorConfig,
    ) -> Self {
        let this = Self {
            chain,
            key,
            project_id: project_id.into(),
            cert_serial: Mutex::new(cert_serial),
            transport,
            pending_path: pending_path.into(),
            cfg,
            op: Mutex::new(()),
            state: Mutex::new(State::default()),
        };
        {
            let mut st = this.state.lock().unwrap();
            st.last = this.recover(&[]);
            if this.read_pending().is_some_and(|p| st.last.as_ref().is_some_and(|l| p.seq <= l.stmt.seq)) {
                let _ = std::fs::remove_file(&this.pending_path);
            }
        }
        this
    }

    /// The certificate serial put into new statements (after a rekey).
    pub fn set_cert_serial(&self, serial: u64) {
        *self.cert_serial.lock().unwrap() = serial;
    }

    /// `(seq, statement hash)` of the last accepted statement.
    pub fn last_accepted(&self) -> Option<(u64, String)> {
        let st = self.state.lock().unwrap();
        st.last.as_ref().map(|a| (a.stmt.seq, a.stmt.hash()))
    }

    /// The statement waiting for the parent, if any.
    pub fn pending(&self) -> Option<ProjectAnchorStmt> {
        self.read_pending()
    }

    /// Message of the most recent failed attempt, cleared on success.
    pub fn last_error(&self) -> Option<String> {
        self.state.lock().unwrap().last_error.clone()
    }

    fn pubkey(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// Does `stmt` verify under the current key, a key learned from the
    /// parent, or one of `extra`?
    fn verifies(&self, stmt: &ProjectAnchorStmt, learned: &[[u8; 32]], extra: &[[u8; 32]]) -> bool {
        std::iter::once(self.pubkey())
            .chain(learned.iter().copied())
            .chain(extra.iter().copied())
            .any(|pk| key_id(&pk) == stmt.project_key_id && stmt.verify(&pk).is_ok())
    }

    fn recover(&self, learned: &[[u8; 32]]) -> Option<Accepted> {
        let mut best: Option<Accepted> = None;
        for e in self.chain.tail(0) {
            if e.source != ANCHOR_SOURCE || e.kind != KIND_ANCHORED {
                continue;
            }
            let Some(acc) = e.payload.as_ref().and_then(parse_anchored) else { continue };
            if acc.stmt.project_id != self.project_id || !self.verifies(&acc.stmt, learned, &[]) {
                continue;
            }
            if best.as_ref().is_none_or(|b| acc.stmt.seq > b.stmt.seq) {
                best = Some(acc);
            }
        }
        best
    }

    fn head_event(&self) -> Result<ChainEvent, String> {
        self.chain
            .tail(1)
            .into_iter()
            .next_back()
            .ok_or_else(|| "project chain is empty; nothing to anchor".to_string())
    }

    fn event_for(&self, hash: &[u8; 32]) -> Result<ChainEvent, String> {
        let head = self.head_event()?;
        if &head.hash == hash {
            return Ok(head);
        }
        self.chain
            .tail(usize::MAX)
            .into_iter()
            .rev()
            .find(|e| &e.hash == hash)
            .ok_or_else(|| "hash is not an event of the project chain".to_string())
    }

    /// Does our own chain have `hash` at `seq`?
    fn has_head(&self, seq: u64, hash: &str) -> bool {
        self.chain
            .tail(0)
            .iter()
            .any(|e| e.sequence == seq && hex_encode(&e.hash) == hash)
    }

    /// Nothing new since the last accepted statement (own acknowledgement
    /// events do not count).
    fn covered(last: &Accepted, ev: &ChainEvent) -> bool {
        ev.sequence <= last.stmt.head_seq
            || (ev.sequence == last.stmt.head_seq + 1
                && ev.source == ANCHOR_SOURCE
                && ev.kind == KIND_ANCHORED)
    }

    fn build(&self, last: Option<&Accepted>, ev: &ChainEvent, now: DateTime<Utc>) -> ProjectAnchorStmt {
        ProjectAnchorStmt {
            project_id: self.project_id.clone(),
            project_key_id: String::new(),
            cert_serial: *self.cert_serial.lock().unwrap(),
            seq: last.map_or(1, |l| l.stmt.seq + 1),
            chain_id: u64::from(ev.chain_id),
            head_hash: hex_encode(&ev.hash),
            head_seq: ev.sequence,
            rule_hash: hex_encode(&ev.rule_hash.unwrap_or([0u8; 32])),
            at: ts(now),
            prev_anchor: last.map(|l| l.stmt.hash()),
            sig: String::new(),
        }
        .sign(&self.key)
    }

    fn read_pending(&self) -> Option<ProjectAnchorStmt> {
        let text = std::fs::read_to_string(&self.pending_path).ok()?;
        text.lines().rev().find(|l| !l.trim().is_empty()).and_then(|l| serde_json::from_str(l).ok())
    }

    fn write_pending(&self, stmt: &ProjectAnchorStmt) -> Result<(), String> {
        let mut line = serde_json::to_vec(stmt).map_err(|e| e.to_string())?;
        line.push(b'\n');
        crate::project_identity::write_private_atomic(&self.pending_path, &line, false)
            .map_err(|e| format!("write {}: {e}", self.pending_path.display()))
    }

    fn fail(&self, now: DateTime<Utc>, why: String) -> String {
        let mut st = self.state.lock().unwrap();
        st.failures = st.failures.saturating_add(1);
        let shift = (st.failures - 1).min(20);
        let secs = (self.cfg.backoff_base_secs << shift).min(self.cfg.backoff_max_secs);
        st.retry_at = Some(now + Duration::seconds(secs));
        st.last_error = Some(why.clone());
        why
    }

    /// Append `project.anchored`, remember the statement, clear a covered
    /// pending file. The append comes first: a kill after it leaves a
    /// pending statement with `seq <= last`, which [`Self::new`] discards.
    fn record(&self, stmt: ProjectAnchorStmt, ack: AnchorAck) -> Accepted {
        self.chain.append(
            ANCHOR_SOURCE,
            KIND_ANCHORED,
            Some(json!({
                "statement": stmt,
                "user_seq": ack.user_seq,
                "user_event_hash": ack.user_event_hash,
            })),
        );
        if self.read_pending().is_none_or(|p| p.seq <= stmt.seq) {
            let _ = std::fs::remove_file(&self.pending_path);
        }
        let acc = Accepted { stmt, ack };
        let mut st = self.state.lock().unwrap();
        st.last = Some(acc.clone());
        st.failures = 0;
        st.retry_at = None;
        st.last_error = None;
        acc
    }

    /// Take the parent's last accepted statement as ours. Refused unless it
    /// verifies (current key, learned keys, or `key_history`), extends what
    /// we hold, and OUR chain has its `head_hash` at `head_seq`.
    fn adopt(&self, last: (ProjectAnchorStmt, AnchorAck), key_history: &[[u8; 32]]) -> Result<Accepted, String> {
        let (stmt, ack) = last;
        let (ours, learned) = {
            let st = self.state.lock().unwrap();
            (st.last.clone(), st.learned.clone())
        };
        if stmt.project_id != self.project_id {
            return Err("statement is for another project".into());
        }
        if !self.verifies(&stmt, &learned, key_history) {
            return Err("statement does not verify under any key of this project".into());
        }
        let extends = match &ours {
            Some(o) => stmt.seq == o.stmt.seq + 1 && stmt.prev_anchor.as_deref() == Some(o.stmt.hash().as_str()),
            // Nothing recoverable (fresh run, or history signed by a replaced key).
            None => (stmt.seq == 1) == stmt.prev_anchor.is_none(),
        };
        if !extends {
            return Err("statement does not extend the recorded anchor chain".into());
        }
        if !self.has_head(stmt.head_seq, &stmt.head_hash) {
            return Err(format!(
                "our chain has no event {} at seq {}; refusing to record a head we did not produce",
                stmt.head_hash, stmt.head_seq
            ));
        }
        self.state.lock().unwrap().learned.extend(key_history.iter().copied());
        Ok(self.record(stmt, ack))
    }

    /// One bounded submission. The call runs on a worker thread and is
    /// abandoned after the configured deadline; no lock is held meanwhile.
    fn submit_bounded(&self, stmt: &ProjectAnchorStmt) -> Result<AnchorAck, AnchorSubmitError> {
        let (tx, rx) = mpsc::channel();
        let (transport, s) = (self.transport.clone(), stmt.clone());
        std::thread::spawn(move || {
            let _ = tx.send(transport.submit(&s));
        });
        rx.recv_timeout(std::time::Duration::from_secs(self.cfg.submit_timeout_secs))
            .unwrap_or_else(|_| {
                Err(AnchorSubmitError::Unreachable(format!(
                    "no answer within {} s",
                    self.cfg.submit_timeout_secs
                )))
            })
    }

    /// Caller holds `op`.
    fn deliver(&self, mut stmt: ProjectAnchorStmt, now: DateTime<Utc>) -> Result<AnchorReceipt, String> {
        for attempt in 0..2 {
            match self.submit_bounded(&stmt) {
                Ok(ack) => return Ok(receipt_of(&self.record(stmt, ack), now)),
                Err(AnchorSubmitError::Unreachable(m)) => {
                    return Err(self.fail(now, format!("parent unreachable: {m}")));
                }
                Err(AnchorSubmitError::Rejected { kind, message, last, key_history }) => {
                    if attempt == 0 && let Some(last) = last {
                        match self.adopt(*last, &key_history) {
                            Ok(acc) => {
                                let head = self.head_event()?;
                                if Self::covered(&acc, &head) {
                                    return Ok(receipt_of(&acc, now));
                                }
                                stmt = self.build(Some(&acc), &head, now);
                                self.write_pending(&stmt)?;
                                continue;
                            }
                            Err(why) => {
                                return Err(self.fail(
                                    now,
                                    format!("parent rejected anchor ({kind}): {message}; not adopted: {why}"),
                                ));
                            }
                        }
                    }
                    return Err(self.fail(now, format!("parent rejected anchor ({kind}): {message}")));
                }
            }
        }
        Err(self.fail(now, "parent rejected the rebuilt anchor".into()))
    }

    fn anchor_event(&self, ev: ChainEvent, now: DateTime<Utc>, force: bool) -> Result<AnchorReceipt, String> {
        let _op = self.op.lock().map_err(|_| "parent anchor lock poisoned".to_string())?;
        let (last, retry_at) = {
            let st = self.state.lock().unwrap();
            (st.last.clone(), st.retry_at)
        };
        if let Some(acc) = &last
            && Self::covered(acc, &ev)
        {
            return Ok(receipt_of(acc, now));
        }
        let stmt = self.build(last.as_ref(), &ev, now);
        self.write_pending(&stmt)?;
        if !force && let Some(t) = retry_at && now < t {
            return Err(format!("parent unreachable; next retry after {}", ts(t)));
        }
        self.deliver(stmt, now)
    }

    /// Anchor the current head now, ignoring the frequency policy but not
    /// the retry backoff unless `force` (graceful shutdown passes `true`).
    pub fn anchor_head_at(&self, now: DateTime<Utc>, force: bool) -> Result<AnchorReceipt, String> {
        self.anchor_event(self.head_event()?, now, force)
    }

    /// [`Self::anchor_head_at`] for the wall clock, forced: the shutdown
    /// anchor. Bounded by the submission deadline.
    pub fn anchor_head(&self) -> Result<AnchorReceipt, String> {
        self.anchor_head_at(Utc::now(), true)
    }

    /// Resend the latest pending statement exactly as written once its
    /// backoff has elapsed. `Ok(None)`: nothing pending, or not due yet.
    pub fn retry_pending_at(&self, now: DateTime<Utc>) -> Result<Option<AnchorReceipt>, String> {
        let _op = self.op.lock().map_err(|_| "parent anchor lock poisoned".to_string())?;
        let Some(stmt) = self.read_pending() else { return Ok(None) };
        let (last, retry_at) = {
            let st = self.state.lock().unwrap();
            (st.last.clone(), st.retry_at)
        };
        if last.is_some_and(|l| stmt.seq <= l.stmt.seq) {
            let _ = std::fs::remove_file(&self.pending_path);
            return Ok(None);
        }
        if retry_at.is_some_and(|t| now < t) {
            return Ok(None);
        }
        self.deliver(stmt, now).map(Some)
    }

    /// [`Self::retry_pending_at`] for the wall clock.
    pub fn retry_pending(&self) -> Result<Option<AnchorReceipt>, String> {
        self.retry_pending_at(Utc::now())
    }
}

fn parse_anchored(p: &Value) -> Option<Accepted> {
    let stmt = serde_json::from_value(p.get("statement")?.clone()).ok()?;
    Some(Accepted {
        stmt,
        ack: AnchorAck {
            user_seq: p.get("user_seq")?.as_u64()?,
            user_event_hash: p.get("user_event_hash")?.as_str()?.to_owned(),
        },
    })
}

impl ChainAnchor for ParentAnchor {
    fn anchor(&self, hash: &[u8; 32]) -> Result<AnchorReceipt, String> {
        let now = Utc::now();
        self.anchor_event(self.event_for(hash)?, now, false)
    }

    fn verify(&self, receipt: &AnchorReceipt) -> Result<bool, String> {
        let want = hex_encode(&receipt.hash);
        Ok(self.chain.tail(0).iter().any(|e| {
            e.source == ANCHOR_SOURCE
                && e.kind == KIND_ANCHORED
                && e.payload.as_ref().and_then(parse_anchored).is_some_and(|a| {
                    a.stmt.head_hash == want && format!("parent-{}", a.ack.user_seq) == receipt.tx_id
                })
        }))
    }

    fn backend_name(&self) -> &str {
        "parent"
    }
}

#[cfg(test)]
#[path = "chain_anchor_parent_tests.rs"]
mod tests;
