//! Routing configuration validation.
//!
//! Post-deserialization validation for [`RoutingConfig`] and its nested types.
//! Serde handles structural correctness (JSON types, field names), but cannot
//! enforce semantic constraints like "tier names must be unique" or "complexity
//! min must be less than max." This module fills that gap.
//!
//! Validation runs after deserialization, before `TieredRouter` construction.
//! When `routing.mode` is `"static"`, validation is skipped entirely.

use std::collections::HashSet;

use clawft_types::routing::{ModelTierConfig, PermissionLevelConfig, RoutingConfig};

// ── ValidationSeverity ──────────────────────────────────────────────────

/// Severity level for a validation diagnostic.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationSeverity {
    /// Fatal: the config is invalid and the system cannot start.
    Error,
    /// Non-fatal: the config is technically valid but may indicate a
    /// misconfiguration. Logged at WARN level during startup.
    Warning,
}

// ── ValidationError ─────────────────────────────────────────────────────

/// A single validation diagnostic for the routing config.
///
/// Collects a human-readable field path, a descriptive message, and a
/// severity level. All diagnostics are gathered before returning so
/// operators see every issue at once.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Dotted field path (e.g., `"routing.tiers[1].complexity_range"`).
    pub field: String,
    /// Human-readable description of the problem.
    pub message: String,
    /// Whether this is a fatal error or a non-fatal warning.
    pub severity: ValidationSeverity,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.severity {
            ValidationSeverity::Error => "ERROR",
            ValidationSeverity::Warning => "WARN",
        };
        write!(f, "[{}] {}: {}", level, self.field, self.message)
    }
}

impl std::error::Error for ValidationError {}

// ── Main validation entry point ─────────────────────────────────────────

/// Validate a [`RoutingConfig`] and return all errors/warnings found.
///
/// When `mode` is `"static"`, validation is skipped entirely (static mode
/// ignores routing config). Validation only applies when `mode = "tiered"`.
///
/// All diagnostics are collected in a single pass -- the function does not
/// short-circuit on the first error.
pub fn validate_routing_config(config: &RoutingConfig) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Static mode skips all routing validation (backward compatibility).
    if config.mode == "static" {
        return errors;
    }

    validate_mode(config, &mut errors);
    validate_tiers(config, &mut errors);
    validate_permissions(config, &mut errors);
    validate_escalation(config, &mut errors);
    validate_cost_budgets(config, &mut errors);
    validate_rate_limiting(config, &mut errors);
    validate_fallback_model(config, &mut errors);

    errors
}

// ── Mode validation ─────────────────────────────────────────────────────

/// Rule 1: `mode` must be `"static"` or `"tiered"`.
fn validate_mode(config: &RoutingConfig, errors: &mut Vec<ValidationError>) {
    if config.mode != "static" && config.mode != "tiered" {
        errors.push(ValidationError {
            field: "routing.mode".into(),
            message: format!(
                "unknown mode '{}', expected 'static' or 'tiered'",
                config.mode
            ),
            severity: ValidationSeverity::Error,
        });
    }
}

// ── Tier validation ─────────────────────────────────────────────────────

/// Rules 2-8: tier-level validations.
fn validate_tiers(config: &RoutingConfig, errors: &mut Vec<ValidationError>) {
    // Rule 2: if mode is "tiered", at least one tier must be defined.
    if config.mode == "tiered" && config.tiers.is_empty() {
        errors.push(ValidationError {
            field: "routing.tiers".into(),
            message: "tiered mode requires at least one tier definition".into(),
            severity: ValidationSeverity::Error,
        });
    }

    // Rule 3: no duplicate tier names.
    let mut seen_names: HashSet<&str> = HashSet::new();
    for (i, tier) in config.tiers.iter().enumerate() {
        if !seen_names.insert(&tier.name) {
            errors.push(ValidationError {
                field: format!("routing.tiers[{}].name", i),
                message: format!("duplicate tier name '{}'", tier.name),
                severity: ValidationSeverity::Error,
            });
        }

        validate_single_tier(i, tier, errors);
    }

    // Warning: overlapping complexity ranges (intentional by design but
    // worth logging for operator awareness).
    for i in 0..config.tiers.len() {
        for j in (i + 1)..config.tiers.len() {
            let a = &config.tiers[i];
            let b = &config.tiers[j];
            let overlap_min = a.complexity_range[0].max(b.complexity_range[0]);
            let overlap_max = a.complexity_range[1].min(b.complexity_range[1]);
            if overlap_min < overlap_max {
                errors.push(ValidationError {
                    field: format!("routing.tiers[{}]+[{}]", i, j),
                    message: format!(
                        "tiers '{}' and '{}' have overlapping complexity ranges \
                         [{:.2}, {:.2}]",
                        a.name, b.name, overlap_min, overlap_max
                    ),
                    severity: ValidationSeverity::Warning,
                });
            }
        }
    }
}

