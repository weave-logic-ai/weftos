//! `AgentService` — per-daemon dispatcher around
//! [`clawft_core::agent::AgentLoop`].
//!
//! See `docs/plans/agent-core-v1.md` Phase C for the full plan; this
//! module is C1 (skeleton only — no daemon wiring, no substrate).
//!
//! # Responsibilities
//!
//! 1. Per-conv serialization. Concurrent `dispatch` calls with the
//!    same `conv_id` queue on a `tokio::sync::Mutex` keyed in
//!    `DashMap<ConvId, _>`. Distinct conv_ids run fully in parallel.
//! 2. Per-conv cancellation. `cancel(conv_id)` flips a
//!    [`CancellationToken`] in the cancel `DashMap`; an in-flight
//!    dispatch observes it via `tokio::select!` against the agent
//!    loop future **and** threads the same token into
//!    `AgentLoop::handle_turn` so `loop_core::run_tool_loop` checks
//!    it at each iteration boundary (WEFT-323 / Phase D2).
//! 3. Drainable shutdown. `shutdown(deadline)` flips a "shutting
//!    down" flag (so new dispatches return [`AgentServiceError::ShuttingDown`])
//!    and waits up to the deadline for the in-flight count to hit
//!    zero. The waiter is woken by a [`tokio::sync::Notify`] each
//!    time a dispatch finishes.
//!
//! # Test seam
//!
//! [`AgentLoopHandle`] decouples the service from
//! `clawft_core::agent::AgentLoop`'s heavy `Platform`/`Pipeline`
//! machinery so unit tests can drive the lock + cancel + shutdown
//! semantics with a stub future. The blanket impl
//! `impl<P: Platform> AgentLoopHandle for AgentLoop<P>` makes
//! production use a one-liner: `Arc::new(AgentService::new(loop))`.

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use clawft_core::agent::cost_budget::{BudgetUsage, ConversationBudget};
use clawft_core::agent::loop_core::AgentLoop;
use clawft_platform::Platform;
use clawft_types::event::{InboundMessage, OutboundMessage};
use dashmap::DashMap;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::interrupt_router::{InterruptAction, InterruptCtx, InterruptOutcome};
use crate::protocol::{AgentChatParams, AgentChatResult};
use clawft_types::agent_chat::FINISH_REASON_ERROR;
use crate::session_tier::SessionTier;
use crate::system_service::AgentChatMetrics;
use clawft_kernel::AgentRegistry;
use clawft_types::agent_chat::{
    AGENT_LOOP_RESULT_META_KEY, AgentLoopResultMeta, GATE_AGENT_ID_META_KEY, caller_principal_name,
};

/// Errors returned by [`AgentService::dispatch`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentServiceError {
    /// The service is in the middle of [`AgentService::shutdown`].
    /// New dispatches are refused; in-flight ones are draining.
    #[error("agent service shutting down")]
    ShuttingDown,
    /// The wrapped [`AgentLoopHandle::handle_turn`] returned an error.
    /// The string is the `Display` of the underlying error so the
    /// trait stays cheap to implement.
    #[error("agent loop error: {0}")]
    Loop(String),
    /// The dispatch was cancelled via [`AgentService::cancel`] before
    /// the loop returned.
    #[error("conversation `{0}` was cancelled")]
    Cancelled(String),
    /// `agent.chat.reset_budget` was invoked but no
    /// [`ConversationBudget`] is attached to the service. WEFT-322.
    #[error("no cost budget attached to agent service")]
    NoBudget,
    /// `agent.chat.reset_budget` failed at the persistence layer.
    /// WEFT-322.
    #[error("budget reset failed: {0}")]
    BudgetReset(String),
}

impl AgentServiceError {
    /// Map this service error into a panel-facing [`AgentChatError`]
    /// (WEFT-334).
    ///
    /// Loop failures are classified from the Display text of the
    /// underlying [`clawft_types::ClawftError`] (the handle trait
    /// boundary is still `String`). Structured service variants map
    /// 1:1 onto chat error kinds.
    pub fn to_chat_error(&self) -> clawft_types::agent_chat::AgentChatError {
        use clawft_types::agent_chat::AgentChatError;
        match self {
            Self::ShuttingDown => AgentChatError::shutting_down(self.to_string()),
            Self::Cancelled(conv_id) => {
                AgentChatError::cancelled(format!("conversation `{conv_id}` was cancelled"))
            }
            Self::Loop(inner) => {
                // Display is "agent loop error: {inner}" — classify on
                // the inner ClawftError text so prefixes like
                // "provider error:" / "operation timed out:" match.
                AgentChatError::classify_loop_error(inner)
            }
            Self::NoBudget => AgentChatError::budget_exceeded(self.to_string()),
            Self::BudgetReset(msg) => {
                AgentChatError::internal(format!("budget reset failed: {msg}"))
            }
        }
    }
}

/// Test seam over `clawft_core::agent::AgentLoop`.
///
/// The production impl is the blanket `impl<P: Platform>` below;
/// tests provide their own implementations (typically a stub that
/// awaits a controllable future) to exercise [`AgentService`] without
/// spinning up the full pipeline.
///
/// The error type is `String` so the trait stays cheap to mock; the
/// service maps it into [`AgentServiceError::Loop`] at the boundary.
#[async_trait]
pub trait AgentLoopHandle: Send + Sync + 'static {
    /// Process one turn end-to-end. See
    /// [`AgentLoop::handle_turn`].
    ///
    /// `cancel` is the per-conversation token (WEFT-323): the loop
    /// observes it at each tool-iteration boundary. Taken by value so
    /// `async_trait` does not need a non-`'static` lifetime (the token
    /// is Arc-backed and cheap to clone).
    async fn handle_turn(
        &self,
        msg: InboundMessage,
        cancel: CancellationToken,
    ) -> Result<OutboundMessage, String>;
}

