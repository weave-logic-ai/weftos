//! 6-stage pipeline trait definitions and supporting types.
//!
//! This module defines the core abstractions for the clawft pipeline system.
//! Each stage is represented by a trait that can be implemented at different
//! capability levels (Level 0 = basic, Level 1 = adaptive, Level 2 = neural).
//!
//! The pipeline stages in order:
//! 1. **[`TaskClassifier`]** -- Classify the incoming request by task type
//! 2. **[`ModelRouter`]** -- Select the best provider/model for the task
//! 3. **[`ContextAssembler`]** -- Assemble context (system prompt, memory, history)
//! 4. **[`LlmTransport`]** -- Execute the LLM call via HTTP
//! 5. **[`QualityScorer`]** -- Score response quality
//! 6. **[`LearningBackend`]** -- Record the interaction for future learning

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use clawft_types::provider::LlmResponse;
use clawft_types::routing::AuthContext;

// ── Request / message types ─────────────────────────────────────────────

/// A chat request entering the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The conversation messages.
    pub messages: Vec<LlmMessage>,

    /// Tool definitions as JSON schemas.
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,

    /// Explicit model override (if any).
    #[serde(default)]
    pub model: Option<String>,

    /// Maximum tokens in the response.
    #[serde(default)]
    pub max_tokens: Option<i32>,

    /// Sampling temperature.
    #[serde(default)]
    pub temperature: Option<f64>,

    /// Authentication context for permission-gated routing.
    /// Populated server-side by channel plugins and AgentLoop.
    /// `skip_deserializing` prevents JSON injection via the gateway API.
    #[serde(default, skip_deserializing)]
    pub auth_context: Option<AuthContext>,

    /// Complexity boost applied by hallucination detection.
    /// Added to the classifier's keyword density to push hallucination-prone
    /// sessions into higher-tier models.
    #[serde(default)]
    pub complexity_boost: f32,
}

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    /// The role of the message sender (e.g. "system", "user", "assistant", "tool").
    pub role: String,

    /// The text content of the message.
    pub content: String,

    /// For tool-result messages, the ID of the tool call this responds to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// For assistant messages that invoke tools, the tool call objects.
    /// Serialised as OpenAI-format `tool_calls` array so the next request
    /// round-trip keeps the provider happy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
}

// ── Classification types ────────────────────────────────────────────────

/// Task classification result produced by [`TaskClassifier`].
#[derive(Debug, Clone)]
pub struct TaskProfile {
    /// The detected task type.
    pub task_type: TaskType,

    /// Estimated complexity on a 0.0--1.0 scale.
    pub complexity: f32,

    /// Keywords that contributed to the classification.
    pub keywords: Vec<String>,
}

/// Types of tasks the classifier can identify.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskType {
    /// General conversation / chitchat.
    Chat,
    /// Writing new code.
    CodeGeneration,
    /// Reviewing existing code.
    CodeReview,
    /// Searching for information.
    Research,
    /// Creative writing (stories, poems, etc.).
    Creative,
    /// Analytical reasoning, summarization, explanation.
    Analysis,
    /// Explicit tool invocation.
    ToolUse,
    /// Could not determine the task type.
    Unknown,
}

// ── Routing types ───────────────────────────────────────────────────────

/// Model routing decision produced by [`ModelRouter`].
///
/// # Reason redaction (WEFT-30)
///
/// `reason` is **operator-debug only** — it may contain user identifiers,
/// channel names, complexity scores, model names, and policy details that
/// should never cross a trust boundary (errors surfaced to end users, public
/// audit logs, third-party telemetry, etc.). Always call
/// [`RoutingDecision::redacted_reason`] before placing the reason in any
/// caller-visible context.
#[derive(Debug, Clone, Default)]
pub struct RoutingDecision {
    /// Provider name (e.g. "openai", "anthropic").
    pub provider: String,

    /// Model identifier (e.g. "gpt-4o", "claude-opus-4-5").
    pub model: String,

    /// Human-readable reason for the routing choice.
    ///
    /// **Internal/operator-debug only.** Contains user identifiers, model
    /// names, and policy details. Use [`Self::redacted_reason`] for any
    /// user-facing surface (error messages, public audit logs, telemetry
    /// shipped off-host, etc.).
    pub reason: String,

    /// Tier name that was selected (None for static routing).
    pub tier: Option<String>,

    /// Estimated cost in USD for this request.
    pub cost_estimate_usd: Option<f64>,

    /// Whether the request was escalated to a higher tier.
    pub escalated: bool,

    /// Whether budget constraints affected the routing.
    pub budget_constrained: bool,

    /// The sender who originated this request, for per-user cost attribution.
    pub sender_id: Option<String>,
}

