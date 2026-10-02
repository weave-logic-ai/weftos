//! Tests for [`super`]: a stub parent stands in for the user daemon.

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

const T0: i64 = 1_790_000_000;

fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(T0 + secs, 0).unwrap()
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}

const ID: &str = "01JABCDEFGHJKMNPQRSTVWXYZ0";

/// Enforces the user daemon's seq / prev rules (the full verification list
/// is tested against the real RPC in `clawft-weave`).
#[derive(Default)]
struct StubParent {
    down: AtomicBool,
    /// Accept the statement, then report the link as down (lost answer).
    lose_ack: AtomicBool,
    calls: Mutex<Vec<ProjectAnchorStmt>>,
    last: Mutex<Option<(ProjectAnchorStmt, AnchorAck)>>,
    /// Public keys the parent reports as the project's certificate history.
    history: Mutex<Vec<[u8; 32]>>,
}

impl StubParent {
    fn calls(&self) -> Vec<ProjectAnchorStmt> {
        self.calls.lock().unwrap().clone()
    }
    fn accepted(&self) -> Option<(ProjectAnchorStmt, AnchorAck)> {
        self.last.lock().unwrap().clone()
    }
}

impl ParentTransport for StubParent {
    fn submit(&self, stmt: &ProjectAnchorStmt) -> Result<AnchorAck, AnchorSubmitError> {
        self.calls.lock().unwrap().push(stmt.clone());
        if self.down.load(Ordering::SeqCst) {
            return Err(AnchorSubmitError::Unreachable("connection refused".into()));
        }
        let mut last = self.last.lock().unwrap();
        if let Some((l, ack)) = last.as_ref() {
            if l == stmt {
                return Ok(ack.clone());
            }
        }
        let want = last.as_ref().map_or(1, |(l, _)| l.seq + 1);
        let prev = last.as_ref().map(|(l, _)| l.hash());
        if stmt.seq != want || stmt.prev_anchor != prev {
            return Err(AnchorSubmitError::Rejected {
                kind: "anchor_seq".into(),
                message: "not the next statement".into(),
                last: last.clone().map(Box::new),
                key_history: self.history.lock().unwrap().clone(),
            });
        }
        let ack = AnchorAck { user_seq: 100 + stmt.seq, user_event_hash: format!("{:064x}", stmt.seq) };
        *last = Some((stmt.clone(), ack.clone()));
        if self.lose_ack.load(Ordering::SeqCst) {
            return Err(AnchorSubmitError::Unreachable("reset after accept".into()));
        }
        Ok(ack)
    }
}

struct Fx {
    _t: tempfile::TempDir,
    chain: Arc<ChainManager>,
    parent: Arc<StubParent>,
    pending: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let t = tempfile::tempdir().unwrap();
        let chain = Arc::new(ChainManager::new(0, 100_000));
        for i in 0..3 {
            chain.append("kernel", "boot", Some(json!({ "i": i })));
        }
        Self { pending: t.path().join("run/anchor.pending.jsonl"), _t: t, chain, parent: Arc::default() }
    }

    fn anchor(&self) -> ParentAnchor {
        self.anchor_with(key())
    }

    fn anchor_with(&self, key: SigningKey) -> ParentAnchor {
        ParentAnchor::new(
            self.chain.clone(),
            key,
            ID,
            1,
            self.parent.clone(),
            &self.pending,
            ParentAnchorConfig::default(),
        )
    }

    fn grow(&self, n: u64) {
        for i in 0..n {
            self.chain.append("kernel", "work", Some(json!({ "i": i })));
        }
    }
}

fn anchored_events(chain: &ChainManager) -> Vec<ChainEvent> {
    chain
        .tail(0)
        .into_iter()
        .filter(|e| e.source == ANCHOR_SOURCE && e.kind == KIND_ANCHORED)
        .collect()
}

#[test]
fn success_appends_anchored_and_chains_statements() {
    let f = Fx::new();
    let a = f.anchor();
    let head = f.chain.head_hash();
    let r = a.anchor_head_at(at(0), true).unwrap();
    assert_eq!(r.hash, head);
    assert_eq!(r.tx_id, "parent-101");
    let evs = anchored_events(&f.chain);
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].payload.as_ref().unwrap()["user_seq"], 101);
    assert!(f.pending.metadata().is_err(), "pending cleared on success");
    assert!(a.verify(&r).unwrap());

    f.grow(5);
    a.anchor_head_at(at(1), true).unwrap();
    let calls = f.parent.calls();
    assert_eq!((calls[0].seq, calls[1].seq), (1, 2));
    assert_eq!(calls[0].prev_anchor, None);
    assert_eq!(calls[1].prev_anchor.as_deref(), Some(calls[0].hash().as_str()));
    calls[1].verify(&key().verifying_key().to_bytes()).unwrap();
}