/// Validate a single tier's fields.
fn validate_single_tier(index: usize, tier: &ModelTierConfig, errors: &mut Vec<ValidationError>) {
    let prefix = format!("routing.tiers[{}]", index);

    // Rule 4: each tier must have at least one model (warning).
    if tier.models.is_empty() {
        errors.push(ValidationError {
            field: format!("{}.models", prefix),
            message: format!(
                "tier '{}' has no models -- it is defined but unusable",
                tier.name
            ),
            severity: ValidationSeverity::Warning,
        });
    }

    // Rule 5: complexity_range[0] <= complexity_range[1].
    let [min, max] = tier.complexity_range;
    if min > max {
        errors.push(ValidationError {
            field: format!("{}.complexity_range", prefix),
            message: format!(
                "complexity_range min ({}) > max ({}) for tier '{}'",
                min, max, tier.name
            ),
            severity: ValidationSeverity::Error,
        });
    }

    // Rule 6: complexity_range values in [0.0, 1.0].
    if !(0.0..=1.0).contains(&min) {
        errors.push(ValidationError {
            field: format!("{}.complexity_range[0]", prefix),
            message: format!(
                "complexity_range[0] = {} is outside [0.0, 1.0] for tier '{}'",
                min, tier.name
            ),
            severity: ValidationSeverity::Error,
        });
    }
    if !(0.0..=1.0).contains(&max) {
        errors.push(ValidationError {
            field: format!("{}.complexity_range[1]", prefix),
            message: format!(
                "complexity_range[1] = {} is outside [0.0, 1.0] for tier '{}'",
                max, tier.name
            ),
            severity: ValidationSeverity::Error,
        });
    }

    // Rule 7: cost_per_1k_tokens >= 0.0.
    if tier.cost_per_1k_tokens < 0.0 {
        errors.push(ValidationError {
            field: format!("{}.cost_per_1k_tokens", prefix),
            message: format!(
                "cost_per_1k_tokens = {} must be non-negative for tier '{}'",
                tier.cost_per_1k_tokens, tier.name
            ),
            severity: ValidationSeverity::Error,
        });
    }

    // Rule 8: max_context_tokens > 0.
    if tier.max_context_tokens == 0 {
        errors.push(ValidationError {
            field: format!("{}.max_context_tokens", prefix),
            message: format!("max_context_tokens must be > 0 for tier '{}'", tier.name),
            severity: ValidationSeverity::Error,
        });
    }

    // Warning: model format should contain '/' (provider/model).
    for (j, model) in tier.models.iter().enumerate() {
        if !model.contains('/') {
            errors.push(ValidationError {
                field: format!("{}.models[{}]", prefix, j),
                message: format!(
                    "model '{}' does not contain '/' (expected 'provider/model' format)",
                    model
                ),
                severity: ValidationSeverity::Warning,
            });
        }
    }
}

// ── Permission validation ───────────────────────────────────────────────

/// Rules 9-14: permission-level validations.
fn validate_permissions(config: &RoutingConfig, errors: &mut Vec<ValidationError>) {
    let valid_tier_names: HashSet<&str> = config.tiers.iter().map(|t| t.name.as_str()).collect();

    // Validate the three named permission levels.
    validate_permission_level(
        &config.permissions.zero_trust,
        "routing.permissions.zero_trust",
        &valid_tier_names,
        errors,
    );
    validate_permission_level(
        &config.permissions.user,
        "routing.permissions.user",
        &valid_tier_names,
        errors,
    );
    validate_permission_level(
        &config.permissions.admin,
        "routing.permissions.admin",
        &valid_tier_names,
        errors,
    );

    // Validate per-user overrides.
    for (user_id, plc) in &config.permissions.users {
        validate_permission_level(
            plc,
            &format!("routing.permissions.users.{}", user_id),
            &valid_tier_names,
            errors,
        );
    }

    // Validate per-channel overrides.
    for (channel, plc) in &config.permissions.channels {
        validate_permission_level(
            plc,
            &format!("routing.permissions.channels.{}", channel),
            &valid_tier_names,
            errors,
        );
    }
}

/// Validate a single [`PermissionLevelConfig`].
fn validate_permission_level(
    plc: &PermissionLevelConfig,
    field_prefix: &str,
    valid_tiers: &HashSet<&str>,
    errors: &mut Vec<ValidationError>,
) {
    // Rule 9: level values are 0, 1, or 2 (warn on others).
    if let Some(level) = plc.level
        && level > 2 {
            errors.push(ValidationError {
                field: format!("{}.level", field_prefix),
                message: format!(
                    "permission level {} is not recognized (expected 0, 1, or 2)",
                    level
                ),
                severity: ValidationSeverity::Warning,
            });
        }

    // Rule 10: escalation_threshold in [0.0, 1.0].
    if let Some(threshold) = plc.escalation_threshold
        && (!(0.0..=1.0).contains(&threshold)) {
            errors.push(ValidationError {
                field: format!("{}.escalation_threshold", field_prefix),
                message: format!("escalation_threshold {} must be in [0.0, 1.0]", threshold),
                severity: ValidationSeverity::Error,
            });
        }

    // Rule 11: cost_budget_daily_usd >= 0.0.
    if let Some(daily) = plc.cost_budget_daily_usd
        && daily < 0.0 {
            errors.push(ValidationError {
                field: format!("{}.cost_budget_daily_usd", field_prefix),
                message: format!("cost_budget_daily_usd {} must be non-negative", daily),
                severity: ValidationSeverity::Error,
            });
        }

    // Rule 12: cost_budget_monthly_usd >= 0.0.
    if let Some(monthly) = plc.cost_budget_monthly_usd
        && monthly < 0.0 {
            errors.push(ValidationError {
                field: format!("{}.cost_budget_monthly_usd", field_prefix),
                message: format!("cost_budget_monthly_usd {} must be non-negative", monthly),
                severity: ValidationSeverity::Error,
            });
        }

    // Rule 13: max_tier references an existing tier name (warning only).
    if let Some(ref max_tier) = plc.max_tier
        && !valid_tiers.is_empty() && !valid_tiers.contains(max_tier.as_str()) {
            errors.push(ValidationError {
                field: format!("{}.max_tier", field_prefix),
                message: format!(
                    "max_tier '{}' does not match any defined tier (available: {})",
                    max_tier,
                    valid_tiers.iter().copied().collect::<Vec<_>>().join(", ")
                ),
                severity: ValidationSeverity::Warning,
            });
        }

    // Rule 14: tool access patterns with `*` in middle generate warning.
    if let Some(ref tool_access) = plc.tool_access {
        for entry in tool_access {
            if entry.contains('*') && entry != "*" {
                errors.push(ValidationError {
                    field: format!("{}.tool_access", field_prefix),
                    message: format!(
                        "glob-like pattern '{}' detected; ensure pattern matching is intended",
                        entry
                    ),
                    severity: ValidationSeverity::Warning,
                });
            }
        }
    }
}

// ── Escalation validation ───────────────────────────────────────────────

