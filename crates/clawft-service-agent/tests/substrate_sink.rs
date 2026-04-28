//! Integration tests for [`SubstrateConversationSink`].
//!
//! Exercises the public `ConversationSink` impl + heartbeat lifecycle
//! against a `Mutex<HashMap>` stub of the [`SubstrateClient`] trait
//! seam. Lives in `tests/` rather than inline so `substrate_sink.rs`
//! stays under the 500-line file ceiling per CLAUDE.md.
//!
//! Tests cover the `agent-core-v1.md` Phase C3 acceptance criteria:
//! - Per-turn JSONL lands at `substrate/_derived/chat/<conv>/turns/<ulid>`.
//! - ULID-keyed paths sort monotonically even within the same ms.
//! - Status path `substrate/_derived/chat/<conv>/status` overwrites
//!   in place on each heartbeat.
//! - `load_history` returns turns sorted by `ts_ms`.
//! - Heartbeat task starts/stops cleanly; idempotent; survives
//!   neither `stop_heartbeat` nor sink `Drop`.
//! - `TurnContent::{Text|Audio|Mixed}` serde round-trip (forward-
//!   compat plumbing for voice).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::Duration;

use clawft_core::agent::sink::{ConversationSink, Turn};
use clawft_service_agent::{
    AudioRef, HEARTBEAT_PERIOD, SubstrateClient, SubstrateConversationSink, TurnContent,
    TurnContentPart,
};
use serde_json::Value;

/// `Mutex<HashMap>` stub — exercises sink semantics without a full
/// kernel. Tracks every publish (path → value) plus a write counter
/// so the heartbeat tests can assert "≥ N writes".
#[derive(Default)]
struct StubClient {
    store: StdMutex<HashMap<String, Value>>,
    writes: AtomicUsize,
}

impl StubClient {
    fn writes(&self) -> usize {
        self.writes.load(AtomicOrdering::Acquire)
    }
    fn snapshot(&self) -> HashMap<String, Value> {
        self.store.lock().unwrap().clone()
    }
}

impl SubstrateClient for StubClient {
    fn publish(&self, _node_id: &str, path: &str, value: Value) -> Result<u64, String> {
        let mut g = self.store.lock().unwrap();
        g.insert(path.to_string(), value);
        let tick = self.writes.fetch_add(1, AtomicOrdering::AcqRel) as u64 + 1;
        Ok(tick)
    }
    fn list(&self, prefix: &str, _depth: u32) -> Result<Vec<String>, String> {
        let g = self.store.lock().unwrap();
        let with_sep = format!("{prefix}/");
        let mut out: Vec<String> = g
            .keys()
            .filter(|k| k.starts_with(&with_sep))
            .cloned()
            .collect();
        out.sort();
        Ok(out)
    }
    fn read(&self, path: &str) -> Result<Option<Value>, String> {
        Ok(self.store.lock().unwrap().get(path).cloned())
    }
}

fn mk_sink(stub: Arc<StubClient>, period: Duration) -> Arc<SubstrateConversationSink> {
    Arc::new(SubstrateConversationSink::with_client(
        stub, "n-test", period,
    ))
}

fn turn_text(role: &str, content: &str, ts_ms: u64) -> Turn {
    Turn {
        turn_id: String::new(),
        role: role.into(),
        content: content.into(),
        tool_calls: None,
        tool_call_id: None,
        ts_ms,
    }
}

#[tokio::test]
async fn append_turn_writes_to_substrate() {
    let stub = Arc::new(StubClient::default());
    let sink = mk_sink(Arc::clone(&stub), HEARTBEAT_PERIOD);

    sink.append_turn("c1", turn_text("user", "hello", 1_700_000_000_000))
        .await
        .unwrap();

    let snap = stub.snapshot();
    assert_eq!(snap.len(), 1);
    let (path, val) = snap.iter().next().unwrap();
    assert!(
        path.starts_with("substrate/_derived/chat/c1/turns/"),
        "unexpected path: {path}"
    );
    assert_eq!(val["role"], "user");
    assert_eq!(val["content"], "hello");
    assert_eq!(val["ts_ms"], 1_700_000_000_000u64);
    assert_eq!(val["content_type"], "text");
}

#[tokio::test]
async fn append_turns_are_monotonic() {
    // Two appends within the same ms must produce sortable ids.
    let stub = Arc::new(StubClient::default());
    let sink = mk_sink(Arc::clone(&stub), HEARTBEAT_PERIOD);

    sink.append_turn("c", turn_text("user", "a", 1_700_000_000_000))
        .await
        .unwrap();
    sink.append_turn("c", turn_text("assistant", "b", 1_700_000_000_000))
        .await
        .unwrap();

    let snap = stub.snapshot();
    let mut paths: Vec<String> = snap.keys().cloned().collect();
    paths.sort();
    assert_eq!(paths.len(), 2);
    for p in &paths {
        assert!(p.starts_with("substrate/_derived/chat/c/turns/"));
    }
    // First write's role is "user", second is "assistant" —
    // sortable ids must preserve append order.
    let first = snap.get(&paths[0]).unwrap();
    let second = snap.get(&paths[1]).unwrap();
    assert_eq!(first["role"], "user");
    assert_eq!(second["role"], "assistant");
}

