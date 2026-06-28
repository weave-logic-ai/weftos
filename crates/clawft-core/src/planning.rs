//! Planning strategies with guard rails.
//!
//! Implements ReAct (Reason+Act) and Plan-and-Execute strategies for the
//! router, with configurable guard rails to prevent infinite loops and
//! cost runaway.
//!
//! # Guard rails
//!
//! | Parameter | Default | Description |
//! |-----------|---------|-------------|
//! | `max_planning_depth` | 10 | Max planning steps before forced termination |
//! | `max_planning_cost_usd` | 1.0 | Hard budget cap for LLM calls during planning |
//! | `planning_step_timeout` | 60s | Max duration for a single planning step |
//! | Circuit breaker | 3 | Consecutive no-op steps before abort |
//!
//! # Configuration
//!
//! ```toml
//! [router.planning]
//! max_planning_depth = 10
//! max_planning_cost_usd = 1.0
//! planning_step_timeout = "60s"
//! circuit_breaker_no_op_limit = 3
//! ```

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use tracing::{info, warn};

/// Default maximum planning depth.
const DEFAULT_MAX_DEPTH: u32 = 10;

/// Default maximum planning cost in USD.
const DEFAULT_MAX_COST_USD: f64 = 1.0;

/// Default planning step timeout in seconds.
const DEFAULT_STEP_TIMEOUT_SECS: u64 = 60;

/// Default number of consecutive no-op steps before circuit breaker.
const DEFAULT_NO_OP_LIMIT: u32 = 3;

/// Planning strategy.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningStrategy {
    /// Reason+Act loop: alternate between reasoning about the current
    /// state and taking an action.
    #[serde(alias = "ReAct")]
    React,

    /// Plan-and-Execute: generate a full plan first, then execute
    /// each step sequentially.
    #[serde(alias = "PlanAndExecute")]
    PlanAndExecute,
}

/// Configuration for the planning router.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningConfig {
    /// Maximum number of planning steps before forced termination.
    #[serde(default = "default_max_depth")]
    pub max_planning_depth: u32,

    /// Hard budget cap for LLM calls during planning (USD).
    #[serde(default = "default_max_cost")]
    pub max_planning_cost_usd: f64,

    /// Maximum duration for a single planning step.
    #[serde(
        default = "default_step_timeout",
        serialize_with = "serialize_duration_secs",
        deserialize_with = "deserialize_duration_secs"
    )]
    pub planning_step_timeout: Duration,

    /// Number of consecutive no-op steps before circuit breaker triggers.
    #[serde(default = "default_no_op_limit")]
    pub circuit_breaker_no_op_limit: u32,
}

fn default_max_depth() -> u32 {
    DEFAULT_MAX_DEPTH
}

fn default_max_cost() -> f64 {
    DEFAULT_MAX_COST_USD
}

fn default_step_timeout() -> Duration {
    Duration::from_secs(DEFAULT_STEP_TIMEOUT_SECS)
}

fn default_no_op_limit() -> u32 {
    DEFAULT_NO_OP_LIMIT
}

impl Default for PlanningConfig {
    fn default() -> Self {
        Self {
            max_planning_depth: default_max_depth(),
            max_planning_cost_usd: default_max_cost(),
            planning_step_timeout: default_step_timeout(),
            circuit_breaker_no_op_limit: default_no_op_limit(),
        }
    }
}

/// Reason a planning session was terminated.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    /// Planning completed successfully.
    Completed,
    /// Maximum depth reached.
    MaxDepthReached,
    /// Budget cap exceeded.
    BudgetExceeded,
    /// Step timeout exceeded.
    StepTimeout,
    /// Circuit breaker triggered (too many no-op steps).
    CircuitBreaker,
    /// Cancelled by user or agent shutdown.
    Cancelled,
}

/// Result of a planning step.
#[derive(Debug, Clone)]
pub struct PlanningStepResult {
    /// Step number (0-indexed).
    pub step: u32,
    /// Whether this step produced actionable output.
    pub is_actionable: bool,
    /// Cost of this step in USD.
    pub cost_usd: f64,
    /// Duration of this step.
    pub duration: Duration,
    /// Output from this step.
    pub output: String,
}

/// A plan produced by the planner step in a Plan-and-Execute session.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Ordered list of step descriptions to execute.
    pub steps: Vec<String>,
    /// Cost of producing this plan (charged to the budget cap).
    pub cost_usd: f64,
}