#[async_trait]
impl<P> AgentLoopHandle for AgentLoop<P>
where
    P: Platform + Send + Sync + 'static,
{
    async fn handle_turn(
        &self,
        msg: InboundMessage,
        cancel: CancellationToken,
    ) -> Result<OutboundMessage, String> {
        AgentLoop::handle_turn(self, msg, &cancel)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Channel name attached to inbound messages built by
/// [`AgentService::dispatch`]. Matches the `agent.chat` JSON-RPC
/// method so downstream session keys (`{channel}:{chat_id}` per
/// [`InboundMessage::session_key`]) line up with the daemon's
/// substrate paths.
const AGENT_CHAT_CHANNEL: &str = "agent.chat";

/// Sender id used when the panel doesn't supply a
/// [`AgentChatParams::caller_id`]. Legacy spike used `"panel"`; WEFT-332
/// plumbs real caller identity through when present. The constant
/// keeps single-tenant / anonymous panels on a stable permission key.
const DEFAULT_SENDER_ID: &str = "panel";

/// Optional kernel registry + pubkey used to lazily register per-caller
/// chat principals (WEFT-332). Absent → synthetic `agent.chat:user:<id>`
/// gate ids (no kernel UUID).
struct CallerPrincipalSource {
    registry: AgentRegistry,
    pubkey: [u8; 32],
    /// Boot-time concierge agent_id used when `caller_id` is absent.
    default_agent_id: Option<String>,
}

/// Wave 2 §W2.3: the register-early/commit-late reply submitter the interrupt
/// executor's Refine arm uses to resubmit a steered turn. Implemented by the
/// §W2.1 loop and injected via [`AgentService::set_reply_submitter`]; a trait so
/// `service.rs` owns the executor seam while the loop owns the submit path.
#[async_trait]
pub trait ReplySubmitter: Send + Sync {
    /// Submit a new assistant reply for `conv_id` (register Frontier now, commit
    /// on generation-finalize). Returns the reply turn's chain sequence — the
    /// amendment turn's seq for the `Contradicts` edge — or `None` on failure.
    async fn submit_reply(&self, conv_id: &str, goal_text: &str) -> Option<u64>;
}

/// Daemon-side dispatcher around an [`AgentLoopHandle`].
///
/// See module docs for the full responsibility list. Generic over
/// the loop handle so unit tests can substitute a stub.
pub struct AgentService<H: AgentLoopHandle> {
    agent_loop: Arc<H>,
    conv_locks: DashMap<String, Arc<Mutex<()>>>,
    cancel_tokens: DashMap<String, CancellationToken>,
    /// Set by [`Self::shutdown`]. Once true, new dispatches return
    /// [`AgentServiceError::ShuttingDown`]. Shared with
    /// [`crate::AgentChatSystemService`] so health probes see drain.
    shutting_down: Arc<AtomicBool>,
    /// Number of dispatches currently inside `handle_turn`. The
    /// shutdown waiter blocks on `drain` until this reads zero.
    /// Also shared into [`AgentChatMetrics`].
    in_flight: Arc<AtomicUsize>,
    /// Notified each time `in_flight` decrements. Lets
    /// [`Self::shutdown`] avoid a polling loop.
    drain: Arc<Notify>,
    /// WEFT-333: last-completion / lock-contention metrics for
    /// `agent.chat` SystemService / `weft status`.
    metrics: Arc<AgentChatMetrics>,
    /// Optional [`ConversationBudget`] handle so `agent.chat.reset_budget`
    /// can clear `circuit_open` for a tripped conv (WEFT-322 item 3).
    /// Held here in addition to the agent loop so the daemon RPC layer
    /// can drive `reset_budget` without touching the loop's internals.
    cost_budget: Option<Arc<ConversationBudget>>,
    /// Optional L2 [`SessionTier`] (ADR-058 Phase 5). The same instance the
    /// agent loop grafts from and the turn anchor indexes into; held here so
    /// the daemon's `agent.chat.end` signal can drive conversation-end
    /// promotion (Phase 5 deferred step 4) without reaching into the loop.
    session_tier: Option<Arc<SessionTier>>,
    /// Wave 2 §W2.3: the §W2.1 reply submitter, injected after wiring. Absent ⇒
    /// the interrupt executor's Refine arm degrades to cancel-only (no resubmit).
    reply_submitter: OnceLock<Arc<dyn ReplySubmitter>>,
    /// WEFT-331: interactive defer broker. Present when the daemon wired
    /// human-in-the-loop defer; drives `agent.chat.defer_decide`.
    defer_broker: Option<Arc<crate::defer_broker::InteractiveDeferBroker>>,
    /// WEFT-332: optional kernel registry for per-caller principals.
    /// When set, `dispatch` lazily `get_or_register`s
    /// `agent.chat:user:<caller_id>` and stamps the UUID into inbound
    /// metadata under [`GATE_AGENT_ID_META_KEY`].
    caller_principals: Option<CallerPrincipalSource>,
    /// Cache of caller_id → kernel agent_id (avoids a registry lookup
    /// every turn once the principal is live).
    caller_agent_ids: DashMap<String, String>,
}

impl<H: AgentLoopHandle> AgentService<H> {
    /// Construct a new service around the given loop handle.
    pub fn new(agent_loop: Arc<H>) -> Self {
        Self::new_inner(agent_loop, None)
    }

    /// Construct with an interactive-defer broker (WEFT-331).
    pub fn with_defer_broker(
        mut self,
        broker: Arc<crate::defer_broker::InteractiveDeferBroker>,
    ) -> Self {
        self.defer_broker = Some(broker);
        self
    }

    fn new_inner(
        agent_loop: Arc<H>,
        defer_broker: Option<Arc<crate::defer_broker::InteractiveDeferBroker>>,
    ) -> Self {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let metrics = Arc::new(AgentChatMetrics::with_in_flight(Arc::clone(&in_flight)));
        Self {
            agent_loop,
            conv_locks: DashMap::new(),
            cancel_tokens: DashMap::new(),
            shutting_down: Arc::new(AtomicBool::new(false)),
            in_flight,
            drain: Arc::new(Notify::new()),
            metrics,
            cost_budget: None,
            session_tier: None,
            reply_submitter: OnceLock::new(),
            defer_broker,
            caller_principals: None,
            caller_agent_ids: DashMap::new(),
        }
    }

    /// Attach the kernel [`AgentRegistry`] so multi-tenant chat can
    /// lazily register per-caller principals (WEFT-332).
    ///
    /// - `registry` — the daemon's kernel registry (same handle the
    ///   boot-time concierge was registered into).
    /// - `pubkey` — Ed25519 public key attached to each lazy principal
    ///   (typically the daemon node's verifying key; PoP is skipped for
    ///   in-process self-registration, matching the concierge path).
    /// - `default_agent_id` — boot-time concierge UUID used when the
    ///   request has no `caller_id` (single-tenant fallback).
    pub fn with_caller_registry(
        mut self,
        registry: AgentRegistry,
        pubkey: [u8; 32],
        default_agent_id: Option<String>,
    ) -> Self {
        self.caller_principals = Some(CallerPrincipalSource {
            registry,
            pubkey,
            default_agent_id,
        });
        self
    }

    /// Resolve (and cache) the gate principal for a caller id.
    ///
    /// Returns `None` when no registry is wired. When wired and
    /// `caller_id` is `Some`, lazily registers
    /// [`caller_principal_name`] and returns the kernel UUID. When
    /// wired and `caller_id` is `None`, returns the boot-time default
    /// (if any).
    pub fn resolve_caller_agent_id(&self, caller_id: Option<&str>) -> Option<String> {
        let source = self.caller_principals.as_ref()?;
        let Some(caller) = caller_id.map(str::trim).filter(|s| !s.is_empty()) else {
            return source.default_agent_id.clone();
        };
        if let Some(cached) = self.caller_agent_ids.get(caller) {
            return Some(cached.clone());
        }
        let name = caller_principal_name(caller);
        let entry = source
            .registry
            .get_or_register(name, source.pubkey);
        self.caller_agent_ids
            .insert(caller.to_string(), entry.agent_id.clone());
        Some(entry.agent_id)
    }

    /// Shared metrics handle (WEFT-333) — clone into
    /// [`crate::AgentChatSystemService`] at registration time.
    pub fn metrics(&self) -> Arc<AgentChatMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Shared shutting-down flag (WEFT-333) — clone into
    /// [`crate::AgentChatSystemService`] so health probes reflect drain.
    pub fn shutting_down_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutting_down)
    }

    /// Deliver a human defer decision (WEFT-331 / `agent.chat.defer_decide`).
    pub fn defer_decide(
        &self,
        conv_id: &str,
        defer_id: &str,
        decision: clawft_types::agent_chat::DeferUserDecision,
    ) -> clawft_types::agent_chat::AgentChatDeferDecideResult {
        match self.defer_broker.as_ref() {
            Some(broker) => broker.decide(conv_id, defer_id, decision),
            None => clawft_types::agent_chat::AgentChatDeferDecideResult {
                accepted: false,
                defer_id: defer_id.into(),
                decision: decision.as_str().into(),
                error: Some("defer broker not wired".into()),
            },
        }
    }

    /// Shared defer broker handle (for attaching to `AgentLoop`), if any.
    pub fn defer_broker(&self) -> Option<Arc<crate::defer_broker::InteractiveDeferBroker>> {
        self.defer_broker.clone()
    }

    /// Inject the §W2.1 [`ReplySubmitter`] (register-early/commit-late reply
    /// path) so the interrupt executor's Refine arm can resubmit a steered turn.
    /// Write-once (idempotent); the daemon wiring calls it after the loop exists.
    pub fn set_reply_submitter(&self, submitter: Arc<dyn ReplySubmitter>) {
        let _ = self.reply_submitter.set(submitter);
    }

    /// The wired §W2.1 [`ReplySubmitter`], if any — the voice loop's idle-turn
    /// path dispatches replies through the same submitter the Refine arm uses.
    pub fn reply_submitter(&self) -> Option<Arc<dyn ReplySubmitter>> {
        self.reply_submitter.get().cloned()
    }

    /// Attach the L2 [`SessionTier`] so [`Self::end_conversation`] can drive
    /// conversation-end promotion (ADR-058 Phase 5 deferred step 4). Pass the
    /// same `Arc<SessionTier>` the agent loop grafts from and the turn anchor
    /// indexes into, so all three share one view set.
    pub fn with_session_tier(mut self, tier: Arc<SessionTier>) -> Self {
        self.session_tier = Some(tier);
        self
    }

    /// Borrow the optional L2 [`SessionTier`] — the daemon uses it to build the
    /// postmortem digest before calling [`Self::end_conversation`].
    pub fn session_tier(&self) -> Option<&Arc<SessionTier>> {
        self.session_tier.as_ref()
    }

    /// Conversation-end promotion (ADR-058 Phase 5 deferred step 4).
    ///
    /// The daemon calls this when a conversation ends (the `agent.chat.end`
    /// signal). When a `durable_fact` is supplied (from the LLM postmortem) the
    /// view is promoted to the trunk and dropped, returning the `memory.promote`
    /// chain sequence; when it is `None` (nothing durable, or the postmortem was
    /// unavailable) the ephemeral view is simply dropped — the chain stays the
    /// source of truth. Returns `None` when no tier is attached or nothing was
    /// promoted.
    pub fn end_conversation(&self, conv_id: &str, durable_fact: Option<&str>) -> Option<u64> {
        let tier = self.session_tier.as_ref()?;
        match durable_fact {
            Some(fact) if !fact.trim().is_empty() => tier.promote_and_drop(conv_id, fact),
            _ => {
                tier.drop_view(conv_id);
                None
            }
        }
    }

    /// Attach a [`ConversationBudget`] so [`Self::reset_budget`] can
    /// drive the `agent.chat.reset_budget` RPC (WEFT-322).
    ///
    /// The same `Arc<ConversationBudget>` should also be passed to the
    /// agent loop via `AgentLoop::with_cost_budget` so both layers
    /// share one accumulator.
    pub fn with_cost_budget(mut self, budget: Arc<ConversationBudget>) -> Self {
        self.cost_budget = Some(budget);
        self
    }

    /// Reset the per-conversation budget circuit (WEFT-322 item 3).
    ///
    /// Drives the daemon RPC `agent.chat.reset_budget`. Clears both
    /// `circuit_open` and the accumulator so the next `agent.chat`
    /// call on `conv_id` proceeds. Returns the pre-reset snapshot for
    /// audit logging.
    ///
    /// Errors:
    /// - [`AgentServiceError::NoBudget`] when no budget is attached.
    /// - [`AgentServiceError::BudgetReset`] on persistence failure.
    pub fn reset_budget(&self, conv_id: &str) -> Result<BudgetUsage, AgentServiceError> {
        let Some(ref budget) = self.cost_budget else {
            return Err(AgentServiceError::NoBudget);
        };
        budget
            .reset(conv_id)
            .map_err(AgentServiceError::BudgetReset)
    }

    /// Borrow the optional [`ConversationBudget`].
    pub fn cost_budget(&self) -> Option<&Arc<ConversationBudget>> {
        self.cost_budget.as_ref()
    }

    /// Single-turn dispatch — the entry point the `agent.chat`
    /// JSON-RPC handler will call.
    ///
    /// 1. Refuse if the service is shutting down.
    /// 2. Acquire (or create) the per-conv `Mutex<()>` and hold it
    ///    for the duration of the dispatch — concurrent calls with
    ///    the same `conv_id` serialize.
    /// 3. Acquire (or create) the per-conv [`CancellationToken`].
    /// 4. Build an [`InboundMessage`] from the wire params and
    ///    drive it through [`AgentLoopHandle::handle_turn`], with a
    ///    `select!` so `cancel()` short-circuits.
    /// 5. Convert the [`OutboundMessage`] back into
    ///    [`AgentChatResult`].
    pub async fn dispatch(
        &self,
        params: AgentChatParams,
    ) -> Result<AgentChatResult, AgentServiceError> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AgentServiceError::ShuttingDown);
        }

        let conv_id = params.conv_id.clone();

        // Per-conv lock. `entry().or_insert_with` is racey across
        // shards in DashMap on first insert; the inner `Arc<Mutex>`
        // makes the contention safe — only one waiter holds the
        // guard at a time regardless of the read path.
        let lock = self
            .conv_locks
            .entry(conv_id.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();

        // Per-conv cancel token. Created fresh on first use so the
        // service holds at most one live token per conv at a time;
        // a `cancel()` between dispatches still works because we
        // look it up with the same `entry().or_insert_with` pattern.
        let cancel = self
            .cancel_tokens
            .entry(conv_id.clone())
            .or_default()
            .clone();

        // Hold the per-conv guard for the whole dispatch so the
        // next caller waits until this turn is fully reflected in
        // the sink (Phase C3) before starting its own.
        // WEFT-333: if try_lock fails, another turn holds the lock —
        // count as per-conv lock contention and time the wait.
        let _guard = match lock.try_lock() {
            Ok(g) => g,
            Err(_) => {
                let started = Instant::now();
                let g = lock.lock().await;
                self.metrics.record_lock_contention(started.elapsed());
                g
            }
        };

        // Re-check shutdown after waiting for the lock — we may
        // have queued behind other dispatches that ran during
        // shutdown initiation.
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AgentServiceError::ShuttingDown);
        }

        // Stale-clone check (Wave 2 §W2.4): the interrupt executor cancels the
        // in-flight turn and immediately resubmits the amendment; the resubmit
        // cloned the conv's token ABOVE, then queued on the lock while the
        // cancelled turn unwound. The dying dispatch re-arms the MAP on its
        // way out, so only our local clone is stale — re-read the map and
        // adopt the current token. When the map still holds the tripped token
        // (a `cancel()` pre-armed on an idle conv, racing the next
        // `agent.chat`), the re-read returns that same token and this dispatch
        // is still cancelled — the C2-spike-parity semantics stay pinned.
        let cancel = if cancel.is_cancelled() {
            self.cancel_tokens
                .entry(conv_id.clone())
                .or_default()
                .clone()
        } else {
            cancel
        };

        // Bump in-flight only once we've actually committed to
        // running the loop. The drop-guard rolls it back.
        let _flight = InFlightGuard::new(Arc::clone(&self.in_flight), Arc::clone(&self.drain));
        // WEFT-333: stamp last-completion on every terminal exit after
        // the loop has been entered (success, cancel, or loop error).
        let _completion = CompletionRecorder {
            metrics: Arc::clone(&self.metrics),
        };

        let gate_agent_id = self.resolve_caller_agent_id(params.caller_id.as_deref());
        let inbound = inbound_from_params(&params, &conv_id, gate_agent_id.as_deref());

        // Drive the loop. WEFT-323 / Phase D2: thread the per-conv
        // token into `handle_turn` so `run_tool_loop` observes it at
        // each iteration boundary. The outer `select!` remains as the
        // mid-await abort path (e.g. cancel during a long LLM call):
        // when the future is dropped the COW bracket's Drop guard
        // rolls back any checkpoint (WEFT-655). When the loop itself
        // returns a cancel error, map it to the same Cancelled
        // service error and re-arm a fresh token.
        let outbound = tokio::select! {
            res = self.agent_loop.handle_turn(inbound, cancel.clone()) => {
                match res {
                    Ok(out) => out,
                    Err(e) if is_cancelled_loop_error(&e) => {
                        self.cancel_tokens
                            .insert(conv_id.clone(), CancellationToken::new());
                        return Err(AgentServiceError::Cancelled(conv_id));
                    }
                    Err(e) => return Err(AgentServiceError::Loop(e)),
                }
            }
            _ = cancel.cancelled() => {
                // Drop the token — a future dispatch on this
                // conv_id should start with a fresh, un-cancelled
                // token. We replace rather than remove so a
                // concurrent `cancel()` racing with the next
                // dispatch sees the new token, not a missing entry.
                self.cancel_tokens
                    .insert(conv_id.clone(), CancellationToken::new());
                return Err(AgentServiceError::Cancelled(conv_id));
            }
        };

        Ok(result_from_outbound(outbound, &params))
    }

    /// Trip the per-conv cancellation token so any in-flight
    /// dispatch on `conv_id` returns
    /// [`AgentServiceError::Cancelled`] at the next yield point.
    ///
    /// No-op when the conv has no in-flight dispatch — the next
    /// dispatch on this id will start with a fresh token.
    pub fn cancel(&self, conv_id: &str) {
        if let Some(token) = self.cancel_tokens.get(conv_id) {
            token.cancel();
        } else {
            // Pre-arm: insert an already-cancelled token so an
            // immediately-following dispatch on this id observes
            // the cancel. This matches the spike's semantics where
            // `agent.chat.cancel` racing the next `agent.chat`
            // still aborts.
            let token = CancellationToken::new();
            token.cancel();
            self.cancel_tokens.insert(conv_id.to_string(), token);
        }
    }

    /// Wave 2 §W2.3: execute the [`InterruptAction`] the daemon-side router
    /// produced for a during-busy utterance, returning what it left on the
    /// forest ([`InterruptOutcome`] — read by the §W2.6 surface + §W2.7 exit
    /// test). The router calls this in-process after `route()`; idle turns
    /// ([`InterruptAction::Turn`]) never reach here (dispatched as a normal
    /// `agent.chat`). Cancel semantics stay as-landed — inline children unwind
    /// with the cancelled turn, detached spawns survive (M4 D5, unchanged).
    pub async fn execute_interrupt(
        &self,
        action: InterruptAction,
        ctx: InterruptCtx,
    ) -> InterruptOutcome {
        let mut out = InterruptOutcome::default();
        // Without a forest-joined session tier there is nothing to prune or
        // witness — a STOP/Refine degrades to the bare token cancel.
        let Some(tier) = self.session_tier.as_ref() else {
            if matches!(action, InterruptAction::Stop | InterruptAction::Refine { .. }) {
                self.cancel(&ctx.conv_id);
            }
            return out;
        };
        match action {
            // Idle — the router dispatches these as a normal turn; no-op here.
            InterruptAction::Turn => {}
            // STOP: cancel the running dispatch, prune the in-flight node to a
            // `Pruned` tombstone (claim-less ⇒ floor open), witness the cancel.
            InterruptAction::Stop => {
                self.cancel(&ctx.conv_id);
                // Prune the node that was in-flight at the router's decision
                // (ctx.in_flight_seq), robust to any turn that registered after.
                out.pruned_seq = tier.emit_cancel_prune(&ctx.conv_id, ctx.in_flight_seq, None);
                out.witnessed = tier.witness_cancel(&ctx.conv_id, out.pruned_seq);
            }
            // REFINE (conservative cancel-and-resubmit, §W2.4): cancel now.
            // FOLLOW-UP (lands atomically so no steer is dropped): assemble
            // original-goal (from ctx.in_flight_seq) + amendment, submit as a new
            // turn, then emit_cancel_prune(conv, Some(amendment_seq)) so the loop
            // prunes the old in-flight AND draws Contradicts(amendment→pruned) —
            // sets pruned_seq + amendment_seq + contradicts + witnessed. Held
            // together (no prune without the resubmit) so a partial Refine never
            // discards the in-flight work without replacing it.
            InterruptAction::Refine { amendment } => {
                self.cancel(&ctx.conv_id);
                // Conservative cancel-and-resubmit (§W2.4), held atomic so no
                // steer is dropped: only when the §W2.1 reply submitter is wired
                // AND there is an in-flight reply do we resubmit → prune old →
                // Contradicts → witness; otherwise degrade to cancel-only.
                if let (Some(submitter), Some(in_flight)) =
                    (self.reply_submitter.get(), ctx.in_flight_seq)
                {
                    // Reconstruct "original goal + amendment" — the goal is
                    // stashed on the in-flight reply node's metadata (§W2.1).
                    let goal = tier.goal_for(&ctx.conv_id, in_flight).unwrap_or_default();
                    let combined = if goal.is_empty() {
                        amendment.clone()
                    } else {
                        format!("{goal}\n\n[amendment] {amendment}")
                    };
                    if let Some(amendment_seq) =
                        submitter.submit_reply(&ctx.conv_id, &combined).await
                    {
                        out.amendment_seq = Some(amendment_seq);
                        // Prune the OLD reply explicitly (robust to the amendment
                        // now being current_turn) and draw Contradicts(new→old).
                        out.pruned_seq = tier.emit_cancel_prune(
                            &ctx.conv_id,
                            Some(in_flight),
                            Some(amendment_seq),
                        );
                        out.contradicts = out.pruned_seq.is_some();
                        out.witnessed = tier.witness_cancel(&ctx.conv_id, out.pruned_seq);
                    }
                }
            }
            // BACKCHANNEL: a "mm-hmm" is NOT a turn — emit the `Backchannel`
            // impulse so the loop's next tick draws a Continuer cross-ref to
            // the in-flight turn (WEFT-650). Non-fatal: a missed emit (no
            // talk loop) never blocks the busy work it acknowledges.
            InterruptAction::Backchannel => {
                tier.emit_backchannel(&ctx.conv_id, ctx.in_flight_seq);
            }
            // QUEUE: hold behind the in-flight turn; the §W2.1 loop's shared
            // VoiceQueues holds the utterance and the reply submitter drains
            // one on each commit/failure (clawft-weave voice_loop.rs). The
            // executor only reports the decision.
            InterruptAction::Queue => {
                out.queued = true;
            }
        }
        out
    }

    /// Begin shutdown.
    ///
    /// Sets the "shutting down" flag (so new dispatches refuse) and
    /// waits up to `deadline` for the in-flight count to drain.
    /// Returns `true` if all dispatches finished within the
    /// deadline, `false` if the timer elapsed first.
    ///
    /// Idempotent — calling twice is fine; the second call just
    /// observes the flag is already set.
    pub async fn shutdown(&self, deadline: Duration) -> bool {
        self.shutting_down.store(true, Ordering::Release);

        // Cancel every known token so dispatches that are blocked
        // inside `handle_turn` unblock at their next yield point.
        for entry in self.cancel_tokens.iter() {
            entry.value().cancel();
        }

        let drain = Arc::clone(&self.drain);
        let in_flight = Arc::clone(&self.in_flight);

        let drained = tokio::time::timeout(deadline, async move {
            loop {
                if in_flight.load(Ordering::Acquire) == 0 {
                    return;
                }
                // `Notified` future arms before we re-check the
                // count, so we can't miss a wake.
                let notified = drain.notified();
                if in_flight.load(Ordering::Acquire) == 0 {
                    return;
                }
                notified.await;
            }
        })
        .await;

        match drained {
            Ok(()) => {
                debug!("agent service shutdown drained cleanly");
                true
            }
            Err(_) => {
                let outstanding = self.in_flight.load(Ordering::Acquire);
                warn!(
                    outstanding,
                    "agent service shutdown deadline elapsed before drain"
                );
                false
            }
        }
    }
}