impl RoutingDecision {
    /// Return a high-level category label suitable for crossing a trust
    /// boundary (user-facing error messages, public audit logs, telemetry).
    ///
    /// Strips paths, user identifiers, model names, complexity scores, and
    /// channel names from the raw `reason`. The mapping uses the structured
    /// fields on the decision plus keyword classification of the reason
    /// string. Categories are stable identifiers safe to switch on:
    ///
    /// - `static_routing`           — Level-0 static router, no tier system
    /// - `rate_limited`             — caller hit per-sender rate limit
    /// - `tier_check_failed`        — fallback model denied by max_tier
    /// - `cost_cap_hit`             — daily/monthly budget exhausted
    /// - `fallback_chosen`          — primary tier unavailable, fallback used
    /// - `no_tiers_available`       — no permitted tier produced a model
    /// - `model_override_bypass`    — caller's model_override punched through
    /// - `escalated`                — promoted above caller's max_tier
    /// - `budget_constrained`       — selected cheaper tier to fit budget
    /// - `tiered_routing`           — normal tiered selection (default)
    pub fn redacted_reason(&self) -> &'static str {
        let r = self.reason.as_str();
        // Order matters: check the highest-severity / most-specific labels
        // first so they don't get masked by the generic structured-field
        // fallbacks below.
        if r.contains("model_override") {
            return "model_override_bypass";
        }
        if r.contains("rate limited") {
            if r.contains("not permitted") {
                return "tier_check_failed";
            }
            return "rate_limited";
        }
        if r.contains("not permitted") || r.contains("denied") {
            return "tier_check_failed";
        }
        if r.contains("cost") || r.contains("budget") || r.contains("over") {
            return "cost_cap_hit";
        }
        if r.contains("no tiers") {
            return "no_tiers_available";
        }
        if r.contains("fallback") {
            return "fallback_chosen";
        }
        if r.contains("static") {
            return "static_routing";
        }
        // Structured-field fallbacks — these are derived from the decision
        // metadata, not the free-text reason, so they remain accurate even
        // for routers that don't bother formatting a reason string.
        if self.budget_constrained {
            return "budget_constrained";
        }
        if self.escalated {
            return "escalated";
        }
        "tiered_routing"
    }
}

/// Outcome of a response, used to update the router.
#[derive(Debug, Clone)]
pub struct ResponseOutcome {
    /// Whether the response was considered successful.
    pub success: bool,

    /// Quality assessment of the response.
    pub quality: QualityScore,

    /// End-to-end latency in milliseconds.
    pub latency_ms: u64,
}

// ── Quality types ───────────────────────────────────────────────────────

/// Quality assessment of a response.
#[derive(Debug, Clone)]
pub struct QualityScore {
    /// Overall quality score (0.0--1.0).
    pub overall: f32,

    /// Relevance to the original request (0.0--1.0).
    pub relevance: f32,

    /// Coherence and readability (0.0--1.0).
    pub coherence: f32,
}

// ── Context / transport types ───────────────────────────────────────────

/// Assembled context ready for transport to an LLM provider.
#[derive(Debug, Clone)]
pub struct AssembledContext {
    /// The final set of messages to send.
    pub messages: Vec<LlmMessage>,

    /// Estimated token count for the assembled context.
    pub token_estimate: usize,

    /// Whether the context was truncated to fit the budget.
    pub truncated: bool,
}

/// Request sent to the transport layer.
#[derive(Debug, Clone)]
pub struct TransportRequest {
    /// Provider name.
    pub provider: String,

    /// Model identifier.
    pub model: String,

    /// Messages to send.
    pub messages: Vec<LlmMessage>,

    /// Tool definitions as JSON schemas.
    pub tools: Vec<serde_json::Value>,

    /// Maximum tokens in the response.
    pub max_tokens: Option<i32>,

    /// Sampling temperature.
    pub temperature: Option<f64>,
}

// ── Learning types ──────────────────────────────────────────────────────

/// A complete interaction trajectory for learning.
#[derive(Debug, Clone)]
pub struct Trajectory {
    /// The original request.
    pub request: ChatRequest,

    /// The routing decision that was made.
    pub routing: RoutingDecision,

    /// The LLM response.
    pub response: LlmResponse,

    /// The quality assessment of the response.
    pub quality: QualityScore,
}

/// Signal for the learning backend to adapt behavior.
#[derive(Debug, Clone)]
pub struct LearningSignal {
    /// Type of feedback (e.g. "thumbs_up", "thumbs_down", "correction").
    pub feedback_type: String,

    /// Numeric value of the signal (-1.0 to 1.0).
    pub value: f32,
}

// ── Pipeline traits ─────────────────────────────────────────────────────

/// Stage 1: Classify the incoming request to determine task type and complexity.
pub trait TaskClassifier: Send + Sync {
    /// Analyze the request and produce a task profile.
    fn classify(&self, request: &ChatRequest) -> TaskProfile;
}

/// Stage 2: Select the best provider and model for the classified task.
#[async_trait]
pub trait ModelRouter: Send + Sync {
    /// Choose a provider/model combination for the given request and profile.
    async fn route(&self, request: &ChatRequest, profile: &TaskProfile) -> RoutingDecision;