#[tokio::test]
async fn publish_status_overwrites() {
    let stub = Arc::new(StubClient::default());
    let sink = mk_sink(Arc::clone(&stub), HEARTBEAT_PERIOD);

    sink.publish_status("c", "thinking", serde_json::json!({"step": 1}))
        .await
        .unwrap();
    sink.publish_status("c", "done", serde_json::json!({"step": 2}))
        .await
        .unwrap();

    let snap = stub.snapshot();
    // Single status path; overwritten in place.
    let v = snap
        .get("substrate/_derived/chat/c/status")
        .expect("status path written");
    assert_eq!(v["status"], "done");
    assert_eq!(v["payload"]["step"], 2);
}

#[tokio::test]
async fn load_history_returns_in_order() {
    // Append three turns out of timestamp order; load_history must
    // return them sorted ascending by ts_ms.
    let stub = Arc::new(StubClient::default());
    let sink = mk_sink(Arc::clone(&stub), HEARTBEAT_PERIOD);

    sink.append_turn("c", turn_text("user", "second", 200))
        .await
        .unwrap();
    sink.append_turn("c", turn_text("user", "third", 300))
        .await
        .unwrap();
    sink.append_turn("c", turn_text("user", "first", 100))
        .await
        .unwrap();

    let history = sink.load_history("c").await.unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].content, "first");
    assert_eq!(history[1].content, "second");
    assert_eq!(history[2].content, "third");
}

#[tokio::test]
async fn start_heartbeat_periodically_publishes() {
    let stub = Arc::new(StubClient::default());
    let sink = mk_sink(Arc::clone(&stub), Duration::from_millis(50));

    sink.start_heartbeat("c");
    // 250ms / 50ms = 5 ticks; the first interval.tick() returns
    // immediately and is dropped, so we expect ~4 actual writes.
    // Assert ≥3 to give the scheduler some slack.
    tokio::time::sleep(Duration::from_millis(260)).await;
    sink.stop_heartbeat("c");

    let writes = stub.writes();
    assert!(writes >= 3, "expected ≥3 heartbeat writes, got {writes}");
    let snap = stub.snapshot();
    let v = snap
        .get("substrate/_derived/chat/c/status")
        .expect("status path written");
    assert_eq!(v["status"], "alive");
}

#[tokio::test]
async fn stop_heartbeat_terminates_task() {
    let stub = Arc::new(StubClient::default());
    let sink = mk_sink(Arc::clone(&stub), Duration::from_millis(50));

    sink.start_heartbeat("c");
    tokio::time::sleep(Duration::from_millis(160)).await;
    sink.stop_heartbeat("c");
    let writes_at_stop = stub.writes();

    // Wait long enough for several more ticks to NOT happen.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let writes_after = stub.writes();
    assert_eq!(
        writes_at_stop, writes_after,
        "heartbeat continued after stop (stop={writes_at_stop}, after={writes_after})"
    );
    assert_eq!(sink.live_heartbeats(), 0);
}

#[tokio::test]
async fn start_heartbeat_is_idempotent() {
    let stub = Arc::new(StubClient::default());
    let sink = mk_sink(Arc::clone(&stub), Duration::from_millis(50));

    sink.start_heartbeat("c");
    sink.start_heartbeat("c");
    sink.start_heartbeat("c");
    assert_eq!(sink.live_heartbeats(), 1);
    sink.stop_heartbeat("c");
}

#[tokio::test]
async fn drop_aborts_outstanding_heartbeats() {
    // Sanity: dropping the sink without an explicit stop must not
    // leak the heartbeat task. The task holds a `Weak<Self>`, so
    // when the last `Arc<Self>` (other than the task's own counter)
    // drops, the task's next upgrade fails and it exits cleanly.
    let stub = Arc::new(StubClient::default());
    {
        let sink = mk_sink(Arc::clone(&stub), Duration::from_millis(50));
        sink.start_heartbeat("c");
        tokio::time::sleep(Duration::from_millis(80)).await;
        // Sink drops at scope exit.
    }
    let writes_after_drop = stub.writes();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let writes_later = stub.writes();
    assert_eq!(
        writes_after_drop, writes_later,
        "heartbeat survived sink drop"
    );
}

#[test]
fn turn_content_text_only_for_v1() {
    // The chat path constructs only Text today; round-trip through
    // serde to lock the externally-tagged wire shape.
    let c = TurnContent::Text("hello".into());
    let s = serde_json::to_string(&c).unwrap();
    assert!(s.contains("\"text\""), "wire shape: {s}");
    let back: TurnContent = serde_json::from_str(&s).unwrap();
    assert_eq!(back, c);
}

#[test]
fn turn_content_audio_serde_round_trips() {
    // Even though the chat path doesn't construct Audio, the wire
    // must serde-round-trip so future voice work doesn't reshape
    // the substrate JSONL.
    let a = TurnContent::Audio(AudioRef {
        substrate_path: "substrate/_derived/chat/c/audio/0".into(),
        mime: "audio/wav".into(),
        duration_ms: 1_500,
    });
    let s = serde_json::to_string(&a).unwrap();
    let back: TurnContent = serde_json::from_str(&s).unwrap();
    assert_eq!(back, a);
}

#[test]
fn turn_content_mixed_serde_round_trips() {
    let m = TurnContent::Mixed(vec![
        TurnContentPart::Text("hi ".into()),
        TurnContentPart::Audio(AudioRef {
            substrate_path: "substrate/_derived/chat/c/audio/1".into(),
            mime: "audio/opus".into(),
            duration_ms: 300,
        }),
    ]);
    let s = serde_json::to_string(&m).unwrap();
    let back: TurnContent = serde_json::from_str(&s).unwrap();
    assert_eq!(back, m);
}
