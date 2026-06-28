//! Permission resolution for the tiered router.
//!
//! Implements the 5-layer permission resolution algorithm (design doc
//! Section 3.2). All types are imported from `clawft_types::routing`.
//!
//! **This is the ONLY permission resolution implementation.** Phase C
//! and Phase F must NOT re-implement resolution or merge logic.

use std::collections::HashMap;

use tracing::debug;

use clawft_types::routing::{
    AuthContext, PermissionLevelConfig, PermissionsConfig, RoutingConfig, UserPermissions,
};

// ── Named constructors ──────────────────────────────────────────────────

/// Built-in defaults for Level 0 (zero_trust). Maximally restrictive.
pub fn zero_trust_defaults() -> UserPermissions {
    UserPermissions {
        level: 0,
        max_tier: "free".into(),
        model_access: vec![],
        model_denylist: vec![],
        tool_access: vec![],
        tool_denylist: vec![],
        max_context_tokens: 4096,
        max_output_tokens: 1024,
        rate_limit: 10,
        streaming_allowed: false,
        escalation_allowed: false,
        escalation_threshold: 1.0,
        model_override: false,
        cost_budget_daily_usd: 0.10,
        cost_budget_monthly_usd: 2.00,
        custom_permissions: HashMap::new(),
    }
}

/// Built-in defaults for Level 1 (user). Moderate access.
pub fn user_defaults() -> UserPermissions {
    UserPermissions {
        level: 1,
        max_tier: "standard".into(),
        model_access: vec![],
        model_denylist: vec![],
        tool_access: vec![
            "read_file".into(),
            "write_file".into(),
            "edit_file".into(),
            "list_dir".into(),
            "web_search".into(),
            "web_fetch".into(),
            "message".into(),
        ],
        tool_denylist: vec![],
        max_context_tokens: 16384,
        max_output_tokens: 4096,
        rate_limit: 60,
        streaming_allowed: true,
        escalation_allowed: true,
        escalation_threshold: 0.6,
        model_override: false,
        cost_budget_daily_usd: 5.00,
        cost_budget_monthly_usd: 100.00,
        custom_permissions: HashMap::new(),
    }
}

/// Built-in defaults for Level 2 (admin). Full access.
pub fn admin_defaults() -> UserPermissions {
    UserPermissions {
        level: 2,
        max_tier: "elite".into(),
        model_access: vec![],
        model_denylist: vec![],
        tool_access: vec!["*".into()],
        tool_denylist: vec![],
        max_context_tokens: 200_000,
        max_output_tokens: 16384,
        rate_limit: 0,
        streaming_allowed: true,
        escalation_allowed: true,
        escalation_threshold: 0.0,
        model_override: true,
        cost_budget_daily_usd: 0.0,
        cost_budget_monthly_usd: 0.0,
        custom_permissions: HashMap::new(),
    }
}

/// Return built-in defaults for a numeric level.
///
/// Levels 0, 1, 2 map to zero_trust, user, admin respectively.
/// Levels >= 3 are treated as admin (highest built-in level) to avoid
/// silently downgrading high-privilege configs to zero_trust.
pub fn defaults_for_level(level: u8) -> UserPermissions {
    match level {
        0 => zero_trust_defaults(),
        1 => user_defaults(),
        _ => {
            let mut perms = admin_defaults();
            perms.level = level;
            perms
        }
    }
}

/// Convert a numeric level to its config lookup name.
///
/// Levels >= 2 map to "admin" (highest built-in level name).
pub fn level_name(level: u8) -> &'static str {
    match level {
        0 => "zero_trust",
        1 => "user",
        _ => "admin",
    }
}

/// Convert a level name to its numeric value. `None` for unknown.
pub fn level_from_name(name: &str) -> Option<u8> {
    match name {
        "zero_trust" => Some(0),
        "user" => Some(1),
        "admin" => Some(2),
        _ => None,
    }
}

// ── Merge logic ─────────────────────────────────────────────────────────