#[test]
fn own_acknowledgement_is_not_new_work() {
    let f = Fx::new();
    let a = f.anchor();
    a.anchor_head_at(at(0), true).unwrap();
    // Only `project.anchored` is newer than the anchored head.
    a.anchor_head_at(at(1), true).unwrap();
    assert_eq!(f.parent.calls().len(), 1);
    assert_eq!(anchored_events(&f.chain).len(), 1);
}

#[test]
fn restart_continues_the_sequence_from_the_chain() {
    let f = Fx::new();
    f.anchor().anchor_head_at(at(0), true).unwrap();
    f.grow(2);
    let again = f.anchor();
    assert_eq!(again.last_accepted().map(|l| l.0), Some(1));
    again.anchor_head_at(at(1), true).unwrap();
    assert_eq!(f.parent.calls()[1].seq, 2);
}

#[test]
fn parent_down_writes_pending_and_the_project_keeps_appending() {
    let f = Fx::new();
    f.parent.down.store(true, Ordering::SeqCst);
    let a = f.anchor();
    f.grow(10);
    let err = a.anchor_head_at(at(0), false).unwrap_err();
    assert!(err.contains("unreachable"), "{err}");
    let p = a.pending().expect("pending written");
    assert_eq!(p.seq, 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(f.pending.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }
    // The chain is unaffected by the outage.
    let before = f.chain.sequence();
    f.grow(3);
    assert_eq!(f.chain.sequence(), before + 3);
    assert!(anchored_events(&f.chain).is_empty());
}

#[test]
fn only_the_latest_pending_statement_is_replayed_after_a_long_outage() {
    let f = Fx::new();
    f.parent.down.store(true, Ordering::SeqCst);
    let a = f.anchor();
    for round in 0..4 {
        f.grow(120);
        a.anchor_head_at(at(round * 400), true).unwrap_err();
    }
    let latest = a.pending().unwrap();
    assert_eq!(latest.seq, 1, "seq counts accepted statements, not attempts");
    assert_eq!(latest.head_seq, f.chain.sequence() - 1);
    assert_eq!(std::fs::read_to_string(&f.pending).unwrap().lines().count(), 1);

    f.parent.down.store(false, Ordering::SeqCst);
    f.parent.calls.lock().unwrap().clear();
    // Four failures, last at 1200 s: the backoff is 40 s, so 1210 s is too early.
    assert!(a.retry_pending_at(at(1210)).unwrap().is_none());
    assert!(f.parent.calls().is_empty());
    let r = a.retry_pending_at(at(1600 + 3600)).unwrap().expect("replayed");
    assert_eq!(r.tx_id, "parent-101");
    let calls = f.parent.calls();
    assert_eq!(calls.len(), 1, "exactly one replay");
    assert_eq!(calls[0], latest, "the statement is resent as written");
    assert_eq!(f.parent.accepted().unwrap().0.seq, 1, "earlier heads were never accepted");
    assert_eq!(anchored_events(&f.chain).len(), 1);
    assert!(a.retry_pending_at(at(9000)).unwrap().is_none(), "replayed once");
    assert!(a.pending().is_none());
}

#[test]
fn backoff_spaces_retries() {
    let f = Fx::new();
    f.parent.down.store(true, Ordering::SeqCst);
    let a = f.anchor();
    a.anchor_head_at(at(0), false).unwrap_err();
    f.grow(1);
    a.anchor_head_at(at(1), false).unwrap_err();
    assert_eq!(f.parent.calls().len(), 1, "inside the backoff window: no send");
    assert_eq!(a.pending().unwrap().head_seq, f.chain.sequence() - 1, "but the latest head is kept");
    a.anchor_head_at(at(6), false).unwrap_err();
    assert_eq!(f.parent.calls().len(), 2);
    // Doubled: 10 s now.
    a.anchor_head_at(at(12), false).unwrap_err();
    assert_eq!(f.parent.calls().len(), 2);
    a.anchor_head_at(at(17), false).unwrap_err();
    assert_eq!(f.parent.calls().len(), 3);
}

#[test]
fn kill_after_send_before_acknowledgement_replays_idempotently() {
    let f = Fx::new();
    f.parent.lose_ack.store(true, Ordering::SeqCst);
    let a = f.anchor();
    a.anchor_head_at(at(0), true).unwrap_err();
    // The parent has it; the project chain does not, but the statement is on disk.
    assert_eq!(f.parent.accepted().unwrap().0.seq, 1);
    assert!(anchored_events(&f.chain).is_empty());
    drop(a);

    f.parent.lose_ack.store(false, Ordering::SeqCst);
    let restarted = f.anchor();
    assert_eq!(restarted.pending().unwrap().seq, 1);
    restarted.retry_pending_at(at(1)).unwrap().expect("replayed");
    assert_eq!(f.parent.accepted().unwrap().0.seq, 1, "no second acceptance");
    assert_eq!(anchored_events(&f.chain).len(), 1);
    assert_eq!(restarted.last_accepted().unwrap().0, 1);
    assert!(restarted.pending().is_none());
}

#[test]
fn lost_acknowledgement_followed_by_a_newer_head_resynchronises() {
    let f = Fx::new();
    f.parent.lose_ack.store(true, Ordering::SeqCst);
    let a = f.anchor();
    a.anchor_head_at(at(0), true).unwrap_err();
    f.parent.lose_ack.store(false, Ordering::SeqCst);
    f.grow(150);
    // The new statement also says seq 1; the parent refuses and sends its last.
    let r = a.anchor_head_at(at(10), true).unwrap();
    assert_eq!(r.tx_id, "parent-102");
    assert_eq!(a.last_accepted().unwrap().0, 2);
    let evs = anchored_events(&f.chain);
    assert_eq!(evs.len(), 2, "the lost acknowledgement was recorded too");
    let calls = f.parent.calls();
    assert_eq!(calls.last().unwrap().seq, 2);
    assert_eq!(calls.last().unwrap().head_seq, f.chain.sequence() - 2);
}

#[test]
fn kill_after_acknowledgement_before_pending_is_cleared_is_consistent() {
    let f = Fx::new();
    let a = f.anchor();
    a.anchor_head_at(at(0), true).unwrap();
    // Simulate the kill: the accepted statement is back on disk.
    let accepted = f.parent.accepted().unwrap().0;
    std::fs::create_dir_all(f.pending.parent().unwrap()).unwrap();
    std::fs::write(&f.pending, format!("{}\n", serde_json::to_string(&accepted).unwrap())).unwrap();
    drop(a);
    let restarted = f.anchor();
    assert!(restarted.pending().is_none(), "covered pending is discarded");
    assert!(restarted.retry_pending_at(at(5)).unwrap().is_none());
    assert_eq!(f.parent.calls().len(), 1);
}

#[test]
fn rejection_without_a_usable_last_statement_backs_off() {
    struct Refuse;
    impl ParentTransport for Refuse {
        fn submit(&self, _: &ProjectAnchorStmt) -> Result<AnchorAck, AnchorSubmitError> {
            Err(AnchorSubmitError::Rejected {
                kind: "anchor_key_revoked".into(),
                message: "revoked".into(),
                last: None,
                key_history: vec![],
            })
        }
    }
    let f = Fx::new();
    let a = ParentAnchor::new(f.chain.clone(), key(), ID, 1, Arc::new(Refuse), &f.pending, Default::default());
    let e = a.anchor_head_at(at(0), true).unwrap_err();
    assert!(e.contains("anchor_key_revoked"), "{e}");
    assert!(a.pending().is_some());
    assert_eq!(a.last_error().as_deref(), Some(e.as_str()));
}

#[test]
fn chain_anchor_trait_anchors_a_given_event() {
    let f = Fx::new();
    let a = f.anchor();
    let r = ChainAnchor::anchor(&a, &f.chain.head_hash()).unwrap();
    assert_eq!(a.backend_name(), "parent");
    assert!(ChainAnchor::verify(&a, &r).unwrap());
    assert!(ChainAnchor::anchor(&a, &[7u8; 32]).is_err());
}

#[test]
fn rekey_then_restart_continues_at_n_plus_one() {
    let f = Fx::new();
    let k1 = key();
    let k2 = SigningKey::from_bytes(&[10u8; 32]);
    let a1 = f.anchor_with(k1);
    a1.anchor_head_at(at(0), true).unwrap();
    f.grow(3);
    a1.anchor_head_at(at(1), true).unwrap();
    assert_eq!(a1.last_accepted().unwrap().0, 2);
    drop(a1);
    f.grow(200);

    // Restart under the new key: our own chain's K1 statements are the baseline.
    let a2 = f.anchor_with(k2.clone());
    assert_eq!(a2.last_accepted().unwrap().0, 2);
    let r = a2.anchor_head_at(at(100), true).unwrap();
    assert_eq!(a2.last_accepted().unwrap().0, 3, "continues at N + 1");
    let calls = f.parent.calls();
    let last = calls.last().unwrap();
    assert_eq!(last.seq, 3);
    assert_eq!(last.prev_anchor.as_deref(), Some(calls[1].hash().as_str()));
    last.verify(&k2.verifying_key().to_bytes()).unwrap();
    assert!(a2.verify(&r).unwrap());

    f.grow(3);
    drop(a2);
    let a3 = f.anchor_with(k2);
    assert_eq!(a3.last_accepted().unwrap().0, 3);
    a3.anchor_head_at(at(200), true).unwrap();
    assert_eq!(f.parent.calls().last().unwrap().seq, 4);
}

#[test]
fn a_parent_statement_under_another_key_is_never_adopted() {
    let f = Fx::new();
    let a = f.anchor();
    // The parent's "last" is signed by a key that is not ours, even though
    // it is the right shape and names a head our chain has.
    let head = f.chain.tail(1)[0].clone();
    let other = SigningKey::from_bytes(&[11u8; 32]);
    let theirs = ProjectAnchorStmt {
        project_id: ID.into(),
        project_key_id: String::new(),
        cert_serial: 1,
        seq: 1,
        chain_id: 0,
        head_hash: hex_encode(&head.hash),
        head_seq: head.sequence,
        rule_hash: "00".repeat(32),
        at: ts(at(0)),
        prev_anchor: None,
        sig: String::new(),
    }
    .sign(&other);
    *f.parent.last.lock().unwrap() =
        Some((theirs, AnchorAck { user_seq: 1, user_event_hash: "00".repeat(32) }));
    // Even a parent-supplied key history naming that key buys nothing.
    *f.parent.history.lock().unwrap() = vec![other.verifying_key().to_bytes()];
    f.grow(2);
    let e = a.anchor_head_at(at(1), true).unwrap_err();
    assert!(e.contains("not adopted") && e.contains("current project key"), "{e}");
    assert_eq!(a.last_accepted(), None);
    assert!(anchored_events(&f.chain).is_empty());
}

#[test]
fn adoption_requires_our_chain_to_hold_the_head() {
    let f = Fx::new();
    let a = f.anchor();
    // The parent claims a head we never produced.
    let foreign = ProjectAnchorStmt {
        project_id: ID.into(),
        project_key_id: String::new(),
        cert_serial: 1,
        seq: 1,
        chain_id: 0,
        head_hash: "ee".repeat(32),
        head_seq: 2,
        rule_hash: "00".repeat(32),
        at: ts(at(0)),
        prev_anchor: None,
        sig: String::new(),
    }
    .sign(&key());
    *f.parent.last.lock().unwrap() =
        Some((foreign, AnchorAck { user_seq: 1, user_event_hash: "00".repeat(32) }));
    f.grow(5);
    let e = a.anchor_head_at(at(1), true).unwrap_err();
    assert!(e.contains("did not produce"), "{e}");
    assert!(anchored_events(&f.chain).is_empty());
    assert_eq!(a.last_accepted(), None);
}

#[test]
fn a_hung_transport_times_out_without_blocking_readers() {
    use std::sync::mpsc;
    struct Hang(mpsc::Sender<()>);
    impl ParentTransport for Hang {
        fn submit(&self, _: &ProjectAnchorStmt) -> Result<AnchorAck, AnchorSubmitError> {
            let _ = self.0.send(());
            std::thread::sleep(std::time::Duration::from_secs(3));
            Err(AnchorSubmitError::Unreachable("late".into()))
        }
    }
    let f = Fx::new();
    let (tx, entered) = mpsc::channel();
    let cfg = ParentAnchorConfig { submit_timeout_secs: 1, ..Default::default() };
    let a = Arc::new(ParentAnchor::new(f.chain.clone(), key(), ID, 1, Arc::new(Hang(tx)), &f.pending, cfg));
    let worker = {
        let a = a.clone();
        std::thread::spawn(move || a.anchor_head_at(at(0), true))
    };
    entered.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    // The submission is in flight: readers must not wait for it.
    let (rtx, rrx) = mpsc::channel();
    let reader = a.clone();
    std::thread::spawn(move || {
        let _ = rtx.send((reader.last_accepted(), reader.last_error(), reader.pending()));
    });
    let (last, _err, pending) = rrx.recv_timeout(std::time::Duration::from_millis(500)).expect("readers are not blocked");
    assert_eq!(last, None);
    assert!(pending.is_some(), "write-ahead statement is visible");
    let e = worker.join().unwrap().unwrap_err();
    assert!(e.contains("no answer within 1 s"), "{e}");
    assert!(a.pending().is_some());
    // The abandoned worker is still running: no second thread is spawned.
    f.grow(1);
    let e2 = a.anchor_head_at(at(1), true).unwrap_err();
    assert!(e2.contains("previous submit still in flight"), "{e2}");
    assert!(entered.try_recv().is_err(), "the transport was entered only once");
}