impl Plan {
    /// Create a plan with the given steps and cost.
    pub fn new(steps: Vec<String>, cost_usd: f64) -> Self {
        Self { steps, cost_usd }
    }

    /// Number of steps in the plan.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the plan has no steps.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Outcome of a planning session, including partial results.
#[derive(Debug, Clone)]
pub struct PlanningOutcome {
    /// Strategy that was used.
    pub strategy: PlanningStrategy,
    /// Why the planning session ended.
    pub termination_reason: TerminationReason,
    /// Number of steps executed.
    pub steps_executed: u32,
    /// Total cost in USD.
    pub total_cost_usd: f64,
    /// Total duration.
    pub total_duration: Duration,
    /// Results from each step.
    pub step_results: Vec<PlanningStepResult>,
    /// Human-readable explanation of the outcome.
    pub explanation: String,
}

/// Router that applies planning strategies with guard rails.
pub struct PlanningRouter {
    strategy: PlanningStrategy,
    config: PlanningConfig,
}

impl PlanningRouter {
    /// Create a new planning router.
    pub fn new(strategy: PlanningStrategy, config: PlanningConfig) -> Self {
        Self { strategy, config }
    }

    /// Create a planning router with default configuration.
    pub fn with_defaults(strategy: PlanningStrategy) -> Self {
        Self::new(strategy, PlanningConfig::default())
    }

    /// Get the configured strategy.
    pub fn strategy(&self) -> PlanningStrategy {
        self.strategy
    }

    /// Get the planning configuration.
    pub fn config(&self) -> &PlanningConfig {
        &self.config
    }

    /// Validate a planning step against guard rails.
    ///
    /// Returns `Some(TerminationReason)` if a limit has been reached,
    /// or `None` if the step is within limits.
    pub fn check_guard_rails(
        &self,
        current_step: u32,
        total_cost_usd: f64,
        consecutive_no_ops: u32,
    ) -> Option<TerminationReason> {
        if current_step >= self.config.max_planning_depth {
            warn!(
                step = current_step,
                max = self.config.max_planning_depth,
                "planning depth limit reached"
            );
            return Some(TerminationReason::MaxDepthReached);
        }

        if total_cost_usd >= self.config.max_planning_cost_usd {
            warn!(
                cost = total_cost_usd,
                max = self.config.max_planning_cost_usd,
                "planning budget cap exceeded"
            );
            return Some(TerminationReason::BudgetExceeded);
        }

        if consecutive_no_ops >= self.config.circuit_breaker_no_op_limit {
            warn!(
                no_ops = consecutive_no_ops,
                limit = self.config.circuit_breaker_no_op_limit,
                "circuit breaker triggered: too many no-op steps"
            );
            return Some(TerminationReason::CircuitBreaker);
        }

        None
    }