/// Merge overrides into `base`. Only `Some` fields replace base values.
/// Vec fields: non-empty replaces base; empty means "no change".
/// custom_permissions: shallow key-level merge.
pub fn merge_permissions(base: &mut UserPermissions, ov: &PermissionLevelConfig) {
    if let Some(v) = ov.level {
        base.level = v;
    }
    if let Some(ref v) = ov.max_tier {
        base.max_tier = v.clone();
    }
    if let Some(ref v) = ov.model_access
        && !v.is_empty()
    {
        base.model_access = v.clone();
    }
    if let Some(ref v) = ov.model_denylist
        && !v.is_empty()
    {
        base.model_denylist = v.clone();
    }
    if let Some(ref v) = ov.tool_access
        && !v.is_empty()
    {
        base.tool_access = v.clone();
    }
    if let Some(ref v) = ov.tool_denylist
        && !v.is_empty()
    {
        base.tool_denylist = v.clone();
    }
    if let Some(v) = ov.max_context_tokens {
        base.max_context_tokens = v;
    }
    if let Some(v) = ov.max_output_tokens {
        base.max_output_tokens = v;
    }
    if let Some(v) = ov.rate_limit {
        base.rate_limit = v;
    }
    if let Some(v) = ov.streaming_allowed {
        base.streaming_allowed = v;
    }
    if let Some(v) = ov.escalation_allowed {
        base.escalation_allowed = v;
    }
    if let Some(v) = ov.escalation_threshold {
        base.escalation_threshold = v;
    }
    if let Some(v) = ov.model_override {
        base.model_override = v;
    }
    if let Some(v) = ov.cost_budget_daily_usd {
        base.cost_budget_daily_usd = v;
    }
    if let Some(v) = ov.cost_budget_monthly_usd {
        base.cost_budget_monthly_usd = v;
    }
    if let Some(ref custom) = ov.custom_permissions {
        for (k, v) in custom {
            base.custom_permissions.insert(k.clone(), v.clone());
        }
    }
}

// ── Internal helpers ────────────────────────────────────────────────────

fn extract_level_overrides(p: &PermissionsConfig) -> HashMap<String, PermissionLevelConfig> {
    let mut m = HashMap::new();
    if has_any_field(&p.zero_trust) {
        m.insert("zero_trust".into(), p.zero_trust.clone());
    }
    if has_any_field(&p.user) {
        m.insert("user".into(), p.user.clone());
    }
    if has_any_field(&p.admin) {
        m.insert("admin".into(), p.admin.clone());
    }
    m
}

fn has_any_field(c: &PermissionLevelConfig) -> bool {
    c.level.is_some()
        || c.max_tier.is_some()
        || c.model_access.is_some()
        || c.model_denylist.is_some()
        || c.tool_access.is_some()
        || c.tool_denylist.is_some()
        || c.max_context_tokens.is_some()
        || c.max_output_tokens.is_some()
        || c.rate_limit.is_some()
        || c.streaming_allowed.is_some()
        || c.escalation_allowed.is_some()
        || c.escalation_threshold.is_some()
        || c.model_override.is_some()
        || c.cost_budget_daily_usd.is_some()
        || c.cost_budget_monthly_usd.is_some()
        || c.custom_permissions.is_some()
}

/// Tier name -> numeric rank. free(0) < standard(1) < premium(2) < elite(3).
fn tier_rank(tier: &str) -> u8 {
    match tier {
        "free" => 0,
        "standard" => 1,
        "premium" => 2,
        "elite" => 3,
        _ => 0,
    }
}

// ── PermissionResolver ──────────────────────────────────────────────────

/// The single, authoritative permission resolution implementation.
///
/// Holds pre-extracted config layers from global and workspace configs
/// for the 5-layer resolution algorithm. Workspace ceiling enforcement
/// prevents workspace configs from expanding beyond global permissions.
pub struct PermissionResolver {
    global_level_overrides: HashMap<String, PermissionLevelConfig>,
    workspace_level_overrides: HashMap<String, PermissionLevelConfig>,
    user_overrides: HashMap<String, PermissionLevelConfig>,
    channel_overrides: HashMap<String, PermissionLevelConfig>,
    cli_default_level: u8,
}