/// Match the string form of [`clawft_types::ClawftError::Cancelled`]
/// produced by `AgentLoop::handle_turn` when the per-conv token is
/// observed at an iteration boundary (WEFT-323). The trait boundary
/// uses `String` so we match on the Display text rather than the
/// typed error.
fn is_cancelled_loop_error(err: &str) -> bool {
    // ClawftError::Cancelled displays as:
    //   "conversation `<id>` was cancelled"
    err.contains("was cancelled")
}

/// RAII guard that increments the in-flight counter on construction
/// and notifies the drain `Notify` on drop. Dropped at every exit
/// path of `dispatch` (cancel, error, success) so `shutdown` always
/// sees an accurate count.
struct InFlightGuard {
    counter: Arc<AtomicUsize>,
    drain: Arc<Notify>,
}

impl InFlightGuard {
    fn new(counter: Arc<AtomicUsize>, drain: Arc<Notify>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter, drain }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        // SeqCst not needed — the drain waiter re-checks the count
        // after `notified.await` returns and treats the load with
        // Acquire ordering, which pairs with this AcqRel store.
        self.counter.fetch_sub(1, Ordering::AcqRel);
        self.drain.notify_waiters();
    }
}

/// RAII guard that stamps [`AgentChatMetrics::record_completion`] on
/// drop so every terminal exit after the loop is entered updates
/// `last_completion` (WEFT-333).
struct CompletionRecorder {
    metrics: Arc<AgentChatMetrics>,
}