    /// Execute a Plan-and-Execute planning session.
    ///
    /// Generates a plan via `planner` (a single LLM-style call that
    /// returns a list of step descriptions and an estimated cost),
    /// then runs `step_executor` on each step in order, threading
    /// guard rails (depth, budget, timeout, circuit breaker) the
    /// whole way.
    ///
    /// # Arguments
    ///
    /// * `goal` - the task description passed to the planner.
    /// * `planner` - async closure that turns a goal into a [`Plan`].
    /// * `step_executor` - async closure that runs a single step and
    ///   returns a [`PlanningStepResult`].
    ///
    /// # Returns
    ///
    /// A [`PlanningOutcome`] capturing every step that ran (including
    /// the planning step), the termination reason, and a human
    /// explanation. Partial results are always returned, never
    /// dropped — even on guard-rail termination.
    pub async fn execute_plan_and_execute<P, S, PFut, SFut>(
        &self,
        goal: &str,
        planner: P,
        step_executor: S,
    ) -> PlanningOutcome
    where
        P: FnOnce(String) -> PFut,
        PFut: Future<Output = std::result::Result<Plan, String>>,
        S: Fn(u32, String) -> SFut,
        SFut: Future<Output = std::result::Result<PlanningStepResult, String>>,
    {
        if self.strategy != PlanningStrategy::PlanAndExecute {
            warn!(
                got = ?self.strategy,
                "execute_plan_and_execute called on a non-PlanAndExecute router; running anyway"
            );
        }

        let total_start = Instant::now();
        let mut step_results: Vec<PlanningStepResult> = Vec::new();
        let mut total_cost: f64 = 0.0;
        let mut consecutive_no_ops: u32 = 0;

        // ── Step 0: planning ────────────────────────────────────────
        let plan_start = Instant::now();
        let plan = match planner(goal.to_string()).await {
            Ok(p) => p,
            Err(msg) => {
                let dur = plan_start.elapsed();
                let explanation = format!("Planning step failed: {msg}");
                step_results.push(PlanningStepResult {
                    step: 0,
                    is_actionable: false,
                    cost_usd: 0.0,
                    duration: dur,
                    output: msg,
                });
                return PlanningOutcome {
                    strategy: self.strategy,
                    termination_reason: TerminationReason::Cancelled,
                    steps_executed: 1,
                    total_cost_usd: total_cost,
                    total_duration: total_start.elapsed(),
                    step_results,
                    explanation,
                };
            }
        };

        total_cost += plan.cost_usd;
        step_results.push(PlanningStepResult {
            step: 0,
            is_actionable: !plan.steps.is_empty(),
            cost_usd: plan.cost_usd,
            duration: plan_start.elapsed(),
            output: format!("Generated plan with {} step(s)", plan.steps.len()),
        });

        info!(
            goal,
            plan_steps = plan.steps.len(),
            plan_cost = plan.cost_usd,
            "plan-and-execute: plan generated"
        );

        // Check budget after planning step.
        if let Some(reason) = self.check_guard_rails(0, total_cost, consecutive_no_ops) {
            let explanation = self.explain_termination(&reason, 1, total_cost);
            return PlanningOutcome {
                strategy: self.strategy,
                termination_reason: reason,
                steps_executed: 1,
                total_cost_usd: total_cost,
                total_duration: total_start.elapsed(),
                step_results,
                explanation,
            };
        }

        // ── Step 1..N: execute plan steps in order ──────────────────
        for (idx, step_desc) in plan.steps.iter().enumerate() {
            // 1-indexed for downstream display; idx+1 is the step
            // number passed to the executor and recorded.
            let step_num = (idx + 1) as u32;

            // Guard rails before running the step.
            if let Some(reason) = self.check_guard_rails(step_num, total_cost, consecutive_no_ops) {
                let explanation = self.explain_termination(&reason, step_num, total_cost);
                return PlanningOutcome {
                    strategy: self.strategy,
                    termination_reason: reason,
                    steps_executed: step_num,
                    total_cost_usd: total_cost,
                    total_duration: total_start.elapsed(),
                    step_results,
                    explanation,
                };
            }

            // Run step with per-step timeout from config.
            let step_fut = step_executor(step_num, step_desc.clone());
            let timeout = self.config.planning_step_timeout;
            let res = tokio::time::timeout(timeout, step_fut).await;

            match res {
                Ok(Ok(mut result)) => {
                    // Force step number from us (defensive) and
                    // increment counters.
                    result.step = step_num;
                    total_cost += result.cost_usd;
                    if result.is_actionable {
                        consecutive_no_ops = 0;
                    } else {
                        consecutive_no_ops += 1;
                    }
                    step_results.push(result);
                }
                Ok(Err(msg)) => {
                    // Step explicitly failed; record and continue
                    // counting toward circuit breaker.
                    consecutive_no_ops += 1;
                    step_results.push(PlanningStepResult {
                        step: step_num,
                        is_actionable: false,
                        cost_usd: 0.0,
                        duration: Duration::ZERO,
                        output: format!("step failed: {msg}"),
                    });
                }
                Err(_elapsed) => {
                    // Timeout: record + terminate.
                    step_results.push(PlanningStepResult {
                        step: step_num,
                        is_actionable: false,
                        cost_usd: 0.0,
                        duration: timeout,
                        output: "step timed out".into(),
                    });
                    let reason = TerminationReason::StepTimeout;
                    let explanation = self.explain_termination(&reason, step_num, total_cost);
                    return PlanningOutcome {
                        strategy: self.strategy,
                        termination_reason: reason,
                        steps_executed: step_num,
                        total_cost_usd: total_cost,
                        total_duration: total_start.elapsed(),
                        step_results,
                        explanation,
                    };
                }
            }
        }

        // ── Plan exhausted normally ─────────────────────────────────
        let steps_executed = step_results.len() as u32;
        let explanation =
            self.explain_termination(&TerminationReason::Completed, steps_executed, total_cost);
        PlanningOutcome {
            strategy: self.strategy,
            termination_reason: TerminationReason::Completed,
            steps_executed,
            total_cost_usd: total_cost,
            total_duration: total_start.elapsed(),
            step_results,
            explanation,
        }
    }

