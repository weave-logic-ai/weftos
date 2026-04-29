//! Integration tests for [`AgentService`].
//!
//! These exercise only the public surface (no `super::*`-style reach
//! into private adapters) and prove the lock + cancel + shutdown
//! semantics promised by `docs/plans/agent-core-v1.md` Phase C1.
//!
//! Lives in `tests/` rather than inline so `service.rs` stays under
//! the 500-line file ceiling per CLAUDE.md. Inline unit tests for the
//! private adapter functions (`inbound_from_params`,
//! `result_from_outbound`) remain in `service.rs::tests`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use clawft_service_agent::{
    AgentChatMessage, AgentChatParams, AgentLoopHandle, AgentService, AgentServiceError,
};
use clawft_types::event::{InboundMessage, OutboundMessage};
use tokio::sync::Notify;

/// Stub loop handle. Each call to `handle_turn` awaits an external
/// `release` Notify and tracks how many concurrent calls were live
/// at peak — that's how the parallel-vs-serial tests assert lock
/// behaviour without observing internal state of [`AgentService`].
struct StubHandle {
    release: Arc<Notify>,
    in_progress: Arc<AtomicUsize>,
    peak_concurrent: Arc<AtomicUsize>,
}

impl StubHandle {
    fn new(release: Arc<Notify>) -> Self {
        Self {
            release,
            in_progress: Arc::new(AtomicUsize::new(0)),
            peak_concurrent: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl AgentLoopHandle for StubHandle {
    async fn handle_turn(&self, msg: InboundMessage) -> Result<OutboundMessage, String> {
        let now = self.in_progress.fetch_add(1, Ordering::AcqRel) + 1;
        // Peak-concurrency tracking — CAS loop so we don't lose to a
        // racing peer.
        let mut peak = self.peak_concurrent.load(Ordering::Acquire);
        while now > peak {
            match self.peak_concurrent.compare_exchange(
                peak,
                now,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => peak = actual,
            }
        }
        self.release.notified().await;
        self.in_progress.fetch_sub(1, Ordering::AcqRel);
        Ok(OutboundMessage {
            channel: msg.channel,
            chat_id: msg.chat_id,
            content: format!("echo: {}", msg.content),
            reply_to: None,
            media: Vec::new(),
            metadata: HashMap::new(),
        })
    }
}

fn params_for(conv_id: &str, content: &str) -> AgentChatParams {
    AgentChatParams {
        messages: vec![AgentChatMessage {
            role: "user".into(),
            content: content.into(),
        }],
        temperature: None,
        max_tokens: None,
        conv_id: conv_id.into(),
    }
}

#[tokio::test]
async fn dispatch_acquires_per_conv_lock() {
    // Two concurrent dispatches with the SAME conv_id must serialise:
    // peak concurrency in the stub stays at 1 throughout.
    let release = Arc::new(Notify::new());
    let stub = Arc::new(StubHandle::new(Arc::clone(&release)));
    let svc = Arc::new(AgentService::new(Arc::clone(&stub)));

    let svc1 = Arc::clone(&svc);
    let svc2 = Arc::clone(&svc);
    let h1 = tokio::spawn(async move { svc1.dispatch(params_for("c1", "first")).await });
    let h2 = tokio::spawn(async move { svc2.dispatch(params_for("c1", "second")).await });

    // Yield enough times for both tasks to reach the lock acquire.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    // Release the first dispatch.
    release.notify_one();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    // Release the second.
    release.notify_one();

    let r1 = h1.await.unwrap().unwrap();
    let r2 = h2.await.unwrap().unwrap();
    assert_eq!(r1.assistant_text, "echo: first");
    assert_eq!(r2.assistant_text, "echo: second");
    assert_eq!(
        stub.peak_concurrent.load(Ordering::Acquire),
        1,
        "same-conv dispatches must never overlap"
    );
}

#[tokio::test]
async fn dispatch_parallel_for_distinct_conv_ids() {
    // Different conv_ids must overlap: peak concurrency reaches 2.
    let release = Arc::new(Notify::new());
    let stub = Arc::new(StubHandle::new(Arc::clone(&release)));
    let svc = Arc::new(AgentService::new(Arc::clone(&stub)));

    let svc_a = Arc::clone(&svc);
    let svc_b = Arc::clone(&svc);
    let h_a = tokio::spawn(async move { svc_a.dispatch(params_for("a", "x")).await });
    let h_b = tokio::spawn(async move { svc_b.dispatch(params_for("b", "y")).await });

    // Spin until both stubs have entered handle_turn (50ms is
    // generous in a tokio:test single-thread scheduler).
    let deadline = std::time::Instant::now() + Duration::from_millis(50);
    while stub.in_progress.load(Ordering::Acquire) < 2 {
        if std::time::Instant::now() > deadline {
            break;
        }
        tokio::task::yield_now().await;
    }

    let peak = stub.peak_concurrent.load(Ordering::Acquire);
    // Drain — `notify_waiters()` is one-shot so we may need to nudge
    // a few times.
    for _ in 0..16 {
        release.notify_waiters();
        tokio::task::yield_now().await;
    }

    h_a.await.unwrap().unwrap();
    h_b.await.unwrap().unwrap();
    assert_eq!(peak, 2, "distinct-conv dispatches must overlap");
}

#[tokio::test]
async fn cancel_aborts_in_flight_dispatch() {
    let release = Arc::new(Notify::new());
    let stub = Arc::new(StubHandle::new(Arc::clone(&release)));
    let svc = Arc::new(AgentService::new(Arc::clone(&stub)));

    let svc_clone = Arc::clone(&svc);
    let handle =
        tokio::spawn(async move { svc_clone.dispatch(params_for("c-cancel", "wait")).await });

    // Wait for the stub to actually be inside handle_turn.
    let deadline = std::time::Instant::now() + Duration::from_millis(50);
    while stub.in_progress.load(Ordering::Acquire) == 0 {
        if std::time::Instant::now() > deadline {
            panic!("stub never entered handle_turn");
        }
        tokio::task::yield_now().await;
    }

    svc.cancel("c-cancel");

    let result = handle.await.unwrap();
    match result {
        Err(AgentServiceError::Cancelled(id)) => assert_eq!(id, "c-cancel"),
        other => panic!("expected Cancelled, got {:?}", other),
    }

    // Release the stub so its task tree drops cleanly.
    release.notify_waiters();
}

#[tokio::test]
async fn cancel_on_idle_arms_next_dispatch() {
    // A `cancel()` issued before any dispatch should still fire on
    // the very next dispatch — matches the spike's
    // `agent.chat.cancel` racing with the next `agent.chat`.
    let release = Arc::new(Notify::new());
    let stub = Arc::new(StubHandle::new(Arc::clone(&release)));
    let svc = Arc::new(AgentService::new(Arc::clone(&stub)));

    svc.cancel("future-conv");

    let res = svc.dispatch(params_for("future-conv", "x")).await;
    match res {
        Err(AgentServiceError::Cancelled(id)) => assert_eq!(id, "future-conv"),
        other => panic!("expected pre-armed cancel to fire, got {:?}", other),
    }
}

#[tokio::test]
async fn shutdown_returns_true_when_idle() {
    let release = Arc::new(Notify::new());
    let stub = Arc::new(StubHandle::new(Arc::clone(&release)));
    let svc = AgentService::new(Arc::clone(&stub));

    let drained = svc.shutdown(Duration::from_millis(100)).await;
    assert!(drained, "idle shutdown must drain immediately");
}

#[tokio::test]
async fn shutdown_drains_in_flight_within_deadline() {
    let release = Arc::new(Notify::new());
    let stub = Arc::new(StubHandle::new(Arc::clone(&release)));
    let svc = Arc::new(AgentService::new(Arc::clone(&stub)));

    let svc_clone = Arc::clone(&svc);
    let dispatch_handle =
        tokio::spawn(async move { svc_clone.dispatch(params_for("c", "x")).await });

    let deadline = std::time::Instant::now() + Duration::from_millis(50);
    while stub.in_progress.load(Ordering::Acquire) == 0 {
        if std::time::Instant::now() > deadline {
            panic!("stub never entered handle_turn");
        }
        tokio::task::yield_now().await;
    }

    // Shutdown cancels every known token, the dispatch returns
    // `Cancelled`, the in-flight counter drops to zero, drain wakes.
    let drained = svc.shutdown(Duration::from_secs(2)).await;
    assert!(drained, "shutdown must drain after cancel propagates");

    // Best-effort cleanup — dispatch already returned Cancelled.
    release.notify_waiters();
    let _ = dispatch_handle.await;
}

#[tokio::test]
async fn shutdown_refuses_new_dispatches() {
    let release = Arc::new(Notify::new());
    let stub = Arc::new(StubHandle::new(Arc::clone(&release)));
    let svc = AgentService::new(Arc::clone(&stub));

    // Idle drain.
    let drained = svc.shutdown(Duration::from_millis(100)).await;
    assert!(drained);

    // Post-shutdown dispatch refuses.
    let r = svc.dispatch(params_for("c", "ignored")).await;
    match r {
        Err(AgentServiceError::ShuttingDown) => {}
        other => panic!("expected ShuttingDown, got {:?}", other),
    }
}

// -- WEFT-322: agent.chat.reset_budget integration ---------------------------

#[tokio::test]
async fn reset_budget_without_budget_attached_returns_no_budget_error() {
    let release = Arc::new(Notify::new());
    let stub = Arc::new(StubHandle::new(Arc::clone(&release)));
    let svc = AgentService::new(Arc::clone(&stub));

    let r = svc.reset_budget("conv-x");
    assert!(matches!(r, Err(AgentServiceError::NoBudget)));
}

#[tokio::test]
async fn reset_budget_clears_circuit_and_returns_prior_snapshot() {
    use clawft_core::agent::cost_budget::{
        BudgetStore, ConversationBudget, InMemoryBudgetStore,
    };
    use clawft_types::config::CostBudgetConfig;

    let release = Arc::new(Notify::new());
    let stub = Arc::new(StubHandle::new(Arc::clone(&release)));
    let store: Arc<dyn BudgetStore> = Arc::new(InMemoryBudgetStore::new());
    let budget = Arc::new(ConversationBudget::new(
        CostBudgetConfig {
            max_tokens_per_conv: 1_000,
            max_usd_per_conv: 1.0,
            max_iterations_per_conv: 10,
        },
        Arc::clone(&store),
    ));

    // Pre-trip the budget directly through the budget façade — the
    // service-level test doesn't need a full agent loop to exercise
    // the reset RPC contract.
    budget.record_call("conv-rb", 100, 50, 0.0).unwrap();
    budget.mark_open("conv-rb", "tokens").unwrap();
    assert!(budget.usage("conv-rb").circuit_open);

    let svc = AgentService::new(Arc::clone(&stub)).with_cost_budget(Arc::clone(&budget));

    let prev = svc.reset_budget("conv-rb").expect("reset should succeed");
    assert!(prev.circuit_open, "snapshot must reflect tripped state");
    assert_eq!(prev.input_tokens, 100);

    // Post-reset: circuit closed, accumulator zero.
    let post = budget.usage("conv-rb");
    assert!(!post.circuit_open);
    assert_eq!(post.input_tokens, 0);
}