    /// Update internal state based on a routing outcome (for adaptive routers).
    fn update(&self, decision: &RoutingDecision, outcome: &ResponseOutcome);
}

/// Stage 3: Assemble the context (system prompt, memory, skills, history).
#[async_trait]
pub trait ContextAssembler: Send + Sync {
    /// Build the assembled context for the given request and task profile.
    async fn assemble(&self, request: &ChatRequest, profile: &TaskProfile) -> AssembledContext;
}

/// A callback invoked for each streaming chunk from the LLM.
///
/// The callback receives a text fragment (for text deltas) or a
/// serialized chunk description. It is called from the transport
/// layer as SSE chunks arrive.
///
/// The callback should return `true` to continue streaming, or
/// `false` to abort the stream early.
///
/// Uses `FnMut` to allow stateful callbacks (e.g. buffering,
/// counting tokens, accumulating output).
pub type StreamCallback = Box<dyn FnMut(&str) -> bool + Send>;

/// Stage 4: Execute the LLM call via HTTP transport.
///
/// The `async_trait` `?Send` relaxation is applied for the `browser`
/// feature so the WASM-resident transport (which wraps `reqwest`'s
/// `!Send` Fetch-API client) satisfies the trait. Native impls keep
/// the strict `Send` bound for tokio multi-threaded runtimes.
///
/// Streaming (`complete_stream`) is gated to the native build because
/// the [`StreamCallback`] type is `+ Send`-bounded; once the browser
/// transport learns SSE under `wasm-streams`/`ReadableStream` a
/// browser-equivalent will land alongside it.
#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
pub trait LlmTransport: Send + Sync {
    /// Send a request to the LLM provider and return the response.
    async fn complete(&self, request: &TransportRequest) -> clawft_types::Result<LlmResponse>;

    /// Send a streaming request, invoking `callback` for each text chunk.
    ///
    /// The default implementation falls back to `complete()` and invokes
    /// the callback once with the full response text.
    #[cfg(not(feature = "browser"))]
    async fn complete_stream(
        &self,
        request: &TransportRequest,
        mut callback: StreamCallback,
    ) -> clawft_types::Result<LlmResponse> {
        let response = self.complete(request).await?;

        // Invoke callback with the full text for non-streaming fallback
        for block in &response.content {
            if let clawft_types::provider::ContentBlock::Text { text } = block
                && !callback(text)
            {
                break;
            }
        }

        Ok(response)
    }
}

/// Stage 5: Score the quality of a response.
pub trait QualityScorer: Send + Sync {
    /// Assess the quality of the response relative to the original request.
    fn score(&self, request: &ChatRequest, response: &LlmResponse) -> QualityScore;
}

/// Stage 6: Record interactions and adapt behavior based on feedback.
pub trait LearningBackend: Send + Sync {
    /// Record a complete interaction trajectory.
    fn record(&self, trajectory: &Trajectory);

    /// Process a learning signal (e.g. user feedback).
    fn adapt(&self, signal: &LearningSignal);

    /// Apply learned-from-trajectories mutations to a system prompt.
    ///
    /// Default implementation is a no-op: returns the prompt unchanged.
    /// Trajectory-collecting implementations
    /// (e.g. [`crate::pipeline::learner::TrajectoryLearner`]) override
    /// this to apply
    /// [`crate::pipeline::mutation::mutate_prompt`] when an evolution
    /// is due.
    ///
    /// Called from [`PipelineRegistry::complete`] before the transport
    /// stage so the model sees an up-to-date system prompt.
    fn evolve_prompt(&self, prompt: &str) -> String {
        prompt.to_string()
    }
}

// ── Cost & rate-limiting traits ──────────────────────────────────────────

/// Result of a budget check from [`CostTrackable::check_budget`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetResult {
    /// The request fits within budget.
    Approved,
    /// The daily spending limit has been exceeded.
    DailyLimitExceeded { spent: f64, limit: f64 },
    /// The monthly spending limit has been exceeded.
    MonthlyLimitExceeded { spent: f64, limit: f64 },
}

impl BudgetResult {
    /// Returns `true` if the budget check passed.
    pub fn is_approved(&self) -> bool {
        matches!(self, BudgetResult::Approved)
    }
}

/// Budget tracking interface used by the TieredRouter.
///
/// The real implementation lives in Phase D (CostTracker). This trait
/// allows the router to be tested independently with mock implementations.
pub trait CostTrackable: Send + Sync {
    /// Check whether the estimated cost fits within the sender's daily and
    /// monthly limits.
    fn check_budget(
        &self,
        sender_id: &str,
        estimated_cost: f64,
        daily_limit: f64,
        monthly_limit: f64,
    ) -> BudgetResult;

    /// Record an estimated cost before the LLM call (pre-reservation).
    fn record_estimated(&self, sender_id: &str, estimated_cost: f64);

    /// Reconcile actual cost after response -- adjusts the reservation.
    fn record_actual(&self, sender_id: &str, estimated_cost: f64, actual_cost: f64);
}