    /// Execute a ReAct (Reason+Act) planning session.
    ///
    /// **Not yet implemented.** Returns a [`PlanningOutcome`] with
    /// [`TerminationReason::Cancelled`] and an explanation pointing
    /// to the deferred work item.
    ///
    /// Tracked under `0.8.x` cycle; see the WEFT board for the live
    /// item linking back to this stub.
    #[allow(clippy::unused_async)]
    pub async fn execute_react(&self, _goal: &str) -> PlanningOutcome {
        let explanation = "execute_react: not yet implemented (deferred to 0.8.x; \
              use PlanAndExecute strategy for now)"
            .to_string();
        warn!("{explanation}");
        PlanningOutcome {
            strategy: self.strategy,
            termination_reason: TerminationReason::Cancelled,
            steps_executed: 0,
            total_cost_usd: 0.0,
            total_duration: Duration::ZERO,
            step_results: Vec::new(),
            explanation,
        }
    }

    /// Build a partial results explanation for the given termination reason.
    pub fn explain_termination(&self, reason: &TerminationReason, steps: u32, cost: f64) -> String {
        match reason {
            TerminationReason::Completed => {
                format!("Planning completed successfully after {steps} steps (${cost:.4} spent).")
            }
            TerminationReason::MaxDepthReached => {
                format!(
                    "Planning terminated: maximum depth of {} steps reached. \
                     Partial results from {steps} steps returned (${cost:.4} spent).",
                    self.config.max_planning_depth
                )
            }
            TerminationReason::BudgetExceeded => {
                format!(
                    "Planning terminated: budget cap of ${:.2} exceeded (${cost:.4} spent). \
                     Partial results from {steps} steps returned.",
                    self.config.max_planning_cost_usd
                )
            }
            TerminationReason::StepTimeout => {
                format!(
                    "Planning terminated: step timeout of {:?} exceeded at step {steps}. \
                     Partial results returned (${cost:.4} spent).",
                    self.config.planning_step_timeout
                )
            }
            TerminationReason::CircuitBreaker => {
                format!(
                    "Planning terminated: {} consecutive no-op steps detected (circuit breaker). \
                     Partial results from {steps} steps returned (${cost:.4} spent).",
                    self.config.circuit_breaker_no_op_limit
                )
            }
            TerminationReason::Cancelled => {
                format!(
                    "Planning cancelled by user or agent shutdown at step {steps} (${cost:.4} spent)."
                )
            }
        }
    }
}

impl std::fmt::Debug for PlanningRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanningRouter")
            .field("strategy", &self.strategy)
            .field("config", &self.config)
            .finish()
    }
}

fn serialize_duration_secs<S>(d: &Duration, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.serialize_u64(d.as_secs())
}