impl PermissionResolver {
    /// Create from a global `RoutingConfig` and optional workspace config.
    pub fn new(global: &RoutingConfig, workspace: Option<&RoutingConfig>) -> Self {
        Self {
            global_level_overrides: extract_level_overrides(&global.permissions),
            workspace_level_overrides: workspace
                .map(|ws| extract_level_overrides(&ws.permissions))
                .unwrap_or_default(),
            user_overrides: global.permissions.users.clone(),
            channel_overrides: global.permissions.channels.clone(),
            cli_default_level: 2,
        }
    }

    /// Minimal resolver: CLI = admin, everything else = zero_trust.
    pub fn default_resolver() -> Self {
        Self {
            global_level_overrides: HashMap::new(),
            workspace_level_overrides: HashMap::new(),
            user_overrides: HashMap::new(),
            channel_overrides: HashMap::new(),
            cli_default_level: 2,
        }
    }

    /// Resolve effective permissions for a sender on a channel.
    ///
    /// 5-layer resolution: built-in -> global -> workspace -> user -> channel.
    /// `allow_from_match`: whether the channel plugin confirmed sender
    /// is in the channel's allow_from list.
    pub fn resolve(
        &self,
        sender_id: &str,
        channel: &str,
        allow_from_match: bool,
    ) -> UserPermissions {
        let level = self.determine_level(sender_id, channel, allow_from_match);
        let mut perms = defaults_for_level(level);
        let lname = level_name(level);

        if let Some(g) = self.global_level_overrides.get(lname) {
            merge_permissions(&mut perms, g);
        }
        if let Some(w) = self.workspace_level_overrides.get(lname) {
            merge_permissions(&mut perms, w);
        }
        if let Some(u) = self.user_overrides.get(sender_id) {
            merge_permissions(&mut perms, u);
        }
        if let Some(c) = self.channel_overrides.get(channel) {
            merge_permissions(&mut perms, c);
        }

        if !self.workspace_level_overrides.is_empty() {
            let ceiling = self.resolve_global_only(sender_id, channel, allow_from_match);
            Self::enforce_workspace_ceiling(&mut perms, &ceiling);
        }

        debug!(
            sender_id = %sender_id,
            channel = %channel,
            resolved_level = perms.level,
            level_name = lname,
            tool_count = perms.tool_access.len(),
            has_wildcard = perms.tool_access.iter().any(|t| t == "*"),
            "permissions resolved"
        );

        perms
    }

    /// Resolve permissions and wrap in an `AuthContext`.
    pub fn resolve_auth_context(
        &self,
        sender_id: &str,
        channel: &str,
        allow_from_match: bool,
    ) -> AuthContext {
        AuthContext {
            sender_id: sender_id.to_string(),
            channel: channel.to_string(),
            permissions: self.resolve(sender_id, channel, allow_from_match),
        }
    }

    /// Determine level: user override > channel override > allow_from > cli > zero_trust.
    fn determine_level(&self, sender_id: &str, channel: &str, allow_from_match: bool) -> u8 {
        if let Some(u) = self.user_overrides.get(sender_id)
            && let Some(level) = u.level
        {
            debug!(sender_id = %sender_id, level, source = "user_override", "permission level determined");
            return level;
        }
        if let Some(c) = self.channel_overrides.get(channel)
            && let Some(level) = c.level
        {
            debug!(sender_id = %sender_id, channel = %channel, level, source = "channel_override", "permission level determined");
            return level;
        }
        if allow_from_match {
            debug!(sender_id = %sender_id, channel = %channel, level = 1u8, source = "allow_from", "permission level determined");
            return 1;
        }
        if channel == "cli" {
            debug!(sender_id = %sender_id, level = self.cli_default_level, source = "cli_default", "permission level determined");
            return self.cli_default_level;
        }
        debug!(sender_id = %sender_id, channel = %channel, level = 0u8, source = "fallback_zero_trust", "permission level determined");
        0
    }

