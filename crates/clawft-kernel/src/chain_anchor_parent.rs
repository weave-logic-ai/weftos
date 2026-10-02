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
//! Honest limit: the user daemon attests "this key claimed head X at time
//! T"; it cannot verify X without subscribing to the project chain.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use clawft_types::project::canon::{hex_decode, hex_encode};
use clawft_types::project::cert::{ProjectAnchorStmt, ts};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

use super::ANCHOR_SOURCE;
use crate::chain::{AnchorReceipt, ChainAnchor, ChainEvent, ChainManager};

/// Kind of the user-chain event the user daemon appends per accepted statement.
pub const KIND_ANCHOR: &str = "project.anchor";
/// Kind of the project-chain acknowledgement.
pub const KIND_ANCHORED: &str = "project.anchored";

/// The user daemon's acknowledgement of an accepted statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorAck {
    /// Sequence of the user-chain `project.anchor` event.
    pub user_seq: u64,
    /// Hash of that event, hex.
    pub user_event_hash: String,
}

/// Why a submission failed.
#[derive(Debug, Clone)]
pub enum AnchorSubmitError {
    /// The user daemon could not be reached (down, socket gone, timeout).
    Unreachable(String),
    /// The user daemon answered and refused. `last` is its last accepted
    /// statement for this project when the refusal was about `seq` or
    /// `prev_anchor`.
    Rejected {
        /// Daemon `error_kind`.
        kind: String,
        /// Human message.
        message: String,
        /// Its last accepted statement and acknowledgement.
        last: Option<Box<(ProjectAnchorStmt, AnchorAck)>>,
    },
}

/// Sends statements to the user daemon.
pub trait ParentTransport: Send + Sync {
    /// `project.anchor.submit`. An identical resubmission of the last
    /// accepted statement must return the original acknowledgement.
    fn submit(&self, stmt: &ProjectAnchorStmt) -> Result<AnchorAck, AnchorSubmitError>;
}

/// Retry backoff.
#[derive(Debug, Clone, Copy)]
pub struct ParentAnchorConfig {
    /// First retry delay, seconds.
    pub backoff_base_secs: i64,
    /// Longest retry delay, seconds.
    pub backoff_max_secs: i64,
}