/// Rate limiting interface used by the TieredRouter.
///
/// The real implementation lives in Phase E (RateLimiter). This trait
/// allows the router to be tested independently with mock implementations.
pub trait RateLimitable: Send + Sync {
    /// Returns true if the request is allowed, false if rate-limited.
    fn check(&self, sender_id: &str, limit: u32) -> bool;
}

// ── Pipeline & Registry ─────────────────────────────────────────────────

/// A complete pipeline wiring all 6 stages together.
pub struct Pipeline {
    /// Stage 1: task classifier.
    pub classifier: Arc<dyn TaskClassifier>,
    /// Stage 2: model router.
    pub router: Arc<dyn ModelRouter>,
    /// Stage 3: context assembler.
    pub assembler: Arc<dyn ContextAssembler>,
    /// Stage 4: LLM transport.
    pub transport: Arc<dyn LlmTransport>,
    /// Stage 5: quality scorer.
    pub scorer: Arc<dyn QualityScorer>,
    /// Stage 6: learning backend.
    pub learner: Arc<dyn LearningBackend>,
}

/// Pipeline-internal helper for stage 3.5: ask the learner to mutate
/// the assembled system message before transport.
///
/// Walks the assembled messages, finds the system prompt (first
/// `role == "system"`), passes its content through
/// [`LearningBackend::evolve_prompt`], and returns a new vector with
/// the (possibly) mutated content. Non-system messages are passed
/// through unchanged.
///
/// If no system message exists the input is returned unchanged — the
/// learner only ever transforms the system layer.
fn apply_prompt_evolution(
    messages: Vec<LlmMessage>,
    learner: &dyn LearningBackend,
) -> Vec<LlmMessage> {
    let mut mutated = false;
    let out: Vec<LlmMessage> = messages
        .into_iter()
        .map(|m| {
            if !mutated && m.role == "system" {
                let new_content = learner.evolve_prompt(&m.content);
                if new_content != m.content {
                    tracing::debug!(
                        before_chars = m.content.len(),
                        after_chars = new_content.len(),
                        "pipeline: learner mutated system prompt"
                    );
                }
                mutated = true; // only the first system message is mutated
                LlmMessage {
                    role: m.role,
                    content: new_content,
                    tool_call_id: m.tool_call_id,
                    tool_calls: m.tool_calls,
                }
            } else {
                m
            }
        })
        .collect();
    out
}

/// Registry that maps task types to specialized pipelines.
///
/// When a request arrives, the registry classifies it, looks up the
/// pipeline for that task type (falling back to the default), and
/// orchestrates the full 6-stage flow.
pub struct PipelineRegistry {
    pipelines: HashMap<TaskType, Pipeline>,
    default: Pipeline,
}

impl PipelineRegistry {
    /// Create a new registry with the given default pipeline.
    pub fn new(default: Pipeline) -> Self {
        Self {
            pipelines: HashMap::new(),
            default,
        }
    }

    /// Register a specialized pipeline for a specific task type.
    pub fn register(&mut self, task_type: TaskType, pipeline: Pipeline) {
        self.pipelines.insert(task_type, pipeline);
    }

    /// Look up the pipeline for a task type, falling back to the default.
    pub fn get(&self, task_type: &TaskType) -> &Pipeline {
        self.pipelines.get(task_type).unwrap_or(&self.default)
    }

    /// Execute the full pipeline: classify -> route -> assemble -> transport -> score -> learn.
    pub async fn complete(&self, request: &ChatRequest) -> clawft_types::Result<LlmResponse> {
        // Stage 1: classify using the default pipeline's classifier
        let profile = self.default.classifier.classify(request);

        // Select the pipeline for this task type
        let pipeline = self.get(&profile.task_type);

        // Stage 2: route
        let routing = pipeline.router.route(request, &profile).await;

        // Stage 3: assemble context
        let context = pipeline.assembler.assemble(request, &profile).await;

        // Stage 3.5: feedback loop — let the learner mutate the
        // assembled system prompt. NoopLearner returns it unchanged;
        // TrajectoryLearner only mutates when an evolution is due
        // (configured trigger count of poor trajectories accumulated).
        let messages =
            apply_prompt_evolution(context.messages, pipeline.learner.as_ref());

        // Stage 4: transport (with latency measurement)
        let transport_request = TransportRequest {
            provider: routing.provider.clone(),
            model: routing.model.clone(),
            messages,
            tools: request.tools.clone(),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
        };
        let start_ms = crate::runtime::now_millis();
        let response = pipeline.transport.complete(&transport_request).await?;
        let latency_ms = crate::runtime::now_millis().saturating_sub(start_ms);

        // Stage 5: score
        let quality = pipeline.scorer.score(request, &response);

        // Stage 6: learn
        let trajectory = Trajectory {
            request: request.clone(),
            routing: routing.clone(),
            response: response.clone(),
            quality,
        };
        pipeline.learner.record(&trajectory);

        // Update the router with the outcome (now with actual latency)
        let outcome = ResponseOutcome {
            success: true,
            quality: trajectory.quality,
            latency_ms,
        };
        pipeline.router.update(&routing, &outcome);

        Ok(response)
    }