/// Rules 15-16: escalation validations.
fn validate_escalation(config: &RoutingConfig, errors: &mut Vec<ValidationError>) {
    // Rule 15: max_escalation_tiers > 0 when escalation is enabled.
    if config.escalation.enabled && config.escalation.max_escalation_tiers == 0 {
        errors.push(ValidationError {
            field: "routing.escalation.max_escalation_tiers".into(),
            message: "max_escalation_tiers must be > 0 when escalation is enabled".into(),
            severity: ValidationSeverity::Error,
        });
    }

    // Rule 16: escalation threshold in [0.0, 1.0].
    if config.escalation.threshold < 0.0 || config.escalation.threshold > 1.0 {
        errors.push(ValidationError {
            field: "routing.escalation.threshold".into(),
            message: format!(
                "escalation threshold {} must be in [0.0, 1.0]",
                config.escalation.threshold
            ),
            severity: ValidationSeverity::Error,
        });
    }

    // Warning: max_escalation_tiers exceeds defined tier count.
    if config.escalation.max_escalation_tiers > config.tiers.len() as u32 {
        errors.push(ValidationError {
            field: "routing.escalation.max_escalation_tiers".into(),
            message: format!(
                "max_escalation_tiers ({}) exceeds the number of defined tiers ({})",
                config.escalation.max_escalation_tiers,
                config.tiers.len()
            ),
            severity: ValidationSeverity::Warning,
        });
    }
}

// ── Cost budget validation ──────────────────────────────────────────────

/// Cost budget validations.
fn validate_cost_budgets(config: &RoutingConfig, errors: &mut Vec<ValidationError>) {
    if config.cost_budgets.global_daily_limit_usd < 0.0 {
        errors.push(ValidationError {
            field: "routing.cost_budgets.global_daily_limit_usd".into(),
            message: format!(
                "global_daily_limit_usd {} must be non-negative",
                config.cost_budgets.global_daily_limit_usd
            ),
            severity: ValidationSeverity::Error,
        });
    }

    if config.cost_budgets.global_monthly_limit_usd < 0.0 {
        errors.push(ValidationError {
            field: "routing.cost_budgets.global_monthly_limit_usd".into(),
            message: format!(
                "global_monthly_limit_usd {} must be non-negative",
                config.cost_budgets.global_monthly_limit_usd
            ),
            severity: ValidationSeverity::Error,
        });
    }

    if config.cost_budgets.reset_hour_utc > 23 {
        errors.push(ValidationError {
            field: "routing.cost_budgets.reset_hour_utc".into(),
            message: format!(
                "reset_hour_utc {} must be 0-23",
                config.cost_budgets.reset_hour_utc
            ),
            severity: ValidationSeverity::Error,
        });
    }
}

// ── Rate limiting validation ────────────────────────────────────────────

/// Rate limiting validations.
fn validate_rate_limiting(config: &RoutingConfig, errors: &mut Vec<ValidationError>) {
    if config.rate_limiting.window_seconds == 0 {
        errors.push(ValidationError {
            field: "routing.rate_limiting.window_seconds".into(),
            message: "window_seconds must be > 0".into(),
            severity: ValidationSeverity::Error,
        });
    }

    let valid_strategies = ["sliding_window", "fixed_window"];
    if !valid_strategies.contains(&config.rate_limiting.strategy.as_str()) {
        errors.push(ValidationError {
            field: "routing.rate_limiting.strategy".into(),
            message: format!(
                "unknown strategy '{}', expected 'sliding_window' or 'fixed_window'",
                config.rate_limiting.strategy
            ),
            severity: ValidationSeverity::Error,
        });
    }
}

// ── Fallback model validation ───────────────────────────────────────────

/// Rule 17: fallback model format.
fn validate_fallback_model(config: &RoutingConfig, errors: &mut Vec<ValidationError>) {
    if let Some(ref model) = config.fallback_model
        && !model.contains('/') {
            errors.push(ValidationError {
                field: "routing.fallback_model".into(),
                message: format!(
                    "fallback_model '{}' should match 'provider/model' format",
                    model
                ),
                severity: ValidationSeverity::Warning,
            });
        }
}

// ── Workspace ceiling enforcement (FIX-04) ──────────────────────────────

/// Default maximum grantable permission level for workspace configs.
///
/// Until `max_grantable_level` is added to [`RoutingConfig`], this constant
/// provides the default ceiling. Workspaces cannot grant levels above this.
const DEFAULT_MAX_GRANTABLE_LEVEL: u8 = 1;

/// Validate that a workspace config does not exceed the global config ceiling.
///
/// Returns a [`Vec`] of validation errors for any ceiling violations found.
/// An empty [`Vec`] means the workspace is within bounds.
///
/// Security-sensitive ceiling fields:
/// - `level`: cannot exceed `DEFAULT_MAX_GRANTABLE_LEVEL` (1)
/// - `escalation_allowed`: cannot be true if global is false
/// - `tool_access`: cannot add tools not in global allowlist
/// - `rate_limit`: workspace cannot increase beyond global
/// - `cost_budget_daily_usd`: workspace cannot increase
/// - `cost_budget_monthly_usd`: workspace cannot increase
/// - `max_tier`: cannot upgrade beyond global's max_tier
pub fn validate_workspace_ceiling(
    global: &RoutingConfig,
    workspace: &RoutingConfig,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let max_grantable = DEFAULT_MAX_GRANTABLE_LEVEL;

    // Check the three named permission levels.
    check_level_ceiling(
        &workspace.permissions.zero_trust,
        "routing.permissions.zero_trust",
        &global.permissions.zero_trust,
        max_grantable,
        &mut errors,
    );
    check_level_ceiling(
        &workspace.permissions.user,
        "routing.permissions.user",
        &global.permissions.user,
        max_grantable,
        &mut errors,
    );
    check_level_ceiling(
        &workspace.permissions.admin,
        "routing.permissions.admin",
        &global.permissions.admin,
        max_grantable,
        &mut errors,
    );

    // Check per-user overrides.
    for (user_id, ws_plc) in &workspace.permissions.users {
        let global_plc = global
            .permissions
            .users
            .get(user_id)
            .unwrap_or(&global.permissions.user);
        check_level_ceiling(
            ws_plc,
            &format!("routing.permissions.users.{}", user_id),
            global_plc,
            max_grantable,
            &mut errors,
        );
    }

    // Check per-channel overrides.
    for (channel, ws_plc) in &workspace.permissions.channels {
        let global_plc = global
            .permissions
            .channels
            .get(channel)
            .unwrap_or(&global.permissions.user);
        check_level_ceiling(
            ws_plc,
            &format!("routing.permissions.channels.{}", channel),
            global_plc,
            max_grantable,
            &mut errors,
        );
    }

    errors
}