impl Drop for CompletionRecorder {
    fn drop(&mut self) {
        self.metrics.record_completion();
    }
}

/// Normalize an optional wire `caller_id` to a non-empty trimmed id.
fn normalize_caller_id(caller_id: Option<&str>) -> Option<String> {
    caller_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Convert an [`AgentChatParams`] into an [`InboundMessage`] suitable
/// for [`AgentLoop::handle_turn`].
///
/// The last `user`-role message becomes `content`; if there is no
/// user message the trailing message of any role wins (the spike
/// tolerated arbitrary tail roles for assistant-driven kickoffs).
/// Channel is the constant [`AGENT_CHAT_CHANNEL`]; `sender_id` is the
/// wire [`AgentChatParams::caller_id`] when present, else
/// [`DEFAULT_SENDER_ID`] (so [`PermissionResolver`](clawft_core)
/// per-user policy keys correctly — WEFT-332).
///
/// When `gate_agent_id` is provided it is stamped under
/// [`GATE_AGENT_ID_META_KEY`] so the agent loop uses that principal
/// for every `EffectGate::check` (preferring it over the boot-time
/// concierge id). When absent and a `caller_id` is present but no
/// registry was wired, a synthetic namespaced id
/// (`agent.chat:user:<caller>`) is stamped so two callers still
/// never share a gate principal.
///
/// `chat_id` is set to the supplied `conv_id` so the downstream
/// `session_key()` (`"agent.chat:<conv_id>"`) is stable across calls.
fn inbound_from_params(
    params: &AgentChatParams,
    conv_id: &str,
    gate_agent_id: Option<&str>,
) -> InboundMessage {
    let content = last_user_content(&params.messages).unwrap_or_default();
    // Thread wire metadata (skill_instructions / allowed_tools / model /
    // provenance) into the InboundMessage so the daemon loop sees the same
    // per-turn context the in-process REPL injects directly (D5). The wire
    // shape is a JSON object; `InboundMessage.metadata` is a HashMap.
    let mut metadata: std::collections::HashMap<String, serde_json::Value> = params
        .metadata
        .as_ref()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let caller = normalize_caller_id(params.caller_id.as_deref());
    let sender_id = caller
        .clone()
        .unwrap_or_else(|| DEFAULT_SENDER_ID.to_string());

    // Gate principal: explicit (registry UUID) > synthetic namespaced
    // id for the caller > leave unset (loop falls back to daemon /
    // channel:sender).
    let resolved_gate = gate_agent_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| caller.as_ref().map(|c| caller_principal_name(c)));
    if let Some(id) = resolved_gate {
        metadata.insert(
            GATE_AGENT_ID_META_KEY.to_string(),
            serde_json::Value::String(id),
        );
    }

    // WEFT-350: promote message-level audio onto metadata so the loop
    // can populate TurnContent::Audio / Mixed via sink_append_user.
    if let Some(audio) = last_user_audio(&params.messages)
        && let Ok(v) = serde_json::to_value(&audio) {
            metadata
                .entry(clawft_types::turn_content::voice_meta::AUDIO.into())
                .or_insert(v);
        }
    let media = last_user_audio(&params.messages)
        .map(|a| vec![a.substrate_path])
        .unwrap_or_default();
    InboundMessage {
        channel: AGENT_CHAT_CHANNEL.into(),
        sender_id,
        chat_id: conv_id.into(),
        content,
        timestamp: chrono::Utc::now(),
        media,
        metadata,
    }
}