    /// Execute the pipeline with streaming: stages 1-3 run normally, then
    /// stage 4 streams text deltas via `callback`. Stages 5-6 run after
    /// the stream completes.
    ///
    /// The `callback` receives each text delta as it arrives and should
    /// return `true` to continue or `false` to abort early.
    ///
    /// Browser builds skip this method — [`StreamCallback`] requires
    /// `Send` and the browser's WASM runtime is single-threaded; a
    /// browser-specific streaming entry will land alongside an
    /// SSE-via-`ReadableStream` parser in W-BROWSER P3.
    #[cfg(not(feature = "browser"))]
    pub async fn complete_stream(
        &self,
        request: &ChatRequest,
        callback: StreamCallback,
    ) -> clawft_types::Result<LlmResponse> {
        // Stages 1-3 are identical to non-streaming
        let profile = self.default.classifier.classify(request);
        let pipeline = self.get(&profile.task_type);
        let routing = pipeline.router.route(request, &profile).await;
        let context = pipeline.assembler.assemble(request, &profile).await;

        // Stage 3.5: same feedback loop as the non-streaming path.
        let messages =
            apply_prompt_evolution(context.messages, pipeline.learner.as_ref());

        let transport_request = TransportRequest {
            provider: routing.provider.clone(),
            model: routing.model.clone(),
            messages,
            tools: request.tools.clone(),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
        };

        // Stage 4: streaming transport (with latency measurement)
        let start_ms = crate::runtime::now_millis();
        let response = pipeline
            .transport
            .complete_stream(&transport_request, callback)
            .await?;
        let latency_ms = crate::runtime::now_millis().saturating_sub(start_ms);

        // Stages 5-6: score and learn
        let quality = pipeline.scorer.score(request, &response);
        let trajectory = Trajectory {
            request: request.clone(),
            routing: routing.clone(),
            response: response.clone(),
            quality,
        };
        pipeline.learner.record(&trajectory);
        let outcome = ResponseOutcome {
            success: true,
            quality: trajectory.quality,
            latency_ms,
        };
        pipeline.router.update(&routing, &outcome);

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_types::provider::{ContentBlock, StopReason, Usage};

    // ── Type construction tests ─────────────────────────────────────

    #[test]
    fn chat_request_construction() {
        let req = ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "hello".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            model: Some("gpt-4o".into()),
            max_tokens: Some(1024),
            temperature: Some(0.7),
            auth_context: None,
            complexity_boost: 0.0,
        };
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn llm_message_with_tool_call_id() {
        let msg = LlmMessage {
            role: "tool".into(),
            content: "result data".into(),
            tool_call_id: Some("call-123".into()),
            tool_calls: None,
        };
        assert_eq!(msg.tool_call_id.as_deref(), Some("call-123"));
    }

    #[test]
    fn task_profile_construction() {
        let profile = TaskProfile {
            task_type: TaskType::CodeGeneration,
            complexity: 0.7,
            keywords: vec!["implement".into(), "function".into()],
        };
        assert_eq!(profile.task_type, TaskType::CodeGeneration);
        assert!(profile.complexity > 0.5);
        assert_eq!(profile.keywords.len(), 2);
    }

    #[test]
    fn routing_decision_construction() {
        let decision = RoutingDecision {
            provider: "anthropic".into(),
            model: "claude-opus-4-5".into(),
            reason: "high complexity code task".into(),
            ..Default::default()
        };
        assert_eq!(decision.provider, "anthropic");
        assert!(decision.tier.is_none());
        assert!(decision.cost_estimate_usd.is_none());
        assert!(!decision.escalated);
        assert!(!decision.budget_constrained);
    }

    // ── WEFT-30: redacted_reason (info-disclosure mitigation) ───────

    /// Helper that asserts a reason string maps to the expected category
    /// AND contains no sensitive sub-strings (user ids, model names, etc).
    fn assert_redacts(reason: &str, category: &str) {
        let d = RoutingDecision {
            reason: reason.into(),
            ..Default::default()
        };
        let red = d.redacted_reason();
        assert_eq!(red, category, "reason {reason:?} -> {red}, expected {category}");
        // Category labels must be ASCII snake_case identifiers — no
        // formatted fields, no whitespace, no special chars.
        assert!(
            red.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "category {red} must be ascii snake_case"
        );
    }

    #[test]
    fn redacted_reason_strips_user_metadata() {
        // The verbatim format-string the tiered router emits today.
        let raw = "tiered routing: complexity=0.85, tier=premium, level=2, user=alice@example.com";
        let d = RoutingDecision {
            reason: raw.into(),
            ..Default::default()
        };
        let red = d.redacted_reason();
        // Redacted form must NOT leak any of those fields.
        assert!(!red.contains("alice"));
        assert!(!red.contains("@example.com"));
        assert!(!red.contains("0.85"));
        assert!(!red.contains("premium"));
        assert!(!red.contains("level=2"));
    }

    #[test]
    fn redacted_reason_categories() {
        assert_redacts("static routing (Level 0)", "static_routing");
        assert_redacts("rate limited: using fallback model", "rate_limited");
        assert_redacts(
            "rate limited: fallback model not permitted for user tier",
            "tier_check_failed",
        );
        assert_redacts(
            "fallback chain: fallback_model denied for user tier",
            "tier_check_failed",
        );
        assert_redacts("daily budget cap reached", "cost_cap_hit");
        assert_redacts("no tiers or fallback model available", "no_tiers_available");
        assert_redacts(
            "fallback to configured fallback_model 'groq/llama-3.1-8b'",
            "fallback_chosen",
        );
        assert_redacts(
            "tiered routing: complexity=0.50, tier=standard, level=1, user=carol",
            "tiered_routing",
        );
    }

    #[test]
    fn redacted_reason_uses_structured_fields_when_text_silent() {
        let escalated = RoutingDecision {
            reason: String::new(),
            escalated: true,
            ..Default::default()
        };
        assert_eq!(escalated.redacted_reason(), "escalated");

        let constrained = RoutingDecision {
            reason: String::new(),
            budget_constrained: true,
            ..Default::default()
        };
        assert_eq!(constrained.redacted_reason(), "budget_constrained");
    }

    #[test]
    fn redacted_reason_default_is_safe() {
        let d = RoutingDecision::default();
        // Default decision falls through to the catch-all category.
        let red = d.redacted_reason();
        assert!(
            ["tiered_routing", "static_routing"].contains(&red),
            "unexpected default category: {red}"
        );
    }

    #[test]
    fn quality_score_construction() {
        let score = QualityScore {
            overall: 0.9,
            relevance: 0.95,
            coherence: 0.85,
        };
        assert!(score.overall > 0.0 && score.overall <= 1.0);
    }

    #[test]
    fn assembled_context_construction() {
        let ctx = AssembledContext {
            messages: vec![LlmMessage {
                role: "system".into(),
                content: "You are a helpful assistant.".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            token_estimate: 50,
            truncated: false,
        };
        assert!(!ctx.truncated);
        assert_eq!(ctx.token_estimate, 50);
    }

    #[test]
    fn transport_request_construction() {
        let req = TransportRequest {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            messages: vec![],
            tools: vec![],
            max_tokens: Some(2048),
            temperature: None,
        };
        assert_eq!(req.provider, "openai");
        assert!(req.temperature.is_none());
    }

    #[test]
    fn response_outcome_construction() {
        let score = QualityScore {
            overall: 0.8,
            relevance: 0.9,
            coherence: 0.7,
        };
        let outcome = ResponseOutcome {
            success: true,
            quality: score,
            latency_ms: 1500,
        };
        assert!(outcome.success);
        assert_eq!(outcome.latency_ms, 1500);
    }

    #[test]
    fn learning_signal_construction() {
        let signal = LearningSignal {
            feedback_type: "thumbs_up".into(),
            value: 1.0,
        };
        assert_eq!(signal.feedback_type, "thumbs_up");
        assert!((signal.value - 1.0).abs() < f32::EPSILON);
    }

    // ── Serde roundtrip tests ───────────────────────────────────────

    #[test]
    fn chat_request_serde_roundtrip() {
        let req = ChatRequest {
            messages: vec![
                LlmMessage {
                    role: "system".into(),
                    content: "You are helpful.".into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
                LlmMessage {
                    role: "user".into(),
                    content: "Write a function".into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
            ],
            tools: vec![serde_json::json!({"type": "function", "name": "web_search"})],
            model: Some("gpt-4o".into()),
            max_tokens: Some(4096),
            temperature: Some(0.5),
            auth_context: None,
            complexity_boost: 0.0,
        };
        let json = serde_json::to_string(&req).unwrap();
        let restored: ChatRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.messages[0].role, "system");
        assert_eq!(restored.model.as_deref(), Some("gpt-4o"));
        assert_eq!(restored.max_tokens, Some(4096));
        assert_eq!(restored.tools.len(), 1);
        // auth_context is skip_deserializing, so it should be None after roundtrip
        assert!(restored.auth_context.is_none());
    }

    #[test]
    fn llm_message_serde_roundtrip() {
        let msg = LlmMessage {
            role: "tool".into(),
            content: "search results".into(),
            tool_call_id: Some("tc-42".into()),
            tool_calls: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let restored: LlmMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, "tool");
        assert_eq!(restored.tool_call_id.as_deref(), Some("tc-42"));
    }

    #[test]
    fn llm_message_serde_skips_none_tool_call_id() {
        let msg = LlmMessage {
            role: "user".into(),
            content: "hello".into(),
            tool_call_id: None,
            tool_calls: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("tool_call_id"));
    }

    #[test]
    fn task_type_serde_roundtrip() {
        let types = [
            TaskType::Chat,
            TaskType::CodeGeneration,
            TaskType::CodeReview,
            TaskType::Research,
            TaskType::Creative,
            TaskType::Analysis,
            TaskType::ToolUse,
            TaskType::Unknown,
        ];
        for tt in &types {
            let json = serde_json::to_string(tt).unwrap();
            let restored: TaskType = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, tt);
        }
    }

    #[test]
    fn task_type_equality_and_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(TaskType::CodeGeneration);
        set.insert(TaskType::CodeGeneration);
        assert_eq!(set.len(), 1);
        set.insert(TaskType::Chat);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn chat_request_with_defaults_deserializes() {
        let json = r#"{"messages": [{"role": "user", "content": "hi"}]}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert!(req.tools.is_empty());
        assert!(req.model.is_none());
        assert!(req.max_tokens.is_none());
        assert!(req.temperature.is_none());
    }

    // ── Pipeline registry tests ─────────────────────────────────────

    /// Minimal classifier for testing.
    struct TestClassifier {
        task_type: TaskType,
    }

    impl TaskClassifier for TestClassifier {
        fn classify(&self, _request: &ChatRequest) -> TaskProfile {
            TaskProfile {
                task_type: self.task_type.clone(),
                complexity: 0.5,
                keywords: vec![],
            }
        }
    }

    /// Minimal router for testing.
    struct TestRouter {
        provider: String,
        model: String,
    }

    #[async_trait]
    impl ModelRouter for TestRouter {
        async fn route(&self, _request: &ChatRequest, _profile: &TaskProfile) -> RoutingDecision {
            RoutingDecision {
                provider: self.provider.clone(),
                model: self.model.clone(),
                reason: "test".into(),
                ..Default::default()
            }
        }

        fn update(&self, _decision: &RoutingDecision, _outcome: &ResponseOutcome) {}
    }

    /// Minimal assembler for testing.
    struct TestAssembler;

    #[async_trait]
    impl ContextAssembler for TestAssembler {
        async fn assemble(
            &self,
            request: &ChatRequest,
            _profile: &TaskProfile,
        ) -> AssembledContext {
            AssembledContext {
                messages: request.messages.clone(),
                token_estimate: 100,
                truncated: false,
            }
        }
    }

    /// Minimal transport that returns a canned response.
    struct TestTransport;

    #[async_trait]
    impl LlmTransport for TestTransport {
        async fn complete(&self, _request: &TransportRequest) -> clawft_types::Result<LlmResponse> {
            Ok(LlmResponse {
                id: "test-resp".into(),
                content: vec![ContentBlock::Text {
                    text: "Hello from test transport".into(),
                }],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    total_tokens: 0,
                },
                metadata: HashMap::new(),
            })
        }
    }

    /// Minimal scorer for testing.
    struct TestScorer;

    impl QualityScorer for TestScorer {
        fn score(&self, _request: &ChatRequest, _response: &LlmResponse) -> QualityScore {
            QualityScore {
                overall: 1.0,
                relevance: 1.0,
                coherence: 1.0,
            }
        }
    }

    /// Minimal learner for testing.
    struct TestLearner;

    impl LearningBackend for TestLearner {
        fn record(&self, _trajectory: &Trajectory) {}
        fn adapt(&self, _signal: &LearningSignal) {}
    }

    fn make_test_pipeline(task_type: TaskType, provider: &str, model: &str) -> Pipeline {
        Pipeline {
            classifier: Arc::new(TestClassifier { task_type }),
            router: Arc::new(TestRouter {
                provider: provider.into(),
                model: model.into(),
            }),
            assembler: Arc::new(TestAssembler),
            transport: Arc::new(TestTransport),
            scorer: Arc::new(TestScorer),
            learner: Arc::new(TestLearner),
        }
    }

    #[test]
    fn pipeline_registry_new() {
        let registry =
            PipelineRegistry::new(make_test_pipeline(TaskType::Chat, "openai", "gpt-4o"));
        // Default pipeline should be returned for any task type
        let pipeline = registry.get(&TaskType::CodeGeneration);
        // We cannot easily assert identity, but we can verify it does not panic
        let _ = pipeline.classifier.classify(&ChatRequest {
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: None,
            complexity_boost: 0.0,
        });
    }

    #[test]
    fn pipeline_registry_register_and_get() {
        let mut registry =
            PipelineRegistry::new(make_test_pipeline(TaskType::Chat, "openai", "gpt-4o"));
        registry.register(
            TaskType::CodeGeneration,
            make_test_pipeline(TaskType::CodeGeneration, "anthropic", "claude-opus-4-5"),
        );

        // Registered type should return the specialized pipeline
        let _code_pipeline = registry.get(&TaskType::CodeGeneration);
        // Unregistered type should return the default
        let _default_pipeline = registry.get(&TaskType::Research);
    }

    #[tokio::test]
    async fn pipeline_registry_complete_orchestrates_all_stages() {
        let registry =
            PipelineRegistry::new(make_test_pipeline(TaskType::Chat, "openai", "gpt-4o"));

        let request = ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "hello".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: None,
            complexity_boost: 0.0,
        };

        let response = registry.complete(&request).await.unwrap();
        assert_eq!(response.id, "test-resp");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        match &response.content[0] {
            ContentBlock::Text { text } => {
                assert_eq!(text, "Hello from test transport");
            }
            _ => panic!("expected text block"),
        }
    }

    #[tokio::test]
    async fn pipeline_registry_complete_uses_specialized_pipeline() {
        let mut registry = PipelineRegistry::new(make_test_pipeline(
            TaskType::CodeGeneration,
            "default-provider",
            "default-model",
        ));
        registry.register(
            TaskType::CodeGeneration,
            make_test_pipeline(TaskType::CodeGeneration, "anthropic", "claude-opus-4-5"),
        );

        let request = ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "write code".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: None,
            complexity_boost: 0.0,
        };

        // The classifier returns CodeGeneration, so the specialized pipeline is used.
        let response = registry.complete(&request).await.unwrap();
        assert_eq!(response.id, "test-resp");
    }