/// Check a single workspace permission level against the global ceiling.
fn check_level_ceiling(
    ws_plc: &PermissionLevelConfig,
    field_prefix: &str,
    global_plc: &PermissionLevelConfig,
    max_grantable: u8,
    errors: &mut Vec<ValidationError>,
) {
    // Level ceiling: workspace cannot grant level above max_grantable_level.
    if let Some(ws_level) = ws_plc.level
        && ws_level > max_grantable {
            errors.push(ValidationError {
                field: format!("{}.level", field_prefix),
                message: format!(
                    "workspace level {} exceeds max_grantable_level {}",
                    ws_level, max_grantable
                ),
                severity: ValidationSeverity::Error,
            });
        }

    // Escalation ceiling: workspace cannot enable escalation if global disables it.
    if let (Some(true), Some(false)) = (ws_plc.escalation_allowed, global_plc.escalation_allowed) {
        errors.push(ValidationError {
            field: format!("{}.escalation_allowed", field_prefix),
            message: "workspace cannot enable escalation when global disables it".into(),
            severity: ValidationSeverity::Error,
        });
    }

    // Tool access ceiling: workspace cannot add tools not in global allowlist.
    if let (Some(ws_tools), Some(global_tools)) =
        (&ws_plc.tool_access, &global_plc.tool_access)
    {
        // If global is ["*"], anything goes.
        if !global_tools.iter().any(|s| s == "*") {
            for ws_tool in ws_tools {
                if ws_tool == "*" {
                    errors.push(ValidationError {
                        field: format!("{}.tool_access", field_prefix),
                        message: "workspace cannot grant wildcard tool access \
                                  when global does not"
                            .into(),
                        severity: ValidationSeverity::Error,
                    });
                } else if !global_tools.contains(ws_tool) {
                    errors.push(ValidationError {
                        field: format!("{}.tool_access", field_prefix),
                        message: format!("workspace tool '{}' not in global tool_access", ws_tool),
                        severity: ValidationSeverity::Error,
                    });
                }
            }
        }
    }

    // Rate limit ceiling: workspace cannot increase beyond global.
    if let (Some(ws_rate), Some(global_rate)) = (ws_plc.rate_limit, global_plc.rate_limit)
        && global_rate > 0 && (ws_rate > global_rate || ws_rate == 0) {
            errors.push(ValidationError {
                field: format!("{}.rate_limit", field_prefix),
                message: format!(
                    "workspace rate_limit {} exceeds global ceiling {}",
                    ws_rate, global_rate
                ),
                severity: ValidationSeverity::Error,
            });
        }

    // Cost budget daily ceiling.
    if let (Some(ws_daily), Some(global_daily)) = (
        ws_plc.cost_budget_daily_usd,
        global_plc.cost_budget_daily_usd,
    )
        && global_daily > 0.0 && (ws_daily > global_daily || ws_daily == 0.0) {
            errors.push(ValidationError {
                field: format!("{}.cost_budget_daily_usd", field_prefix),
                message: format!(
                    "workspace daily budget {} exceeds global ceiling {}",
                    ws_daily, global_daily
                ),
                severity: ValidationSeverity::Error,
            });
        }

    // Cost budget monthly ceiling.
    if let (Some(ws_monthly), Some(global_monthly)) = (
        ws_plc.cost_budget_monthly_usd,
        global_plc.cost_budget_monthly_usd,
    )
        && global_monthly > 0.0 && (ws_monthly > global_monthly || ws_monthly == 0.0) {
            errors.push(ValidationError {
                field: format!("{}.cost_budget_monthly_usd", field_prefix),
                message: format!(
                    "workspace monthly budget {} exceeds global ceiling {}",
                    ws_monthly, global_monthly
                ),
                severity: ValidationSeverity::Error,
            });
        }
}

// ── Default tiers ───────────────────────────────────────────────────────