impl Default for ParentAnchorConfig {
    fn default() -> Self {
        Self { backoff_base_secs: 5, backoff_max_secs: 300 }
    }
}

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
    /// from the chain and drops a pending statement that is already covered.
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
            state: Mutex::new(State::default()),
        };
        {
            let mut st = this.state.lock().unwrap();
            st.last = this.recover();
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

    fn recover(&self) -> Option<Accepted> {
        let pk = self.pubkey();
        let mut best: Option<Accepted> = None;
        for e in self.chain.tail(0) {
            if e.source != ANCHOR_SOURCE || e.kind != KIND_ANCHORED {
                continue;
            }
            let Some(acc) = e.payload.as_ref().and_then(parse_anchored) else { continue };
            if acc.stmt.project_id != self.project_id || acc.stmt.verify(&pk).is_err() {
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

    /// Nothing new since the last accepted statement (own acknowledgement
    /// events do not count).
    fn covered(&self, last: &Accepted, ev: &ChainEvent) -> bool {
        ev.sequence <= last.stmt.head_seq
            || (ev.sequence == last.stmt.head_seq + 1
                && ev.source == ANCHOR_SOURCE
                && ev.kind == KIND_ANCHORED)
    }

    fn build(&self, st: &State, ev: &ChainEvent, now: DateTime<Utc>) -> ProjectAnchorStmt {
        ProjectAnchorStmt {
            project_id: self.project_id.clone(),
            project_key_id: String::new(),
            cert_serial: *self.cert_serial.lock().unwrap(),
            seq: st.last.as_ref().map_or(1, |l| l.stmt.seq + 1),
            chain_id: u64::from(ev.chain_id),
            head_hash: hex_encode(&ev.hash),
            head_seq: ev.sequence,
            rule_hash: hex_encode(&ev.rule_hash.unwrap_or([0u8; 32])),
            at: ts(now),
            prev_anchor: st.last.as_ref().map(|l| l.stmt.hash()),
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

    fn fail(&self, st: &mut State, now: DateTime<Utc>, why: String) -> String {
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
    fn record(&self, st: &mut State, stmt: ProjectAnchorStmt, ack: AnchorAck) {
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
        st.last = Some(Accepted { stmt, ack });
        st.failures = 0;
        st.retry_at = None;
        st.last_error = None;
    }

    /// Take the parent's last accepted statement as ours when it verifies
    /// under our key and extends what we know (an acknowledgement we lost).
    fn adopt(&self, st: &mut State, last: (ProjectAnchorStmt, AnchorAck)) -> bool {
        let (stmt, ack) = last;
        let ours = st.last.as_ref();
        let extends = stmt.project_id == self.project_id
            && stmt.verify(&self.pubkey()).is_ok()
            && stmt.seq == ours.map_or(1, |l| l.stmt.seq + 1)
            && stmt.prev_anchor == ours.map(|l| l.stmt.hash());
        if extends {
            self.record(st, stmt, ack);
        }
        extends
    }

    fn deliver(
        &self,
        st: &mut State,
        mut stmt: ProjectAnchorStmt,
        now: DateTime<Utc>,
    ) -> Result<AnchorReceipt, String> {
        for attempt in 0..2 {
            match self.transport.submit(&stmt) {
                Ok(ack) => {
                    self.record(st, stmt, ack);
                    return Ok(receipt_of(st.last.as_ref().unwrap(), now));
                }
                Err(AnchorSubmitError::Unreachable(m)) => {
                    return Err(self.fail(st, now, format!("parent unreachable: {m}")));
                }
                Err(AnchorSubmitError::Rejected { kind, message, last }) => {
                    if attempt == 0 && let Some(last) = last && self.adopt(st, *last) {
                        let head = self.head_event()?;
                        let acc = st.last.as_ref().unwrap();
                        if self.covered(acc, &head) || head.sequence <= acc.stmt.head_seq {
                            return Ok(receipt_of(acc, now));
                        }
                        stmt = self.build(st, &head, now);
                        self.write_pending(&stmt)?;
                        continue;
                    }
                    return Err(self.fail(st, now, format!("parent rejected anchor ({kind}): {message}")));
                }
            }
        }
        Err(self.fail(st, now, "parent rejected the rebuilt anchor".into()))
    }

    fn anchor_event(&self, ev: ChainEvent, now: DateTime<Utc>, force: bool) -> Result<AnchorReceipt, String> {
        let mut st = self.state.lock().map_err(|_| "parent anchor lock poisoned".to_string())?;
        if let Some(acc) = &st.last
            && self.covered(acc, &ev)
        {
            return Ok(receipt_of(acc, now));
        }
        let stmt = self.build(&st, &ev, now);
        self.write_pending(&stmt)?;
        if !force && let Some(t) = st.retry_at && now < t {
            return Err(format!("parent unreachable; next retry after {}", ts(t)));
        }
        self.deliver(&mut st, stmt, now)
    }

    /// Anchor the current head now, ignoring the frequency policy but not
    /// the retry backoff unless `force` (graceful shutdown passes `true`).
    pub fn anchor_head_at(&self, now: DateTime<Utc>, force: bool) -> Result<AnchorReceipt, String> {
        self.anchor_event(self.head_event()?, now, force)
    }

    /// [`Self::anchor_head_at`] for the wall clock, forced: the shutdown anchor.
    pub fn anchor_head(&self) -> Result<AnchorReceipt, String> {
        self.anchor_head_at(Utc::now(), true)
    }

    /// Resend the latest pending statement exactly as written once its
    /// backoff has elapsed. `Ok(None)`: nothing pending, or not due yet.
    pub fn retry_pending_at(&self, now: DateTime<Utc>) -> Result<Option<AnchorReceipt>, String> {
        let mut st = self.state.lock().map_err(|_| "parent anchor lock poisoned".to_string())?;
        let Some(stmt) = self.read_pending() else { return Ok(None) };
        if st.last.as_ref().is_some_and(|l| stmt.seq <= l.stmt.seq) {
            let _ = std::fs::remove_file(&self.pending_path);
            return Ok(None);
        }
        if st.retry_at.is_some_and(|t| now < t) {
            return Ok(None);
        }
        self.deliver(&mut st, stmt, now).map(Some)
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