    /// Resolve using only global config (no workspace). For ceiling comparison.
    fn resolve_global_only(&self, sender_id: &str, channel: &str, afm: bool) -> UserPermissions {
        let level = self.determine_level(sender_id, channel, afm);
        let mut perms = defaults_for_level(level);
        let lname = level_name(level);
        if let Some(g) = self.global_level_overrides.get(lname) {
            merge_permissions(&mut perms, g);
        }
        if let Some(u) = self.user_overrides.get(sender_id) {
            merge_permissions(&mut perms, u);
        }
        if let Some(c) = self.channel_overrides.get(channel) {
            merge_permissions(&mut perms, c);
        }
        perms
    }

    /// Clamp workspace permissions to global ceiling for security fields.
    fn enforce_workspace_ceiling(perms: &mut UserPermissions, ceil: &UserPermissions) {
        if perms.level > ceil.level {
            perms.level = ceil.level;
        }
        if !ceil.escalation_allowed {
            perms.escalation_allowed = false;
        }
        if !ceil.tool_access.contains(&"*".to_string()) {
            perms
                .tool_access
                .retain(|t| t == "*" || ceil.tool_access.contains(t));
            if perms.tool_access.contains(&"*".to_string()) {
                perms.tool_access = ceil.tool_access.clone();
            }
        }
        if ceil.rate_limit > 0 && (perms.rate_limit == 0 || perms.rate_limit > ceil.rate_limit) {
            perms.rate_limit = ceil.rate_limit;
        }
        if ceil.cost_budget_daily_usd > 0.0
            && (perms.cost_budget_daily_usd == 0.0
                || perms.cost_budget_daily_usd > ceil.cost_budget_daily_usd)
        {
            perms.cost_budget_daily_usd = ceil.cost_budget_daily_usd;
        }
        if ceil.cost_budget_monthly_usd > 0.0
            && (perms.cost_budget_monthly_usd == 0.0
                || perms.cost_budget_monthly_usd > ceil.cost_budget_monthly_usd)
        {
            perms.cost_budget_monthly_usd = ceil.cost_budget_monthly_usd;
        }
        if tier_rank(&perms.max_tier) > tier_rank(&ceil.max_tier) {
            perms.max_tier = ceil.max_tier.clone();
        }
    }