/// Generate a set of default tiers when mode is `"tiered"` but no tiers
/// are configured.
///
/// This enables the minimal config migration path:
/// ```json
/// { "routing": { "mode": "tiered" } }
/// ```
///
/// Returns three standard tiers: free, standard, and premium.
pub fn default_tiers() -> Vec<ModelTierConfig> {
    vec![
        ModelTierConfig {
            name: "free".into(),
            models: vec![
                "openrouter/meta-llama/llama-3.1-8b-instruct:free".into(),
                "groq/llama-3.1-8b".into(),
            ],
            complexity_range: [0.0, 0.3],
            cost_per_1k_tokens: 0.0,
            max_context_tokens: 8192,
        },
        ModelTierConfig {
            name: "standard".into(),
            models: vec![
                "anthropic/claude-haiku-3.5".into(),
                "openai/gpt-4o-mini".into(),
                "groq/llama-3.3-70b".into(),
            ],
            complexity_range: [0.0, 0.7],
            cost_per_1k_tokens: 0.001,
            max_context_tokens: 16384,
        },
        ModelTierConfig {
            name: "premium".into(),
            models: vec![
                "anthropic/claude-sonnet-4-20250514".into(),
                "openai/gpt-4o".into(),
            ],
            complexity_range: [0.3, 1.0],
            cost_per_1k_tokens: 0.01,
            max_context_tokens: 200_000,
        },
    ]
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use clawft_types::routing::{EscalationConfig, ModelTierConfig, RateLimitConfig};

    /// Build a minimal valid tiered config for testing.
    fn valid_tiered_config() -> RoutingConfig {
        RoutingConfig {
            mode: "tiered".into(),
            tiers: vec![
                ModelTierConfig {
                    name: "free".into(),
                    models: vec!["groq/llama-3.1-8b".into()],
                    complexity_range: [0.0, 0.5],
                    cost_per_1k_tokens: 0.0,
                    max_context_tokens: 8192,
                },
                ModelTierConfig {
                    name: "standard".into(),
                    models: vec!["anthropic/claude-haiku-3.5".into()],
                    complexity_range: [0.3, 1.0],
                    cost_per_1k_tokens: 0.001,
                    max_context_tokens: 16384,
                },
            ],
            ..RoutingConfig::default()
        }
    }

    /// Build a minimal valid static config for testing.
    fn valid_static_config() -> RoutingConfig {
        RoutingConfig::default()
    }

    /// Count errors at a given severity level.
    fn count_severity(errors: &[ValidationError], severity: ValidationSeverity) -> usize {
        errors.iter().filter(|e| e.severity == severity).count()
    }

    /// Check if any error matches a field prefix and severity.
    fn has_error(
        errors: &[ValidationError],
        field_prefix: &str,
        severity: ValidationSeverity,
    ) -> bool {
        errors
            .iter()
            .any(|e| e.field.starts_with(field_prefix) && e.severity == severity)
    }

    // ── Test 1: valid static config passes ──────────────────────────

    #[test]
    fn valid_static_config_passes() {
        let config = valid_static_config();
        let errors = validate_routing_config(&config);
        assert!(errors.is_empty(), "static config should produce no errors");
    }

    // ── Test 2: valid tiered config passes ──────────────────────────

    #[test]
    fn valid_tiered_config_passes() {
        let config = valid_tiered_config();
        let errors = validate_routing_config(&config);
        let fatal = count_severity(&errors, ValidationSeverity::Error);
        assert_eq!(
            fatal, 0,
            "valid tiered config should have no fatal errors, got: {:?}",
            errors
        );
    }

    // ── Test 3: invalid mode rejected ───────────────────────────────

    #[test]
    fn invalid_mode_rejected() {
        let mut config = valid_tiered_config();
        config.mode = "hybrid".into();
        let errors = validate_routing_config(&config);
        assert!(
            has_error(&errors, "routing.mode", ValidationSeverity::Error),
            "unknown mode should produce an error"
        );
    }

    // ── Test 4: tiered with no tiers gives error ────────────────────

    #[test]
    fn tiered_with_no_tiers_gives_error() {
        let config = RoutingConfig {
            mode: "tiered".into(),
            tiers: vec![],
            ..RoutingConfig::default()
        };
        let errors = validate_routing_config(&config);
        assert!(
            has_error(&errors, "routing.tiers", ValidationSeverity::Error),
            "tiered mode with no tiers should be an error"
        );
    }

    // ── Test 5: duplicate tier names detected ───────────────────────

    #[test]
    fn duplicate_tier_names_detected() {
        let mut config = valid_tiered_config();
        config.tiers[1].name = "free".into(); // duplicate
        let errors = validate_routing_config(&config);
        assert!(
            has_error(&errors, "routing.tiers[1].name", ValidationSeverity::Error),
            "duplicate tier names should produce an error"
        );
    }

    // ── Test 6: empty tier models detected ──────────────────────────

    #[test]
    fn empty_tier_models_warning() {
        let mut config = valid_tiered_config();
        config.tiers[0].models.clear();
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.tiers[0].models",
                ValidationSeverity::Warning
            ),
            "empty models should produce a warning"
        );
    }

    // ── Test 7: invalid complexity range (min > max) ────────────────

    #[test]
    fn complexity_range_inverted() {
        let mut config = valid_tiered_config();
        config.tiers[0].complexity_range = [0.8, 0.3];
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.tiers[0].complexity_range",
                ValidationSeverity::Error
            ),
            "inverted complexity range should produce an error"
        );
    }

    // ── Test 8: complexity out of [0, 1] range ──────────────────────

    #[test]
    fn complexity_out_of_range() {
        let mut config = valid_tiered_config();
        config.tiers[0].complexity_range = [-0.1, 1.5];
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.tiers[0].complexity_range[0]",
                ValidationSeverity::Error
            ),
            "negative min should produce an error"
        );
        assert!(
            has_error(
                &errors,
                "routing.tiers[0].complexity_range[1]",
                ValidationSeverity::Error
            ),
            "max > 1.0 should produce an error"
        );
    }

    // ── Test 9: negative cost rejected ──────────────────────────────

    #[test]
    fn negative_cost_rejected() {
        let mut config = valid_tiered_config();
        config.tiers[0].cost_per_1k_tokens = -0.001;
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.tiers[0].cost_per_1k_tokens",
                ValidationSeverity::Error
            ),
            "negative cost should produce an error"
        );
    }

    // ── Test 10: zero max_context_tokens rejected ───────────────────

    #[test]
    fn zero_max_context_tokens_rejected() {
        let mut config = valid_tiered_config();
        config.tiers[0].max_context_tokens = 0;
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.tiers[0].max_context_tokens",
                ValidationSeverity::Error
            ),
            "zero max_context_tokens should produce an error"
        );
    }

    // ── Test 11: invalid level warning ──────────────────────────────

    #[test]
    fn invalid_permission_level_warning() {
        let mut config = valid_tiered_config();
        config.permissions.zero_trust.level = Some(5);
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.permissions.zero_trust.level",
                ValidationSeverity::Warning
            ),
            "invalid level should produce a warning"
        );
    }

    // ── Test 12: invalid escalation threshold ───────────────────────

    #[test]
    fn invalid_escalation_threshold() {
        let mut config = valid_tiered_config();
        config.permissions.user.escalation_threshold = Some(1.5);
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.escalation_threshold",
                ValidationSeverity::Error
            ),
            "escalation_threshold > 1.0 should produce an error"
        );
    }

    // ── Test 13: negative budget rejected ───────────────────────────

    #[test]
    fn negative_budget_rejected() {
        let mut config = valid_tiered_config();
        config.permissions.user.cost_budget_daily_usd = Some(-1.0);
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.cost_budget_daily_usd",
                ValidationSeverity::Error
            ),
            "negative daily budget should produce an error"
        );
    }

    // ── Test 14: max tier referencing non-existent tier warns ────────

    #[test]
    fn max_tier_nonexistent_warns() {
        let mut config = valid_tiered_config();
        config.permissions.user.max_tier = Some("mythical".into());
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.max_tier",
                ValidationSeverity::Warning
            ),
            "undefined max_tier should produce a warning"
        );
    }

    // ── Test 15: glob-like tool pattern warning ─────────────────────

    #[test]
    fn glob_like_tool_pattern_warning() {
        let mut config = valid_tiered_config();
        config.permissions.user.tool_access = Some(vec!["file_*".into()]);
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.tool_access",
                ValidationSeverity::Warning
            ),
            "glob-like tool_access should produce a warning"
        );
    }

    // ── Test 16: exact wildcard "*" does not warn ───────────────────

    #[test]
    fn exact_wildcard_no_warning() {
        let mut config = valid_tiered_config();
        config.permissions.admin.tool_access = Some(vec!["*".into()]);
        let errors = validate_routing_config(&config);
        let glob_warnings = errors.iter().any(|e| {
            e.field.contains("tool_access")
                && e.severity == ValidationSeverity::Warning
                && e.message.contains("glob")
        });
        assert!(
            !glob_warnings,
            "exact '*' should not produce a glob pattern warning"
        );
    }

    // ── Test 17: fallback model format validation ───────────────────

    #[test]
    fn fallback_model_format_warning() {
        let mut config = valid_tiered_config();
        config.fallback_model = Some("gpt-4o".into());
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.fallback_model",
                ValidationSeverity::Warning
            ),
            "fallback model without '/' should produce a warning"
        );
    }

    // ── Test 18: workspace ceiling -- level escalation blocked ──────

    #[test]
    fn workspace_ceiling_level_escalation_blocked() {
        let global = valid_tiered_config();
        let mut workspace = RoutingConfig::default();
        workspace.permissions.user.level = Some(2); // admin
        let errors = validate_workspace_ceiling(&global, &workspace);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.level",
                ValidationSeverity::Error
            ),
            "workspace level exceeding max_grantable should produce an error"
        );
    }

    // ── Test 19: workspace ceiling -- rate limit increase blocked ───

    #[test]
    fn workspace_ceiling_rate_limit_increase_blocked() {
        let mut global = valid_tiered_config();
        global.permissions.user.rate_limit = Some(60);
        let mut workspace = RoutingConfig::default();
        workspace.permissions.user.rate_limit = Some(120);
        let errors = validate_workspace_ceiling(&global, &workspace);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.rate_limit",
                ValidationSeverity::Error
            ),
            "workspace rate_limit exceeding global should produce an error"
        );
    }

    // ── Test 20: workspace ceiling -- budget increase blocked ───────

    #[test]
    fn workspace_ceiling_budget_increase_blocked() {
        let mut global = valid_tiered_config();
        global.permissions.user.cost_budget_daily_usd = Some(5.0);
        let mut workspace = RoutingConfig::default();
        workspace.permissions.user.cost_budget_daily_usd = Some(10.0);
        let errors = validate_workspace_ceiling(&global, &workspace);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.cost_budget_daily_usd",
                ValidationSeverity::Error
            ),
            "workspace daily budget exceeding global should produce an error"
        );
    }

    // ── Test 21: workspace ceiling -- tool access expansion blocked ─

    #[test]
    fn workspace_ceiling_tool_access_expansion_blocked() {
        let mut global = valid_tiered_config();
        global.permissions.user.tool_access = Some(vec!["read_file".into(), "list_dir".into()]);
        let mut workspace = RoutingConfig::default();
        workspace.permissions.user.tool_access = Some(vec!["exec_shell".into()]);
        let errors = validate_workspace_ceiling(&global, &workspace);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.tool_access",
                ValidationSeverity::Error
            ),
            "workspace adding tool not in global should produce an error"
        );
    }

    // ── Test 22: workspace within bounds passes ─────────────────────

    #[test]
    fn workspace_within_bounds_passes() {
        let mut global = valid_tiered_config();
        global.permissions.user.level = Some(1);
        global.permissions.user.rate_limit = Some(60);
        global.permissions.user.cost_budget_daily_usd = Some(5.0);

        let mut workspace = RoutingConfig::default();
        workspace.permissions.user.level = Some(1);
        workspace.permissions.user.rate_limit = Some(30);
        workspace.permissions.user.cost_budget_daily_usd = Some(3.0);

        let errors = validate_workspace_ceiling(&global, &workspace);
        let fatal = count_severity(&errors, ValidationSeverity::Error);
        assert_eq!(
            fatal, 0,
            "workspace within bounds should have no errors, got: {:?}",
            errors
        );
    }

    // ── Test 23: default tiers generation ───────────────────────────

    #[test]
    fn default_tiers_generation() {
        let tiers = default_tiers();
        assert_eq!(tiers.len(), 3, "should produce 3 default tiers");
        assert_eq!(tiers[0].name, "free");
        assert_eq!(tiers[1].name, "standard");
        assert_eq!(tiers[2].name, "premium");

        // Each tier should have at least one model.
        for tier in &tiers {
            assert!(
                !tier.models.is_empty(),
                "tier '{}' should have models",
                tier.name
            );
        }

        // Free tier should have zero cost.
        assert_eq!(tiers[0].cost_per_1k_tokens, 0.0);

        // Complexity ranges should be within [0.0, 1.0].
        for tier in &tiers {
            assert!(tier.complexity_range[0] >= 0.0);
            assert!(tier.complexity_range[1] <= 1.0);
            assert!(tier.complexity_range[0] <= tier.complexity_range[1]);
        }
    }

    // ── Test 24: default tiers pass validation ──────────────────────

    #[test]
    fn default_tiers_pass_validation() {
        let config = RoutingConfig {
            mode: "tiered".into(),
            tiers: default_tiers(),
            ..RoutingConfig::default()
        };
        let errors = validate_routing_config(&config);
        let fatal = count_severity(&errors, ValidationSeverity::Error);
        assert_eq!(
            fatal, 0,
            "default tiers should pass validation, got: {:?}",
            errors
        );
    }

    // ── Test 25: escalation threshold out of range ──────────────────

    #[test]
    fn escalation_global_threshold_out_of_range() {
        let mut config = valid_tiered_config();
        config.escalation = EscalationConfig {
            enabled: true,
            threshold: 1.5,
            max_escalation_tiers: 1,
        };
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.escalation.threshold",
                ValidationSeverity::Error
            ),
            "escalation threshold > 1.0 should produce an error"
        );
    }

    // ── Test 26: zero window_seconds rejected ───────────────────────

    #[test]
    fn zero_window_seconds_rejected() {
        let mut config = valid_tiered_config();
        config.rate_limiting.window_seconds = 0;
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.rate_limiting.window_seconds",
                ValidationSeverity::Error
            ),
            "zero window_seconds should produce an error"
        );
    }

    // ── Test 26b: window_seconds=1 accepted ──────────────────────────

    /// Lower boundary -- 1 second is the smallest legal sliding window.
    /// Pairs with `zero_window_seconds_rejected` to nail down the
    /// half-open `(0, ..]` admissible range (WEFT-29).
    #[test]
    fn one_second_window_accepted() {
        let mut config = valid_tiered_config();
        config.rate_limiting.window_seconds = 1;
        let errors = validate_routing_config(&config);
        assert!(
            !has_error(
                &errors,
                "routing.rate_limiting.window_seconds",
                ValidationSeverity::Error
            ),
            "window_seconds=1 must be accepted (lower boundary)"
        );
    }

    // ── Test 27: unknown rate limit strategy rejected ────────────────

    #[test]
    fn unknown_rate_limit_strategy_rejected() {
        let mut config = valid_tiered_config();
        config.rate_limiting.strategy = "token_bucket".into();
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.rate_limiting.strategy",
                ValidationSeverity::Error
            ),
            "unknown rate limit strategy should produce an error"
        );
    }

    // ── Test 28: reset_hour_utc out of range ────────────────────────

    #[test]
    fn reset_hour_out_of_range() {
        let mut config = valid_tiered_config();
        config.cost_budgets.reset_hour_utc = 25;
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.cost_budgets.reset_hour_utc",
                ValidationSeverity::Error
            ),
            "reset_hour_utc > 23 should produce an error"
        );
    }

    // ── Test 29: negative global daily limit ────────────────────────

    #[test]
    fn negative_global_daily_limit() {
        let mut config = valid_tiered_config();
        config.cost_budgets.global_daily_limit_usd = -10.0;
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.cost_budgets.global_daily_limit_usd",
                ValidationSeverity::Error
            ),
            "negative global daily limit should produce an error"
        );
    }

    // ── Test 30: negative global monthly limit ──────────────────────

    #[test]
    fn negative_global_monthly_limit() {
        let mut config = valid_tiered_config();
        config.cost_budgets.global_monthly_limit_usd = -100.0;
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.cost_budgets.global_monthly_limit_usd",
                ValidationSeverity::Error
            ),
            "negative global monthly limit should produce an error"
        );
    }

    // ── Test 31: multiple errors collected ──────────────────────────

    #[test]
    fn multiple_errors_collected() {
        let config = RoutingConfig {
            mode: "tiered".into(),
            tiers: vec![
                ModelTierConfig {
                    name: "bad".into(),
                    models: vec![],
                    complexity_range: [0.8, 0.3], // inverted
                    cost_per_1k_tokens: -1.0,     // negative
                    max_context_tokens: 0,        // zero
                },
                ModelTierConfig {
                    name: "bad".into(), // duplicate
                    models: vec![],
                    complexity_range: [0.0, 1.0],
                    cost_per_1k_tokens: 0.0,
                    max_context_tokens: 8192,
                },
            ],
            rate_limiting: RateLimitConfig {
                window_seconds: 0, // invalid
                strategy: "sliding_window".into(),
                ..RateLimitConfig::default()
            },
            ..RoutingConfig::default()
        };
        let errors = validate_routing_config(&config);
        let fatal_count = count_severity(&errors, ValidationSeverity::Error);
        assert!(
            fatal_count >= 4,
            "should collect at least 4 errors, got {}: {:?}",
            fatal_count,
            errors
        );
    }

    // ── Test 32: static mode skips validation of bad data ───────────

    #[test]
    fn static_mode_skips_validation() {
        let config = RoutingConfig {
            mode: "static".into(),
            tiers: vec![ModelTierConfig {
                name: "broken".into(),
                models: vec![],
                complexity_range: [0.8, 0.3], // would be invalid
                cost_per_1k_tokens: -1.0,
                max_context_tokens: 0,
            }],
            ..RoutingConfig::default()
        };
        let errors = validate_routing_config(&config);
        assert!(
            errors.is_empty(),
            "static mode should skip all validation, got: {:?}",
            errors
        );
    }

    // ── Test 33: overlapping complexity ranges warning ───────────────

    #[test]
    fn overlapping_complexity_ranges_warning() {
        let config = RoutingConfig {
            mode: "tiered".into(),
            tiers: vec![
                ModelTierConfig {
                    name: "a".into(),
                    models: vec!["provider/model-a".into()],
                    complexity_range: [0.0, 0.7],
                    cost_per_1k_tokens: 0.0,
                    max_context_tokens: 8192,
                },
                ModelTierConfig {
                    name: "b".into(),
                    models: vec!["provider/model-b".into()],
                    complexity_range: [0.3, 1.0],
                    cost_per_1k_tokens: 0.01,
                    max_context_tokens: 16384,
                },
            ],
            ..RoutingConfig::default()
        };
        let errors = validate_routing_config(&config);
        let overlap_warnings = errors.iter().any(|e| {
            e.severity == ValidationSeverity::Warning && e.message.contains("overlapping")
        });
        assert!(
            overlap_warnings,
            "overlapping ranges should produce a warning"
        );
    }

    // ── Test 34: model format warning ───────────────────────────────

    #[test]
    fn model_format_warning() {
        let mut config = valid_tiered_config();
        config.tiers[0].models = vec!["gpt-4o".into()]; // no slash
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.tiers[0].models[0]",
                ValidationSeverity::Warning
            ),
            "model without '/' should produce a warning"
        );
    }

    // ── Test 35: per-user override validation ───────────────────────

    #[test]
    fn per_user_override_validated() {
        let mut config = valid_tiered_config();
        config.permissions.users.insert(
            "alice".into(),
            PermissionLevelConfig {
                cost_budget_monthly_usd: Some(-5.0),
                ..PermissionLevelConfig::default()
            },
        );
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.permissions.users.alice.cost_budget_monthly_usd",
                ValidationSeverity::Error
            ),
            "per-user negative budget should produce an error"
        );
    }

    // ── Test 36: per-channel override validation ────────────────────

    #[test]
    fn per_channel_override_validated() {
        let mut config = valid_tiered_config();
        config.permissions.channels.insert(
            "telegram".into(),
            PermissionLevelConfig {
                escalation_threshold: Some(-0.5),
                ..PermissionLevelConfig::default()
            },
        );
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.permissions.channels.telegram.escalation_threshold",
                ValidationSeverity::Error
            ),
            "per-channel invalid threshold should produce an error"
        );
    }

    // ── Test 37: escalation max tiers exceeds defined count warns ───

    #[test]
    fn escalation_max_tiers_exceeds_defined_count_warns() {
        let mut config = valid_tiered_config();
        config.escalation = EscalationConfig {
            enabled: true,
            threshold: 0.6,
            max_escalation_tiers: 10, // only 2 tiers defined
        };
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.escalation.max_escalation_tiers",
                ValidationSeverity::Warning
            ),
            "max_escalation_tiers > tier count should warn"
        );
    }

    // ── Test 38: workspace ceiling -- monthly budget increase ───────

    #[test]
    fn workspace_ceiling_monthly_budget_increase_blocked() {
        let mut global = valid_tiered_config();
        global.permissions.user.cost_budget_monthly_usd = Some(50.0);
        let mut workspace = RoutingConfig::default();
        workspace.permissions.user.cost_budget_monthly_usd = Some(100.0);
        let errors = validate_workspace_ceiling(&global, &workspace);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.cost_budget_monthly_usd",
                ValidationSeverity::Error
            ),
            "workspace monthly budget exceeding global should produce an error"
        );
    }

    // ── Test 39: workspace ceiling -- escalation enable blocked ──────

    #[test]
    fn workspace_ceiling_escalation_enable_blocked() {
        let mut global = valid_tiered_config();
        global.permissions.user.escalation_allowed = Some(false);
        let mut workspace = RoutingConfig::default();
        workspace.permissions.user.escalation_allowed = Some(true);
        let errors = validate_workspace_ceiling(&global, &workspace);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.escalation_allowed",
                ValidationSeverity::Error
            ),
            "workspace enabling escalation when global disables should error"
        );
    }

    // ── Test 40: workspace wildcard tool access blocked ──────────────

    #[test]
    fn workspace_wildcard_tool_access_blocked() {
        let mut global = valid_tiered_config();
        global.permissions.user.tool_access = Some(vec!["read_file".into()]);
        let mut workspace = RoutingConfig::default();
        workspace.permissions.user.tool_access = Some(vec!["*".into()]);
        let errors = validate_workspace_ceiling(&global, &workspace);
        assert!(
            has_error(
                &errors,
                "routing.permissions.user.tool_access",
                ValidationSeverity::Error
            ),
            "workspace wildcard when global does not should error"
        );
    }

    // ── Test 41: workspace per-user override ceiling ────────────────

    #[test]
    fn workspace_per_user_override_ceiling() {
        let global = valid_tiered_config();
        let mut workspace = RoutingConfig::default();
        workspace.permissions.users.insert(
            "attacker".into(),
            PermissionLevelConfig {
                level: Some(2), // admin -- exceeds max_grantable=1
                ..PermissionLevelConfig::default()
            },
        );
        let errors = validate_workspace_ceiling(&global, &workspace);
        assert!(
            has_error(
                &errors,
                "routing.permissions.users.attacker.level",
                ValidationSeverity::Error
            ),
            "workspace per-user admin level should be blocked"
        );
    }

    // ── Test 42: Display impl ───────────────────────────────────────

    #[test]
    fn display_impl_formats_correctly() {
        let err = ValidationError {
            field: "routing.tiers[0].name".into(),
            message: "duplicate tier name 'free'".into(),
            severity: ValidationSeverity::Error,
        };
        let displayed = format!("{}", err);
        assert!(displayed.contains("[ERROR]"));
        assert!(displayed.contains("routing.tiers[0].name"));
        assert!(displayed.contains("duplicate tier name 'free'"));

        let warn = ValidationError {
            field: "routing.fallback_model".into(),
            message: "missing provider prefix".into(),
            severity: ValidationSeverity::Warning,
        };
        let displayed = format!("{}", warn);
        assert!(displayed.contains("[WARN]"));
    }

    // ── Test 43: escalation enabled with zero max tiers ─────────────

    #[test]
    fn escalation_enabled_zero_max_tiers() {
        let mut config = valid_tiered_config();
        config.escalation = EscalationConfig {
            enabled: true,
            threshold: 0.6,
            max_escalation_tiers: 0,
        };
        let errors = validate_routing_config(&config);
        assert!(
            has_error(
                &errors,
                "routing.escalation.max_escalation_tiers",
                ValidationSeverity::Error
            ),
            "max_escalation_tiers=0 with enabled escalation should error"
        );
    }

    // ── Test 44: valid fallback model format ────────────────────────

    #[test]
    fn valid_fallback_model_no_warning() {
        let mut config = valid_tiered_config();
        config.fallback_model = Some("groq/llama-3.1-8b".into());
        let errors = validate_routing_config(&config);
        let fb_warnings = errors.iter().any(|e| {
            e.field.contains("fallback_model") && e.severity == ValidationSeverity::Warning
        });
        assert!(!fb_warnings, "valid fallback model should not warn");
    }

    // ── Test 45: workspace within global wildcard tool access ok ─────

    #[test]
    fn workspace_tools_within_global_wildcard_ok() {
        let mut global = valid_tiered_config();
        global.permissions.user.tool_access = Some(vec!["*".into()]);
        let mut workspace = RoutingConfig::default();
        workspace.permissions.user.tool_access =
            Some(vec!["read_file".into(), "exec_shell".into()]);
        let errors = validate_workspace_ceiling(&global, &workspace);
        let tool_errors = errors
            .iter()
            .any(|e| e.field.contains("tool_access") && e.severity == ValidationSeverity::Error);
        assert!(
            !tool_errors,
            "workspace tools under global wildcard should pass"
        );
    }
}