fn deserialize_duration_secs<'de, D>(d: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let secs = u64::deserialize(d)?;
    Ok(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_config_defaults() {
        let cfg = PlanningConfig::default();
        assert_eq!(cfg.max_planning_depth, 10);
        assert!((cfg.max_planning_cost_usd - 1.0).abs() < f64::EPSILON);
        assert_eq!(cfg.planning_step_timeout, Duration::from_secs(60));
        assert_eq!(cfg.circuit_breaker_no_op_limit, 3);
    }

    #[test]
    fn planning_config_serde() {
        let cfg = PlanningConfig {
            max_planning_depth: 5,
            max_planning_cost_usd: 0.5,
            planning_step_timeout: Duration::from_secs(30),
            circuit_breaker_no_op_limit: 2,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: PlanningConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.max_planning_depth, 5);
        assert!((restored.max_planning_cost_usd - 0.5).abs() < f64::EPSILON);
        assert_eq!(restored.planning_step_timeout, Duration::from_secs(30));
        assert_eq!(restored.circuit_breaker_no_op_limit, 2);
    }

    #[test]
    fn guard_rails_no_limit_hit() {
        let router = PlanningRouter::with_defaults(PlanningStrategy::React);
        assert!(router.check_guard_rails(0, 0.0, 0).is_none());
        assert!(router.check_guard_rails(5, 0.5, 1).is_none());
    }

    #[test]
    fn guard_rails_max_depth() {
        let router = PlanningRouter::with_defaults(PlanningStrategy::React);
        let result = router.check_guard_rails(10, 0.0, 0);
        assert_eq!(result, Some(TerminationReason::MaxDepthReached));
    }

    #[test]
    fn guard_rails_budget_exceeded() {
        let router = PlanningRouter::with_defaults(PlanningStrategy::React);
        let result = router.check_guard_rails(5, 1.0, 0);
        assert_eq!(result, Some(TerminationReason::BudgetExceeded));
    }

    #[test]
    fn guard_rails_circuit_breaker() {
        let router = PlanningRouter::with_defaults(PlanningStrategy::React);
        let result = router.check_guard_rails(5, 0.5, 3);
        assert_eq!(result, Some(TerminationReason::CircuitBreaker));
    }

    #[test]
    fn guard_rails_custom_limits() {
        let router = PlanningRouter::new(
            PlanningStrategy::PlanAndExecute,
            PlanningConfig {
                max_planning_depth: 3,
                max_planning_cost_usd: 0.25,
                circuit_breaker_no_op_limit: 1,
                ..Default::default()
            },
        );
        // Depth limit.
        assert_eq!(
            router.check_guard_rails(3, 0.0, 0),
            Some(TerminationReason::MaxDepthReached)
        );
        // Budget limit.
        assert_eq!(
            router.check_guard_rails(1, 0.25, 0),
            Some(TerminationReason::BudgetExceeded)
        );
        // Circuit breaker.
        assert_eq!(
            router.check_guard_rails(1, 0.0, 1),
            Some(TerminationReason::CircuitBreaker)
        );
    }

    #[test]
    fn explain_termination_messages() {
        let router = PlanningRouter::with_defaults(PlanningStrategy::React);

        let msg = router.explain_termination(&TerminationReason::Completed, 5, 0.3);
        assert!(msg.contains("successfully"));
        assert!(msg.contains("5 steps"));

        let msg = router.explain_termination(&TerminationReason::MaxDepthReached, 10, 0.8);
        assert!(msg.contains("maximum depth"));
        assert!(msg.contains("Partial results"));

        let msg = router.explain_termination(&TerminationReason::BudgetExceeded, 7, 1.2);
        assert!(msg.contains("budget cap"));

        let msg = router.explain_termination(&TerminationReason::CircuitBreaker, 4, 0.1);
        assert!(msg.contains("no-op steps"));
        assert!(msg.contains("circuit breaker"));

        let msg = router.explain_termination(&TerminationReason::StepTimeout, 3, 0.2);
        assert!(msg.contains("step timeout"));

        let msg = router.explain_termination(&TerminationReason::Cancelled, 2, 0.05);
        assert!(msg.contains("cancelled"));
    }

    #[test]
    fn planning_strategy_serde() {
        let json = serde_json::to_string(&PlanningStrategy::React).unwrap();
        assert_eq!(json, "\"react\"");

        let json = serde_json::to_string(&PlanningStrategy::PlanAndExecute).unwrap();
        assert_eq!(json, "\"plan_and_execute\"");

        let restored: PlanningStrategy = serde_json::from_str("\"react\"").unwrap();
        assert_eq!(restored, PlanningStrategy::React);

        let restored: PlanningStrategy = serde_json::from_str("\"plan_and_execute\"").unwrap();
        assert_eq!(restored, PlanningStrategy::PlanAndExecute);
    }

    #[test]
    fn termination_reason_serde() {
        let json = serde_json::to_string(&TerminationReason::Completed).unwrap();
        assert_eq!(json, "\"completed\"");

        let json = serde_json::to_string(&TerminationReason::CircuitBreaker).unwrap();
        assert_eq!(json, "\"circuit_breaker\"");
    }

    #[test]
    fn router_accessors() {
        let router = PlanningRouter::with_defaults(PlanningStrategy::PlanAndExecute);
        assert_eq!(router.strategy(), PlanningStrategy::PlanAndExecute);
        assert_eq!(router.config().max_planning_depth, 10);
    }

    // ── execute_plan_and_execute / execute_react tests (WEFT-183) ───────

    #[tokio::test]
    async fn plan_and_execute_runs_all_steps() {
        let router = PlanningRouter::with_defaults(PlanningStrategy::PlanAndExecute);
        let outcome = router
            .execute_plan_and_execute(
                "do thing",
                |_goal: String| async {
                    Ok(Plan::new(
                        vec!["step a".into(), "step b".into(), "step c".into()],
                        0.01,
                    ))
                },
                |step, desc: String| async move {
                    Ok(PlanningStepResult {
                        step,
                        is_actionable: true,
                        cost_usd: 0.005,
                        duration: Duration::from_millis(1),
                        output: desc,
                    })
                },
            )
            .await;
        assert_eq!(outcome.termination_reason, TerminationReason::Completed);
        // 1 plan step + 3 execution steps.
        assert_eq!(outcome.steps_executed, 4);
        // Total cost = 0.01 (plan) + 3 * 0.005 = 0.025.
        assert!((outcome.total_cost_usd - 0.025).abs() < 1e-9);
    }

    #[tokio::test]
    async fn plan_and_execute_planner_error_records_partial() {
        let router = PlanningRouter::with_defaults(PlanningStrategy::PlanAndExecute);
        let outcome = router
            .execute_plan_and_execute(
                "do thing",
                |_goal: String| async { Err::<Plan, _>("planner exploded".to_string()) },
                |_, _: String| async {
                    Ok(PlanningStepResult {
                        step: 0,
                        is_actionable: true,
                        cost_usd: 0.0,
                        duration: Duration::ZERO,
                        output: String::new(),
                    })
                },
            )
            .await;
        assert_eq!(outcome.termination_reason, TerminationReason::Cancelled);
        assert_eq!(outcome.steps_executed, 1);
        assert_eq!(outcome.step_results.len(), 1);
        assert!(outcome.explanation.contains("planner exploded"));
    }

    #[tokio::test]
    async fn plan_and_execute_circuit_breaker_on_no_ops() {
        let router = PlanningRouter::new(
            PlanningStrategy::PlanAndExecute,
            PlanningConfig {
                circuit_breaker_no_op_limit: 2,
                ..Default::default()
            },
        );
        let outcome = router
            .execute_plan_and_execute(
                "task",
                |_: String| async {
                    Ok(Plan::new(
                        vec!["a".into(), "b".into(), "c".into(), "d".into()],
                        0.0,
                    ))
                },
                |step, _: String| async move {
                    Ok(PlanningStepResult {
                        step,
                        is_actionable: false, // no-op
                        cost_usd: 0.0,
                        duration: Duration::from_millis(1),
                        output: String::new(),
                    })
                },
            )
            .await;
        assert_eq!(
            outcome.termination_reason,
            TerminationReason::CircuitBreaker
        );
        // Plan step + 2 no-ops = 3 steps before breaker triggers on
        // the 3rd. (consecutive_no_ops becomes 2 after step 2; the
        // guard rail check runs before step 3 and trips.)
        assert!(outcome.steps_executed <= 3);
    }

    #[tokio::test]
    async fn plan_and_execute_budget_exceeded() {
        let router = PlanningRouter::new(
            PlanningStrategy::PlanAndExecute,
            PlanningConfig {
                max_planning_cost_usd: 0.05,
                ..Default::default()
            },
        );
        let outcome = router
            .execute_plan_and_execute(
                "task",
                |_: String| async { Ok(Plan::new(vec!["a".into(), "b".into()], 0.10)) },
                |step, _: String| async move {
                    Ok(PlanningStepResult {
                        step,
                        is_actionable: true,
                        cost_usd: 0.0,
                        duration: Duration::ZERO,
                        output: String::new(),
                    })
                },
            )
            .await;
        // The planning step alone busts the budget.
        assert_eq!(
            outcome.termination_reason,
            TerminationReason::BudgetExceeded
        );
    }

    #[tokio::test]
    async fn plan_and_execute_step_timeout() {
        let router = PlanningRouter::new(
            PlanningStrategy::PlanAndExecute,
            PlanningConfig {
                planning_step_timeout: Duration::from_millis(50),
                ..Default::default()
            },
        );
        let outcome = router
            .execute_plan_and_execute(
                "task",
                |_: String| async { Ok(Plan::new(vec!["slow".into()], 0.0)) },
                |step, _: String| async move {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    Ok(PlanningStepResult {
                        step,
                        is_actionable: true,
                        cost_usd: 0.0,
                        duration: Duration::from_millis(200),
                        output: String::new(),
                    })
                },
            )
            .await;
        assert_eq!(outcome.termination_reason, TerminationReason::StepTimeout);
    }

    #[tokio::test]
    async fn execute_react_returns_not_yet_implemented() {
        let router = PlanningRouter::with_defaults(PlanningStrategy::React);
        let outcome = router.execute_react("task").await;
        assert_eq!(outcome.termination_reason, TerminationReason::Cancelled);
        assert!(outcome.explanation.contains("not yet implemented"));
        assert_eq!(outcome.steps_executed, 0);
    }

    #[test]
    fn plan_struct() {
        let p = Plan::new(vec!["a".into()], 0.1);
        assert_eq!(p.len(), 1);
        assert!(!p.is_empty());
        assert!(Plan::new(vec![], 0.0).is_empty());
    }
}