    /// Static validation: check if workspace expands beyond global ceiling.
    pub fn validate_workspace_ceiling(
        global: &RoutingConfig,
        workspace: &RoutingConfig,
    ) -> Vec<String> {
        let mut violations = Vec::new();
        for ln in &["zero_trust", "user", "admin"] {
            let g = match *ln {
                "zero_trust" => &global.permissions.zero_trust,
                "user" => &global.permissions.user,
                _ => &global.permissions.admin,
            };
            let w = match *ln {
                "zero_trust" => &workspace.permissions.zero_trust,
                "user" => &workspace.permissions.user,
                _ => &workspace.permissions.admin,
            };
            if let Some(wl) = w.level {
                let gl = g.level.unwrap_or(level_from_name(ln).unwrap_or(0));
                if wl > gl {
                    violations.push(format!(
                        "Workspace {ln}: level {wl} exceeds global ceiling {gl}"
                    ));
                }
            }
            if let Some(true) = w.escalation_allowed
                && let Some(false) = g.escalation_allowed
            {
                violations.push(format!(
                    "Workspace {ln}: escalation_allowed=true exceeds global ceiling (false)"
                ));
            }
        }
        violations
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn plc() -> PermissionLevelConfig {
        PermissionLevelConfig::default()
    }

    fn rcfg(perms: PermissionsConfig) -> RoutingConfig {
        RoutingConfig {
            permissions: perms,
            ..RoutingConfig::default()
        }
    }

    // -- Built-in Defaults (3) --

    #[test]
    fn test_zero_trust_defaults() {
        let p = zero_trust_defaults();
        assert_eq!(p.level, 0);
        assert_eq!(p.max_tier, "free");
        assert!(p.model_access.is_empty());
        assert!(p.model_denylist.is_empty());
        assert!(p.tool_access.is_empty());
        assert!(p.tool_denylist.is_empty());
        assert_eq!(p.max_context_tokens, 4096);
        assert_eq!(p.max_output_tokens, 1024);
        assert_eq!(p.rate_limit, 10);
        assert!(!p.streaming_allowed);
        assert!(!p.escalation_allowed);
        assert!((p.escalation_threshold - 1.0).abs() < f32::EPSILON);
        assert!(!p.model_override);
        assert!((p.cost_budget_daily_usd - 0.10).abs() < f64::EPSILON);
        assert!((p.cost_budget_monthly_usd - 2.00).abs() < f64::EPSILON);
        assert!(p.custom_permissions.is_empty());
    }

    #[test]
    fn test_user_defaults() {
        let p = user_defaults();
        assert_eq!(p.level, 1);
        assert_eq!(p.max_tier, "standard");
        assert_eq!(p.tool_access.len(), 7);
        assert!(p.tool_access.contains(&"read_file".to_string()));
        assert!(p.tool_access.contains(&"message".to_string()));
        assert_eq!(p.max_context_tokens, 16384);
        assert_eq!(p.max_output_tokens, 4096);
        assert_eq!(p.rate_limit, 60);
        assert!(p.streaming_allowed);
        assert!(p.escalation_allowed);
        assert!((p.escalation_threshold - 0.6).abs() < f32::EPSILON);
        assert!(!p.model_override);
        assert!((p.cost_budget_daily_usd - 5.00).abs() < f64::EPSILON);
        assert!((p.cost_budget_monthly_usd - 100.00).abs() < f64::EPSILON);
    }

    #[test]
    fn test_admin_defaults() {
        let p = admin_defaults();
        assert_eq!(p.level, 2);
        assert_eq!(p.max_tier, "elite");
        assert_eq!(p.tool_access, vec!["*"]);
        assert_eq!(p.max_context_tokens, 200_000);
        assert_eq!(p.max_output_tokens, 16384);
        assert_eq!(p.rate_limit, 0);
        assert!(p.streaming_allowed);
        assert!(p.escalation_allowed);
        assert!((p.escalation_threshold - 0.0).abs() < f32::EPSILON);
        assert!(p.model_override);
        assert_eq!(p.cost_budget_daily_usd, 0.0);
        assert_eq!(p.cost_budget_monthly_usd, 0.0);
    }

    // -- Level Resolution (5) --

    #[test]
    fn test_determine_level_per_user_override() {
        let mut users = HashMap::new();
        users.insert(
            "alice".into(),
            PermissionLevelConfig {
                level: Some(2),
                ..plc()
            },
        );
        let r = PermissionResolver::new(
            &rcfg(PermissionsConfig {
                users,
                ..Default::default()
            }),
            None,
        );
        assert_eq!(r.determine_level("alice", "telegram", false), 2);
    }

    #[test]
    fn test_determine_level_per_channel_override() {
        let mut ch = HashMap::new();
        ch.insert(
            "discord".into(),
            PermissionLevelConfig {
                level: Some(0),
                ..plc()
            },
        );
        let r = PermissionResolver::new(
            &rcfg(PermissionsConfig {
                channels: ch,
                ..Default::default()
            }),
            None,
        );
        assert_eq!(r.determine_level("unknown", "discord", false), 0);
    }

    #[test]
    fn test_determine_level_allow_from_match() {
        let r = PermissionResolver::default_resolver();
        assert_eq!(r.determine_level("someone", "telegram", true), 1);
        assert_eq!(r.determine_level("someone", "telegram", false), 0);
    }

    #[test]
    fn test_determine_level_cli_default() {
        let r = PermissionResolver::default_resolver();
        assert_eq!(r.determine_level("local", "cli", false), 2);
    }

    #[test]
    fn test_determine_level_unknown_sender() {
        let r = PermissionResolver::default_resolver();
        assert_eq!(r.determine_level("nobody", "telegram", false), 0);
    }

    // -- Merge (4) --

    #[test]
    fn test_merge_scalar_fields() {
        let mut base = zero_trust_defaults();
        merge_permissions(
            &mut base,
            &PermissionLevelConfig {
                max_tier: Some("premium".into()),
                rate_limit: Some(30),
                ..plc()
            },
        );
        assert_eq!(base.max_tier, "premium");
        assert_eq!(base.rate_limit, 30);
        assert_eq!(base.level, 0);
        assert_eq!(base.max_context_tokens, 4096);
    }

    #[test]
    fn test_merge_vec_fields_non_empty() {
        let mut base = user_defaults();
        merge_permissions(
            &mut base,
            &PermissionLevelConfig {
                tool_access: Some(vec!["custom_tool".into()]),
                ..plc()
            },
        );
        assert_eq!(base.tool_access, vec!["custom_tool"]);
    }

    #[test]
    fn test_merge_vec_fields_empty_no_change() {
        let mut base = user_defaults();
        let orig = base.tool_access.clone();
        merge_permissions(
            &mut base,
            &PermissionLevelConfig {
                tool_access: Some(vec![]),
                ..plc()
            },
        );
        assert_eq!(base.tool_access, orig);
    }

    #[test]
    fn test_merge_custom_permissions() {
        let mut base = zero_trust_defaults();
        base.custom_permissions
            .insert("existing".into(), serde_json::json!("base"));
        let mut custom = HashMap::new();
        custom.insert("new_key".into(), serde_json::json!(42));
        custom.insert("existing".into(), serde_json::json!("overridden"));
        merge_permissions(
            &mut base,
            &PermissionLevelConfig {
                custom_permissions: Some(custom),
                ..plc()
            },
        );
        assert_eq!(
            base.custom_permissions["existing"],
            serde_json::json!("overridden")
        );
        assert_eq!(base.custom_permissions["new_key"], serde_json::json!(42));
    }

    // -- Full Resolution (4) --

    #[test]
    fn test_resolve_full_stack() {
        let mut users = HashMap::new();
        users.insert(
            "alice".into(),
            PermissionLevelConfig {
                level: Some(1),
                cost_budget_daily_usd: Some(10.0),
                max_tier: Some("premium".into()),
                ..plc()
            },
        );
        let mut ch = HashMap::new();
        ch.insert(
            "discord".into(),
            PermissionLevelConfig {
                max_tier: Some("standard".into()),
                rate_limit: Some(30),
                ..plc()
            },
        );
        let cfg = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                max_output_tokens: Some(8192),
                ..plc()
            },
            users,
            channels: ch,
            ..Default::default()
        });
        let perms = PermissionResolver::new(&cfg, None).resolve("alice", "discord", false);
        assert_eq!(perms.level, 1);
        assert_eq!(perms.max_tier, "standard"); // channel > user
        assert_eq!(perms.max_output_tokens, 8192);
        assert!((perms.cost_budget_daily_usd - 10.0).abs() < f64::EPSILON);
        assert_eq!(perms.rate_limit, 30);
    }

    #[test]
    fn test_resolve_cli_always_admin() {
        let perms = PermissionResolver::default_resolver().resolve("local", "cli", false);
        assert_eq!(perms.level, 2);
        assert_eq!(perms.max_tier, "elite");
        assert_eq!(perms.tool_access, vec!["*"]);
        assert_eq!(perms.rate_limit, 0);
        assert!(perms.model_override);
    }

    #[test]
    fn test_resolve_zero_trust_enforcement() {
        let perms = PermissionResolver::default_resolver().resolve("stranger", "telegram", false);
        assert_eq!(perms.level, 0);
        assert_eq!(perms.max_tier, "free");
        assert!(perms.tool_access.is_empty());
        assert!(!perms.escalation_allowed);
        assert!((perms.cost_budget_daily_usd - 0.10).abs() < f64::EPSILON);
    }

    #[test]
    fn test_resolve_per_user_budget_override() {
        let mut users = HashMap::new();
        users.insert(
            "bob".into(),
            PermissionLevelConfig {
                level: Some(1),
                cost_budget_daily_usd: Some(2.50),
                ..plc()
            },
        );
        let perms = PermissionResolver::new(
            &rcfg(PermissionsConfig {
                users,
                ..Default::default()
            }),
            None,
        )
        .resolve("bob", "telegram", false);
        assert_eq!(perms.level, 1);
        assert!((perms.cost_budget_daily_usd - 2.50).abs() < f64::EPSILON);
        assert!((perms.cost_budget_monthly_usd - 100.00).abs() < f64::EPSILON);
    }

    // -- Constructor (1) --

    #[test]
    fn test_resolver_constructed_from_routing_config() {
        let mut users = HashMap::new();
        users.insert(
            "tu".into(),
            PermissionLevelConfig {
                level: Some(2),
                ..plc()
            },
        );
        let mut ch = HashMap::new();
        ch.insert(
            "tc".into(),
            PermissionLevelConfig {
                rate_limit: Some(5),
                ..plc()
            },
        );
        let cfg = rcfg(PermissionsConfig {
            zero_trust: PermissionLevelConfig {
                max_output_tokens: Some(512),
                ..plc()
            },
            users,
            channels: ch,
            ..Default::default()
        });
        let perms = PermissionResolver::new(&cfg, None).resolve("tu", "tc", false);
        assert_eq!(perms.level, 2);
        assert_eq!(perms.rate_limit, 5);
    }

    // -- Workspace Ceiling (3) --

    #[test]
    fn test_workspace_ceiling_level_clamped() {
        let g = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                level: Some(1),
                ..plc()
            },
            ..Default::default()
        });
        let w = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                level: Some(2),
                ..plc()
            },
            ..Default::default()
        });
        let perms = PermissionResolver::new(&g, Some(&w)).resolve("someone", "telegram", true);
        assert!(perms.level <= 1);
    }

    #[test]
    fn test_workspace_ceiling_tool_access_filtered() {
        let g = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                tool_access: Some(vec!["read_file".into(), "web_search".into()]),
                ..plc()
            },
            ..Default::default()
        });
        let w = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                tool_access: Some(vec![
                    "read_file".into(),
                    "web_search".into(),
                    "dangerous".into(),
                ]),
                ..plc()
            },
            ..Default::default()
        });
        let perms = PermissionResolver::new(&g, Some(&w)).resolve("someone", "telegram", true);
        assert!(!perms.tool_access.contains(&"dangerous".to_string()));
        assert!(perms.tool_access.contains(&"read_file".to_string()));
    }

    #[test]
    fn test_workspace_ceiling_budget_clamped() {
        let g = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                cost_budget_daily_usd: Some(5.0),
                cost_budget_monthly_usd: Some(50.0),
                ..plc()
            },
            ..Default::default()
        });
        let w = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                cost_budget_daily_usd: Some(0.0),
                cost_budget_monthly_usd: Some(999.0),
                ..plc()
            },
            ..Default::default()
        });
        let perms = PermissionResolver::new(&g, Some(&w)).resolve("someone", "telegram", true);
        assert!(perms.cost_budget_daily_usd <= 5.0 && perms.cost_budget_daily_usd > 0.0);
        assert!(perms.cost_budget_monthly_usd <= 50.0 && perms.cost_budget_monthly_usd > 0.0);
    }

    // -- Edge Cases (2) --

    #[test]
    fn test_unknown_level_gets_admin_with_original_level() {
        let p = defaults_for_level(99);
        // Levels >= 2 get admin defaults but retain the original level value.
        assert_eq!(p.level, 99);
        assert_eq!(p.max_tier, "elite");
        assert!(p.tool_access.iter().any(|t| t == "*"));
        assert!(p.escalation_allowed);
    }

    #[test]
    fn test_auth_context_default_is_zero_trust() {
        let ctx = AuthContext::default();
        assert!(ctx.sender_id.is_empty());
        assert!(ctx.channel.is_empty());
        assert_eq!(ctx.permissions.level, 0);
        assert!((ctx.permissions.cost_budget_daily_usd - 0.10).abs() < f64::EPSILON);
    }

    // -- Additional Coverage --

    #[test]
    fn test_channel_overrides_beat_user_overrides() {
        let mut users = HashMap::new();
        users.insert(
            "alice".into(),
            PermissionLevelConfig {
                level: Some(1),
                max_tier: Some("premium".into()),
                ..plc()
            },
        );
        let mut ch = HashMap::new();
        ch.insert(
            "restricted".into(),
            PermissionLevelConfig {
                max_tier: Some("free".into()),
                streaming_allowed: Some(false),
                ..plc()
            },
        );
        let perms = PermissionResolver::new(
            &rcfg(PermissionsConfig {
                users,
                channels: ch,
                ..Default::default()
            }),
            None,
        )
        .resolve("alice", "restricted", false);
        assert_eq!(perms.max_tier, "free");
        assert!(!perms.streaming_allowed);
    }

    #[test]
    fn test_partial_override_only_affects_specified_fields() {
        let mut users = HashMap::new();
        users.insert(
            "bob".into(),
            PermissionLevelConfig {
                level: Some(1),
                rate_limit: Some(120),
                ..plc()
            },
        );
        let perms = PermissionResolver::new(
            &rcfg(PermissionsConfig {
                users,
                ..Default::default()
            }),
            None,
        )
        .resolve("bob", "telegram", false);
        assert_eq!(perms.rate_limit, 120);
        assert_eq!(perms.max_tier, "standard");
        assert!(perms.streaming_allowed);
        assert_eq!(perms.max_context_tokens, 16384);
    }

    #[test]
    fn test_empty_sender_id_gets_zero_trust() {
        let perms = PermissionResolver::default_resolver().resolve("", "telegram", false);
        assert_eq!(perms.level, 0);
        assert_eq!(perms.max_tier, "free");
    }

    #[test]
    fn test_allow_from_match_promotes_to_user_level() {
        let perms = PermissionResolver::default_resolver().resolve("unknown", "telegram", true);
        assert_eq!(perms.level, 1);
        assert_eq!(perms.max_tier, "standard");
        assert!(perms.streaming_allowed);
    }

    #[test]
    fn test_level_name_mapping() {
        assert_eq!(level_name(0), "zero_trust");
        assert_eq!(level_name(1), "user");
        assert_eq!(level_name(2), "admin");
        assert_eq!(level_name(99), "admin");
    }

    #[test]
    fn test_level_from_name_mapping() {
        assert_eq!(level_from_name("zero_trust"), Some(0));
        assert_eq!(level_from_name("user"), Some(1));
        assert_eq!(level_from_name("admin"), Some(2));
        assert_eq!(level_from_name("unknown"), None);
    }

    #[test]
    fn test_resolve_auth_context() {
        let ctx =
            PermissionResolver::default_resolver().resolve_auth_context("local", "cli", false);
        assert_eq!(ctx.sender_id, "local");
        assert_eq!(ctx.channel, "cli");
        assert_eq!(ctx.permissions.level, 2);
    }

    #[test]
    fn test_validate_workspace_ceiling_detects_violations() {
        let g = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                level: Some(1),
                escalation_allowed: Some(false),
                ..plc()
            },
            ..Default::default()
        });
        let w = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                level: Some(2),
                escalation_allowed: Some(true),
                ..plc()
            },
            ..Default::default()
        });
        let v = PermissionResolver::validate_workspace_ceiling(&g, &w);
        assert!(v.len() >= 2, "expected >= 2 violations, got: {v:?}");
        assert!(v.iter().any(|s| s.contains("level")));
        assert!(v.iter().any(|s| s.contains("escalation_allowed")));
    }

    #[test]
    fn test_validate_workspace_ceiling_clean() {
        let g = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                level: Some(1),
                ..plc()
            },
            ..Default::default()
        });
        let w = rcfg(PermissionsConfig {
            user: PermissionLevelConfig {
                level: Some(0),
                ..plc()
            },
            ..Default::default()
        });
        let v = PermissionResolver::validate_workspace_ceiling(&g, &w);
        assert!(v.is_empty(), "expected no violations, got: {v:?}");
    }
}