/// Audio ref on the most recent user message (WEFT-350).
fn last_user_audio(
    messages: &[crate::protocol::AgentChatMessage],
) -> Option<clawft_types::AudioRef> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.audio.clone())
        .or_else(|| messages.last().and_then(|m| m.audio.clone()))
}

/// Pick the most recent `role == "user"` content from the wire's
/// `messages` array. Falls back to the last message of any role for
/// resilience against odd panel inputs.
fn last_user_content(messages: &[crate::protocol::AgentChatMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .or_else(|| messages.last().map(|m| m.content.clone()))
}

/// Convert an [`OutboundMessage`] into the wire-shape
/// [`AgentChatResult`].
///
/// Since WEFT-328 / M4 D8 the loop threads an [`AgentLoopResultMeta`]
/// through `OutboundMessage.metadata` under
/// [`AGENT_LOOP_RESULT_META_KEY`], carrying `tool_calls`, token counts,
/// `model`, `identity_source`, `finish_reason`, `iterations`, and
/// `spawned_tasks`. When the key is absent (a non-loop outbound, or an
/// older producer) the fields fall back to the pre-M4 / C1 defaults so
/// the panel keeps tolerating partial payloads.
fn result_from_outbound(outbound: OutboundMessage, _params: &AgentChatParams) -> AgentChatResult {
    // M4 D8 / WEFT-328: read the enriched loop result the daemon agent
    // loop stashed in the envelope metadata. A missing/partial object
    // degrades to `AgentLoopResultMeta::default()` rather than failing
    // the turn.
    let meta = outbound
        .metadata
        .get(AGENT_LOOP_RESULT_META_KEY)
        .and_then(|v| serde_json::from_value::<AgentLoopResultMeta>(v.clone()).ok())
        .unwrap_or_default();
    // A turn the loop flagged as failed (provider error, failed or refused
    // delegation) keeps its readable text for the panel but reports
    // `finish_reason: "error"` so scripted callers can tell it apart.
    let finish_reason = if outbound.is_error() {
        FINISH_REASON_ERROR.into()
    } else if meta.finish_reason.is_empty() {
        "stop".into()
    } else {
        meta.finish_reason
    };
    debug!(
        chat_id = %outbound.chat_id,
        iterations = meta.iterations,
        tool_calls = meta.tool_calls.len(),
        spawned_tasks = meta.spawned_tasks.len(),
        model = ?meta.model,
        identity_source = ?meta.identity_source,
        prompt_tokens = meta.prompt_tokens,
        completion_tokens = meta.completion_tokens,
        "agent.chat result populated from OutboundMessage loop meta"
    );
    AgentChatResult {
        assistant_text: outbound.content,
        tool_calls: meta.tool_calls,
        finish_reason,
        iterations: meta.iterations,
        prompt_tokens: meta.prompt_tokens,
        completion_tokens: meta.completion_tokens,
        model: meta.model,
        identity_source: meta.identity_source,
        reasoning: meta.reasoning,
        spawned_tasks: meta.spawned_tasks,
        // WEFT-345: consecutive gate-denial → EscalateToHuman event.
        escalation: meta.escalation,
        // WEFT-258: gate Defer → interactive panel prompt-and-resume.
        deferred: meta.deferred,
    }
}

#[cfg(test)]
mod tests {
    //! Inline unit tests for adapter functions that touch private
    //! helpers (`inbound_from_params`, `result_from_outbound`).
    //!
    //! The integration-style tests covering lock / cancel / shutdown
    //! semantics live in `tests/dispatch.rs` so they exercise only
    //! the public surface (and so this file stays under the 500-line
    //! ceiling per CLAUDE.md).

    use super::*;
    use clawft_types::event::OutboundMessage;
    use std::collections::HashMap;

    fn params_for(conv_id: &str, content: &str) -> AgentChatParams {
        AgentChatParams {
            messages: vec![crate::protocol::AgentChatMessage::text("user", content)],
            temperature: None,
            max_tokens: None,
            conv_id: conv_id.into(),
            metadata: None,
            caller_id: None,
        }
    }

    #[test]
    fn inbound_from_params_threads_wire_metadata() {
        // skill_instructions (and any other wire metadata) must survive
        // the params → InboundMessage hop so the daemon loop reads what
        // the in-process REPL injects directly (D5).
        let mut meta = serde_json::Map::new();
        meta.insert(
            "skill_instructions".into(),
            serde_json::Value::String("be terse".into()),
        );
        let p = AgentChatParams {
            messages: vec![crate::protocol::AgentChatMessage::text("user", "hi")],
            temperature: None,
            max_tokens: None,
            conv_id: "c".into(),
            metadata: Some(meta),
            caller_id: None,
        };
        let inbound = inbound_from_params(&p, "c", None);
        assert_eq!(
            inbound.metadata.get("skill_instructions"),
            Some(&serde_json::Value::String("be terse".into())),
        );
        assert_eq!(inbound.sender_id, DEFAULT_SENDER_ID);
    }

    #[test]
    fn inbound_from_params_absent_metadata_is_empty() {
        let inbound = inbound_from_params(&params_for("c", "hi"), "c", None);
        assert!(inbound.metadata.is_empty());
    }

    #[test]
    fn inbound_from_params_picks_last_user_content() {
        let p = AgentChatParams {
            messages: vec![
                crate::protocol::AgentChatMessage::text("system", "ignore"),
                crate::protocol::AgentChatMessage::text("user", "first user"),
                crate::protocol::AgentChatMessage::text("assistant", "ignore me too"),
                crate::protocol::AgentChatMessage::text("user", "actual ask"),
            ],
            temperature: None,
            max_tokens: None,
            conv_id: "c".into(),
            metadata: None,
            caller_id: None,
        };
        let inbound = inbound_from_params(&p, "c", None);
        assert_eq!(inbound.channel, AGENT_CHAT_CHANNEL);
        assert_eq!(inbound.chat_id, "c");
        assert_eq!(inbound.content, "actual ask");
    }

    #[test]
    fn inbound_from_params_promotes_message_audio() {
        // WEFT-350: message-level audio becomes metadata + media so the
        // loop can persist TurnContent::Mixed.
        let audio = clawft_types::AudioRef::new(
            "substrate/_derived/chat/c/audio/1",
            "audio/wav",
            800,
        );
        let p = AgentChatParams {
            messages: vec![crate::protocol::AgentChatMessage {
                role: "user".into(),
                content: "said aloud".into(),
                audio: Some(audio.clone()),
            }],
            temperature: None,
            max_tokens: None,
            conv_id: "c".into(),
            metadata: None,
            caller_id: None,
        };
        let inbound = inbound_from_params(&p, "c", None);
        assert_eq!(inbound.content, "said aloud");
        assert_eq!(inbound.media, vec![audio.substrate_path.clone()]);
        let got = clawft_types::audio_from_metadata(
            &inbound
                .metadata
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
        .expect("audio metadata");
        assert_eq!(got.duration_ms, 800);
    }

    #[test]
    fn inbound_from_params_falls_back_to_last_message() {
        // No `user` role at all — fallback to the last entry.
        let p = AgentChatParams {
            messages: vec![crate::protocol::AgentChatMessage::text("assistant", "lone")],
            temperature: None,
            max_tokens: None,
            conv_id: "c".into(),
            metadata: None,
            caller_id: None,
        };
        let inbound = inbound_from_params(&p, "c", None);
        assert_eq!(inbound.content, "lone");
    }

    /// WEFT-332: caller_id becomes sender_id and a namespaced gate
    /// principal so two users never share the default "panel" id.
    #[test]
    fn inbound_from_params_scopes_caller_identity() {
        let mut p = params_for("shared-conv", "hi");
        p.caller_id = Some("alice".into());
        let inbound = inbound_from_params(&p, "shared-conv", None);
        assert_eq!(inbound.sender_id, "alice");
        assert_eq!(
            inbound
                .metadata
                .get(GATE_AGENT_ID_META_KEY)
                .and_then(|v| v.as_str()),
            Some("agent.chat:user:alice")
        );

        p.caller_id = Some("bob".into());
        let inbound_b = inbound_from_params(&p, "shared-conv", None);
        assert_eq!(inbound_b.sender_id, "bob");
        assert_ne!(
            inbound.metadata.get(GATE_AGENT_ID_META_KEY),
            inbound_b.metadata.get(GATE_AGENT_ID_META_KEY),
            "two callers must not share a gate principal"
        );
    }

    /// WEFT-332: explicit registry UUID wins over the synthetic name.
    #[test]
    fn inbound_from_params_prefers_registry_agent_id() {
        let mut p = params_for("c", "hi");
        p.caller_id = Some("alice".into());
        let inbound = inbound_from_params(&p, "c", Some("uuid-from-registry"));
        assert_eq!(
            inbound
                .metadata
                .get(GATE_AGENT_ID_META_KEY)
                .and_then(|v| v.as_str()),
            Some("uuid-from-registry")
        );
        assert_eq!(inbound.sender_id, "alice");
    }

    #[test]
    fn result_from_outbound_marks_known_shortfalls() {
        // No loop metadata on the envelope (e.g. a non-loop outbound):
        // enriched fields fall back to their pre-M4 / C1 defaults so the
        // panel keeps tolerating partial payloads across the cutover.
        let out = OutboundMessage {
            channel: "agent.chat".into(),
            chat_id: "c".into(),
            content: "hi".into(),
            reply_to: None,
            media: Vec::new(),
            metadata: HashMap::new(),
        };
        let r = result_from_outbound(out, &params_for("c", ""));
        assert_eq!(r.assistant_text, "hi");
        assert!(r.tool_calls.is_empty());
        assert_eq!(r.finish_reason, "stop");
        assert_eq!(r.iterations, 0);
        assert!(r.spawned_tasks.is_empty());
        // C1 defaults when meta is absent.
        assert_eq!(r.prompt_tokens, 0);
        assert_eq!(r.completion_tokens, 0);
        assert!(r.model.is_none());
        assert!(r.identity_source.is_none());
    }

    #[test]
    fn result_from_outbound_reads_enriched_loop_meta() {
        // M4 D8 / WEFT-328: the daemon loop stashes AgentLoopResultMeta
        // under the well-known key; result_from_outbound must surface
        // all five plumbed fields (tool_calls, tokens, model, identity).
        use clawft_types::agent_chat::{AgentChatToolCall, SpawnedTaskSummary};
        let meta = AgentLoopResultMeta {
            tool_calls: vec![AgentChatToolCall {
                name: "agent_spawn".into(),
                arguments_preview: "{\"goal\":\"answer 2+2\"}".into(),
                result_preview: "{\"status\":\"completed\"}".into(),
                success: true,
            }],
            finish_reason: "stop".into(),
            iterations: 2,
            spawned_tasks: vec![SpawnedTaskSummary {
                task_id: "task-1".into(),
                child_conv_id: "sub:c:01HQ".into(),
                status: "completed".into(),
            }],
            model: Some("hermes-4.3-36b".into()),
            prompt_tokens: 812,
            completion_tokens: 96,
            reasoning: None,
            identity_source: Some("clawft".into()),
            escalation: None,
            deferred: None,
        };
        let mut metadata = HashMap::new();
        metadata.insert(
            AGENT_LOOP_RESULT_META_KEY.to_string(),
            serde_json::to_value(&meta).unwrap(),
        );
        let out = OutboundMessage {
            channel: "agent.chat".into(),
            chat_id: "c".into(),
            content: "kicked off a subagent".into(),
            reply_to: None,
            media: Vec::new(),
            metadata,
        };
        let r = result_from_outbound(out, &params_for("c", ""));
        assert_eq!(r.iterations, 2);
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "agent_spawn");
        assert_eq!(r.spawned_tasks.len(), 1);
        assert_eq!(r.spawned_tasks[0].task_id, "task-1");
        assert_eq!(r.spawned_tasks[0].status, "completed");
        // WEFT-328: real token / model / identity values surface on the wire.
        assert_eq!(r.prompt_tokens, 812);
        assert_eq!(r.completion_tokens, 96);
        assert_eq!(r.model.as_deref(), Some("hermes-4.3-36b"));
        assert_eq!(r.identity_source.as_deref(), Some("clawft"));
        assert!(r.escalation.is_none());
        assert!(r.deferred.is_none());
    }

    #[test]
    fn result_from_outbound_reads_weft345_escalation() {
        // WEFT-345: EscalateToHuman event on the loop meta must surface
        // on AgentChatResult for panels / future allow-abort-refine RPC.
        use clawft_types::agent_chat::{
            EscalateToHumanEvent, FINISH_REASON_ESCALATE_TO_HUMAN, GateDenialRecord,
            GOVERNANCE_DECISION_ESCALATE_TO_HUMAN,
        };
        let event = EscalateToHumanEvent::from_denials(
            "c",
            vec![
                GateDenialRecord {
                    tool: "echo".into(),
                    reason: "blocked".into(),
                },
                GateDenialRecord {
                    tool: "echo".into(),
                    reason: "blocked".into(),
                },
                GateDenialRecord {
                    tool: "echo".into(),
                    reason: "blocked".into(),
                },
            ],
        );
        let meta = AgentLoopResultMeta {
            tool_calls: vec![],
            finish_reason: FINISH_REASON_ESCALATE_TO_HUMAN.into(),
            iterations: 3,
            spawned_tasks: vec![],
            model: None,
            prompt_tokens: 30,
            completion_tokens: 9,
            reasoning: None,
            identity_source: None,
            escalation: Some(event.clone()),
            deferred: None,
        };
        let mut metadata = HashMap::new();
        metadata.insert(
            AGENT_LOOP_RESULT_META_KEY.to_string(),
            serde_json::to_value(&meta).unwrap(),
        );
        let out = OutboundMessage {
            channel: "agent.chat".into(),
            chat_id: "c".into(),
            content: event.summary.clone(),
            reply_to: None,
            media: Vec::new(),
            metadata,
        };
        let r = result_from_outbound(out, &params_for("c", ""));
        assert_eq!(r.finish_reason, FINISH_REASON_ESCALATE_TO_HUMAN);
        let esc = r.escalation.expect("escalation present");
        assert_eq!(esc.decision, GOVERNANCE_DECISION_ESCALATE_TO_HUMAN);
        assert_eq!(esc.denial_count, 3);
        assert_eq!(esc.denials.len(), 3);
        assert!(r.assistant_text.contains("EscalateToHuman"));
        assert!(r.deferred.is_none());
    }

    #[test]
    fn result_from_outbound_reads_weft258_deferred() {
        // WEFT-258: DeferredActionEvent on the loop meta must surface
        // on AgentChatResult so the chat panel can prompt-and-resume.
        use clawft_types::agent_chat::{DeferredActionEvent, FINISH_REASON_DEFERRED};
        let event =
            DeferredActionEvent::new("c", "write_file", "policy review pending", "{\"path\":\"x\"}");
        let meta = AgentLoopResultMeta {
            tool_calls: vec![],
            finish_reason: FINISH_REASON_DEFERRED.into(),
            iterations: 1,
            spawned_tasks: vec![],
            model: None,
            prompt_tokens: 12,
            completion_tokens: 4,
            reasoning: None,
            identity_source: None,
            escalation: None,
            deferred: Some(event.clone()),
        };
        let mut metadata = HashMap::new();
        metadata.insert(
            AGENT_LOOP_RESULT_META_KEY.to_string(),
            serde_json::to_value(&meta).unwrap(),
        );
        let out = OutboundMessage {
            channel: "agent.chat".into(),
            chat_id: "c".into(),
            content: event.summary.clone(),
            reply_to: None,
            media: Vec::new(),
            metadata,
        };
        let r = result_from_outbound(out, &params_for("c", ""));
        assert_eq!(r.finish_reason, FINISH_REASON_DEFERRED);
        let d = r.deferred.expect("deferred present");
        assert!(d.deferred);
        assert_eq!(d.tool, "write_file");
        assert_eq!(d.reason, "policy review pending");
        assert!(r.assistant_text.contains("deferred"));
    }

    #[test]
    fn result_from_outbound_ignores_garbage_meta() {
        // A malformed value under the key must not fail the turn; the
        // fields degrade to defaults.
        let mut metadata = HashMap::new();
        metadata.insert(
            AGENT_LOOP_RESULT_META_KEY.to_string(),
            serde_json::json!("not-an-object"),
        );
        let out = OutboundMessage {
            channel: "agent.chat".into(),
            chat_id: "c".into(),
            content: "hi".into(),
            reply_to: None,
            media: Vec::new(),
            metadata,
        };
        let r = result_from_outbound(out, &params_for("c", ""));
        assert_eq!(r.finish_reason, "stop");
        assert_eq!(r.iterations, 0);
        assert!(r.tool_calls.is_empty());
        assert!(r.spawned_tasks.is_empty());
    }

    #[test]
    fn result_from_outbound_reports_flagged_failure_as_error() {
        let mut out = OutboundMessage {
            channel: "agent.chat".into(),
            chat_id: "c".into(),
            content: "Delegation failed: credit balance is too low.".into(),
            reply_to: None,
            media: Vec::new(),
            metadata: HashMap::new(),
        };
        out.mark_error();
        let r = result_from_outbound(out, &params_for("c", ""));
        assert_eq!(r.finish_reason, FINISH_REASON_ERROR);
        assert!(r.is_error());
        assert!(r.assistant_text.starts_with("Delegation failed"));
    }

    // ── WEFT-334: AgentServiceError → AgentChatError ──────────────

    #[test]
    fn service_error_to_chat_error_maps_each_variant() {
        use clawft_types::agent_chat::AgentChatError;

        assert_eq!(
            AgentServiceError::ShuttingDown.to_chat_error().error_kind(),
            "shutting_down"
        );
        assert_eq!(
            AgentServiceError::Cancelled("c1".into())
                .to_chat_error()
                .error_kind(),
            "cancelled"
        );
        assert_eq!(
            AgentServiceError::NoBudget.to_chat_error().error_kind(),
            "budget_exceeded"
        );
        assert_eq!(
            AgentServiceError::BudgetReset("io".into())
                .to_chat_error()
                .error_kind(),
            "internal"
        );

        // Loop classification — the three panel-branch kinds.
        let timeout = AgentServiceError::Loop("operation timed out: llm_call".into()).to_chat_error();
        assert_eq!(timeout.error_kind(), "timeout");
        assert!(matches!(timeout, AgentChatError::Timeout { .. }));

        let gate = AgentServiceError::Loop("security violation: path traversal".into())
            .to_chat_error();
        assert_eq!(gate.error_kind(), "gate_deny");
        assert!(matches!(gate, AgentChatError::GateDeny { .. }));

        let llm = AgentServiceError::Loop("provider error: 502 bad gateway".into()).to_chat_error();
        assert_eq!(llm.error_kind(), "llm_error");
        assert!(matches!(llm, AgentChatError::LlmError { .. }));
    }
}