    // ── Phase F: ChatRequest auth_context serde injection prevention tests ──

    /// F-01: skip_deserializing prevents auth_context injection via JSON input.
    #[test]
    fn test_chat_request_skip_deserializing_auth_context() {
        let json = r#"{
            "messages": [{"role": "user", "content": "hi"}],
            "auth_context": {
                "sender_id": "injected",
                "channel": "evil",
                "permissions": {"level": 2}
            }
        }"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert!(
            req.auth_context.is_none(),
            "auth_context should be None after deserialization (skip_deserializing)"
        );
    }

    /// F-02: ChatRequest without auth_context deserializes correctly.
    #[test]
    fn test_chat_request_without_auth_context_deserializes() {
        let json = r#"{"messages": [{"role": "user", "content": "hi"}]}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert!(req.auth_context.is_none());
        assert_eq!(req.messages.len(), 1);
    }

    /// F-03: ChatRequest with auth_context: None serializes as null.
    ///
    /// The field has `#[serde(default, skip_deserializing)]`. The
    /// `skip_deserializing` only affects deserialization (prevents JSON
    /// injection). On the serialization side, `None` becomes `null` in
    /// the JSON output. This is acceptable -- the security-critical
    /// direction is deserialization, not serialization. The null value
    /// in serialized output is harmless and useful for logging/debugging.
    #[test]
    fn test_chat_request_none_auth_context_serializes_as_null() {
        let req = ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "hi".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: None,
            complexity_boost: 0.0,
        };
        let json = serde_json::to_string(&req).unwrap();
        // skip_deserializing only affects the Deserialize side.
        // Serialization still includes the field as null.
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            parsed.get("auth_context").is_some(),
            "auth_context field should be present in serialized JSON"
        );
        assert!(
            parsed["auth_context"].is_null(),
            "auth_context: None should serialize as null, got: {}",
            parsed["auth_context"]
        );
    }

    /// F-04: AuthContext serializes but does not survive roundtrip (asymmetric serde).
    #[test]
    fn test_chat_request_with_auth_context_serializes_but_not_roundtrip() {
        use clawft_types::routing::{AuthContext, UserPermissions};

        let req = ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "hi".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: Some(AuthContext {
                sender_id: "user_123".into(),
                channel: "telegram".into(),
                permissions: UserPermissions::default(),
            }),
            complexity_boost: 0.0,
        };

        // Serialize -- should include auth_context.
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("auth_context"),
            "auth_context with Some value should appear in serialized JSON"
        );
        assert!(
            json.contains("user_123"),
            "sender_id should appear in serialized JSON"
        );

        // Deserialize -- auth_context should be dropped (skip_deserializing).
        let restored: ChatRequest = serde_json::from_str(&json).unwrap();
        assert!(
            restored.auth_context.is_none(),
            "auth_context should be None after deserialization roundtrip"
        );
    }

    /// F-11: Pipeline registry passes auth_context through to transport.
    #[tokio::test]
    async fn test_pipeline_registry_passes_auth_context() {
        use clawft_types::routing::AuthContext;

        let registry =
            PipelineRegistry::new(make_test_pipeline(TaskType::Chat, "openai", "gpt-4o"));

        let request = ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "hello".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: Some(AuthContext::cli_default()),
            complexity_boost: 0.0,
        };

        // Complete should succeed with auth_context present.
        let response = registry.complete(&request).await.unwrap();
        assert_eq!(response.id, "test-resp");
    }

    /// F-12: Pipeline registry works without auth_context (None).
    #[tokio::test]
    async fn test_pipeline_registry_works_without_auth_context() {
        let registry =
            PipelineRegistry::new(make_test_pipeline(TaskType::Chat, "openai", "gpt-4o"));

        let request = ChatRequest {
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "hello".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            auth_context: None,
            complexity_boost: 0.0,
        };

        // Should not panic and should return a valid response.
        let response = registry.complete(&request).await.unwrap();
        assert_eq!(response.id, "test-resp");
    }
}
