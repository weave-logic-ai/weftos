//! Environment scoping for WeftOS governance.
//!
//! Environments (development, staging, production) define governance
//! scopes with different risk thresholds, capability sets, audit
//! levels, and learning policies. The same agent identity operates
//! across environments but with capabilities scoped to each
//! environment's governance rules.
//!
//! # Design
//!
//! All types compile unconditionally. Environment-scoped governance
//! enforcement requires the kernel's capability checker and is wired
//! in the boot sequence. The self-learning loop (SONA integration)
//! requires the `ruvector-apps` feature gate.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Unique environment identifier.
pub type EnvironmentId = String;

/// Environment class determines base governance rules.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EnvironmentClass {
    /// Full autonomy. Agents can experiment freely.
    /// Risk threshold: 0.9 (almost anything allowed).
    Development,

    /// Deployed builds, automated testing. Agents test but
    /// do not innovate. Risk threshold: 0.6 (moderate gating).
    Staging,

    /// Live systems. Strong gating, human approval for
    /// high-risk actions. Risk threshold: 0.3 (strict gating).
    Production,

    /// Custom environment with explicit risk threshold.
    Custom {
        /// Custom environment name.
        name: String,
        /// Risk threshold (0.0 = block everything, 1.0 = allow everything).
        risk_threshold: f64,
    },
}

impl EnvironmentClass {
    /// Get the default risk threshold for this environment class.
    pub fn risk_threshold(&self) -> f64 {
        match self {
            EnvironmentClass::Development => 0.9,
            EnvironmentClass::Staging => 0.6,
            EnvironmentClass::Production => 0.3,
            EnvironmentClass::Custom { risk_threshold, .. } => *risk_threshold,
        }
    }
}

impl std::fmt::Display for EnvironmentClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvironmentClass::Development => write!(f, "development"),
            EnvironmentClass::Staging => write!(f, "staging"),
            EnvironmentClass::Production => write!(f, "production"),
            EnvironmentClass::Custom { name, .. } => write!(f, "custom({name})"),
        }
    }
}

/// Governance scope defining rules for an environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceScope {
    /// Risk threshold: actions above this are blocked or escalated.
    #[serde(default = "default_risk_threshold")]
    pub risk_threshold: f64,

    /// Whether human approval is required for actions above threshold.
    #[serde(default)]
    pub human_approval_required: bool,

    /// Which governance branches are active.
    #[serde(default)]
    pub active_branches: GovernanceBranches,

    /// How detailed the audit trail needs to be.
    #[serde(default)]
    pub audit_level: AuditLevel,

    /// SONA learning mode for this environment.
    #[serde(default)]
    pub learning_mode: LearningMode,

    /// Maximum effect vector magnitude before escalation.
    #[serde(default = "default_max_effect")]
    pub max_effect_magnitude: f64,
}

fn default_risk_threshold() -> f64 {
    0.6
}

fn default_max_effect() -> f64 {
    1.0
}

impl Default for GovernanceScope {
    fn default() -> Self {
        Self {
            risk_threshold: default_risk_threshold(),
            human_approval_required: false,
            active_branches: GovernanceBranches::default(),
            audit_level: AuditLevel::default(),
            learning_mode: LearningMode::default(),
            max_effect_magnitude: default_max_effect(),
        }
    }
}

/// Active governance branches (three-branch separation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceBranches {
    /// Legislative: SOP/rule definition.
    #[serde(default = "default_true")]
    pub legislative: bool,

    /// Executive: agent actions.
    #[serde(default = "default_true")]
    pub executive: bool,

    /// Judicial: CGR validation (off in dev for speed).
    #[serde(default)]
    pub judicial: bool,
}

fn default_true() -> bool {
    true
}

impl Default for GovernanceBranches {
    fn default() -> Self {
        Self {
            legislative: true,
            executive: true,
            judicial: false,
        }
    }
}

/// Audit trail detail level.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditLevel {
    /// One summary record per agent session.
    #[default]
    SessionSummary,
    /// One record per action taken.
    PerAction,
    /// Per-action plus full 5D effect vector.
    PerActionWithEffects,
}

/// SONA learning mode.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LearningMode {
    /// High entropy: try novel approaches, learn from failures.
    /// Used in development environments.
    #[default]
    Explore,
    /// Medium entropy: test hypotheses from explore phase.
    /// Used in staging environments.
    Validate,
    /// Low entropy: only use proven patterns.
    /// Used in production environments.
    Exploit,
}

/// An environment definition with its governance scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    /// Unique environment identifier.
    pub id: EnvironmentId,

    /// Human-readable name.
    pub name: String,

    /// Environment class determines base governance rules.
    pub class: EnvironmentClass,

    /// Governance scope (risk thresholds, approval requirements).
    #[serde(default)]
    pub governance: GovernanceScope,

    /// Labels for scheduling and filtering.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Environment management errors.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    /// Environment already exists.
    #[error("environment already exists: '{id}'")]
    AlreadyExists {
        /// Environment ID.
        id: EnvironmentId,
    },

    /// Environment not found.
    #[error("environment not found: '{id}'")]
    NotFound {
        /// Environment ID.
        id: EnvironmentId,
    },

    /// Invalid risk threshold.
    #[error("invalid risk threshold {value}: must be between 0.0 and 1.0")]
    InvalidRiskThreshold {
        /// The invalid value.
        value: f64,
    },

    /// Governance gate denied the operation.
    #[error("governance denied environment operation: {reason}")]
    GovernanceDenied {
        /// Reason for denial.
        reason: String,
    },
}

/// Environment manager.
///
/// Tracks registered environments and the currently active one.
/// Governance enforcement is done by the capability checker using
/// the active environment's governance scope.
pub struct EnvironmentManager {
    environments: DashMap<EnvironmentId, Environment>,
    active: std::sync::RwLock<Option<EnvironmentId>>,
    /// Optional chain manager for exochain audit logging.
    #[cfg(feature = "exochain")]
    chain: Option<Arc<crate::chain::ChainManager>>,
    /// Optional governance gate for policy enforcement.
    #[cfg(feature = "exochain")]
    gate: Option<Arc<crate::gate::GovernanceGate>>,
}

impl EnvironmentManager {
    /// Create a new environment manager.
    pub fn new() -> Self {
        Self {
            environments: DashMap::new(),
            active: std::sync::RwLock::new(None),
            #[cfg(feature = "exochain")]
            chain: None,
            #[cfg(feature = "exochain")]
            gate: None,
        }
    }

    /// Attach a chain manager for audit logging (builder style).
    #[cfg(feature = "exochain")]
    pub fn with_chain(mut self, cm: Arc<crate::chain::ChainManager>) -> Self {
        self.chain = Some(cm);
        self
    }

    /// Attach a governance gate for policy enforcement (builder style).
    #[cfg(feature = "exochain")]
    pub fn with_gate(mut self, gate: Arc<crate::gate::GovernanceGate>) -> Self {
        self.gate = Some(gate);
        self
    }

    /// Register an environment.
    pub fn register(&self, env: Environment) -> Result<(), EnvironmentError> {
        if env.governance.risk_threshold < 0.0 || env.governance.risk_threshold > 1.0 {
            return Err(EnvironmentError::InvalidRiskThreshold {
                value: env.governance.risk_threshold,
            });
        }

        if self.environments.contains_key(&env.id) {
            return Err(EnvironmentError::AlreadyExists { id: env.id });
        }

        // Chain logging: record environment registration.
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain {
            cm.append(
                "environment",
                crate::chain::EVENT_KIND_ENV_REGISTER,
                Some(serde_json::json!({
                    "id": env.id,
                    "name": env.name,
                    "class": env.class.to_string(),
                    "risk_threshold": env.governance.risk_threshold,
                })),
            );
        }

        debug!(id = %env.id, name = %env.name, class = %env.class, "registering environment");
        self.environments.insert(env.id.clone(), env);
        Ok(())
    }

    /// Set the active environment.
    pub fn set_active(&self, id: &str) -> Result<(), EnvironmentError> {
        if !self.environments.contains_key(id) {
            return Err(EnvironmentError::NotFound { id: id.to_owned() });
        }

        // Governance gate: check policy before switching environment.
        #[cfg(feature = "exochain")]
        if let Some(ref gate) = self.gate {
            use crate::gate::GateBackend;
            let env = self.environments.get(id).unwrap();
            let decision = gate.check(
                "system",
                "env.switch",
                &serde_json::json!({
                    "target_env": id,
                    "class": env.class.to_string(),
                    "effect": { "risk": 0.3, "security": 0.2 },
                }),
            );
            if decision.is_deny() {
                return Err(EnvironmentError::GovernanceDenied {
                    reason: format!("environment switch to '{id}' denied by governance policy"),
                });
            }
        }

        // Chain logging: record environment switch.
        #[cfg(feature = "exochain")]
        if let Some(ref cm) = self.chain {
            let prev = self.active_id();
            cm.append(
                "environment",
                crate::chain::EVENT_KIND_ENV_SWITCH,
                Some(serde_json::json!({
                    "id": id,
                    "previous": prev,
                })),
            );
        }

        let mut active = self.active.write().unwrap();
        *active = Some(id.to_owned());
        Ok(())
    }

    /// Get the active environment ID.
    pub fn active_id(&self) -> Option<EnvironmentId> {
        self.active.read().unwrap().clone()
    }

    /// Get the active environment.
    pub fn active(&self) -> Option<Environment> {
        let id = self.active_id()?;
        self.environments.get(&id).map(|e| e.value().clone())
    }

    /// Get an environment by ID.
    pub fn get(&self, id: &str) -> Option<Environment> {
        self.environments.get(id).map(|e| e.value().clone())
    }

    /// List all environments.
    pub fn list(&self) -> Vec<(EnvironmentId, EnvironmentClass, bool)> {
        let active_id = self.active_id();
        self.environments
            .iter()
            .map(|e| {
                let is_active = active_id.as_deref() == Some(e.key().as_str());
                (e.key().clone(), e.class.clone(), is_active)
            })
            .collect()
    }

    /// Remove an environment.
    pub fn remove(&self, id: &str) -> Result<Environment, EnvironmentError> {
        // Cannot remove the active environment
        if self.active_id().as_deref() == Some(id) {
            // Deactivate first
            let mut active = self.active.write().unwrap();
            *active = None;
        }

        let result = self
            .environments
            .remove(id)
            .map(|(_, env)| env)
            .ok_or_else(|| EnvironmentError::NotFound { id: id.to_owned() });

        // Chain logging: record environment removal on success.
        #[cfg(feature = "exochain")]
        if let (Ok(env), Some(cm)) = (&result, &self.chain) {
            cm.append(
                "environment",
                crate::chain::EVENT_KIND_ENV_REMOVE,
                Some(serde_json::json!({
                    "id": env.id,
                    "name": env.name,
                    "class": env.class.to_string(),
                })),
            );
        }

        result
    }

    /// Count environments.
    pub fn len(&self) -> usize {
        self.environments.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.environments.is_empty()
    }

    /// Create a standard set of environments (dev, staging, prod).
    pub fn create_standard_set(&self) -> Result<(), EnvironmentError> {
        self.register(Environment {
            id: "dev".into(),
            name: "Development".into(),
            class: EnvironmentClass::Development,
            governance: GovernanceScope {
                risk_threshold: 0.9,
                human_approval_required: false,
                active_branches: GovernanceBranches {
                    legislative: true,
                    executive: true,
                    judicial: false,
                },
                audit_level: AuditLevel::SessionSummary,
                learning_mode: LearningMode::Explore,
                max_effect_magnitude: 2.0,
            },
            labels: HashMap::new(),
        })?;

        self.register(Environment {
            id: "staging".into(),
            name: "Staging".into(),
            class: EnvironmentClass::Staging,
            governance: GovernanceScope {
                risk_threshold: 0.6,
                human_approval_required: false,
                active_branches: GovernanceBranches {
                    legislative: true,
                    executive: true,
                    judicial: true,
                },
                audit_level: AuditLevel::PerAction,
                learning_mode: LearningMode::Validate,
                max_effect_magnitude: 1.0,
            },
            labels: HashMap::new(),
        })?;

        self.register(Environment {
            id: "prod".into(),
            name: "Production".into(),
            class: EnvironmentClass::Production,
            governance: GovernanceScope {
                risk_threshold: 0.3,
                human_approval_required: true,
                active_branches: GovernanceBranches {
                    legislative: true,
                    executive: true,
                    judicial: true,
                },
                audit_level: AuditLevel::PerActionWithEffects,
                learning_mode: LearningMode::Exploit,
                max_effect_magnitude: 0.5,
            },
            labels: HashMap::new(),
        })?;

        Ok(())
    }
}

impl Default for EnvironmentManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_dev_env() -> Environment {
        Environment {
            id: "dev".into(),
            name: "Development".into(),
            class: EnvironmentClass::Development,
            governance: GovernanceScope {
                risk_threshold: 0.9,
                ..Default::default()
            },
            labels: HashMap::new(),
        }
    }

    #[test]
    fn environment_class_risk_threshold() {
        assert!((EnvironmentClass::Development.risk_threshold() - 0.9).abs() < f64::EPSILON);
        assert!((EnvironmentClass::Staging.risk_threshold() - 0.6).abs() < f64::EPSILON);
        assert!((EnvironmentClass::Production.risk_threshold() - 0.3).abs() < f64::EPSILON);
        let custom = EnvironmentClass::Custom {
            name: "test".into(),
            risk_threshold: 0.75,
        };
        assert!((custom.risk_threshold() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn environment_class_display() {
        assert_eq!(EnvironmentClass::Development.to_string(), "development");
        assert_eq!(EnvironmentClass::Production.to_string(), "production");
        assert_eq!(
            EnvironmentClass::Custom {
                name: "qa".into(),
                risk_threshold: 0.5
            }
            .to_string(),
            "custom(qa)"
        );
    }

    #[test]
    fn governance_scope_default() {
        let scope = GovernanceScope::default();
        assert!((scope.risk_threshold - 0.6).abs() < f64::EPSILON);
        assert!(!scope.human_approval_required);
        assert!(scope.active_branches.legislative);
        assert!(scope.active_branches.executive);
        assert!(!scope.active_branches.judicial);
    }

    #[test]
    fn governance_scope_serde_roundtrip() {
        let scope = GovernanceScope {
            risk_threshold: 0.3,
            human_approval_required: true,
            active_branches: GovernanceBranches {
                legislative: true,
                executive: true,
                judicial: true,
            },
            audit_level: AuditLevel::PerActionWithEffects,
            learning_mode: LearningMode::Exploit,
            max_effect_magnitude: 0.5,
        };
        let json = serde_json::to_string(&scope).unwrap();
        let restored: GovernanceScope = serde_json::from_str(&json).unwrap();
        assert!((restored.risk_threshold - 0.3).abs() < f64::EPSILON);
        assert!(restored.human_approval_required);
        assert_eq!(restored.learning_mode, LearningMode::Exploit);
    }

    #[test]
    fn register_and_list() {
        let mgr = EnvironmentManager::new();
        mgr.register(make_dev_env()).unwrap();
        let list = mgr.list();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn register_duplicate_fails() {
        let mgr = EnvironmentManager::new();
        mgr.register(make_dev_env()).unwrap();
        assert!(matches!(
            mgr.register(make_dev_env()),
            Err(EnvironmentError::AlreadyExists { .. })
        ));
    }

    #[test]
    fn invalid_risk_threshold() {
        let mgr = EnvironmentManager::new();
        let mut env = make_dev_env();
        env.governance.risk_threshold = 1.5;
        assert!(matches!(
            mgr.register(env),
            Err(EnvironmentError::InvalidRiskThreshold { .. })
        ));
    }

    #[test]
    fn set_active() {
        let mgr = EnvironmentManager::new();
        mgr.register(make_dev_env()).unwrap();
        mgr.set_active("dev").unwrap();
        assert_eq!(mgr.active_id().as_deref(), Some("dev"));
        let active = mgr.active().unwrap();
        assert_eq!(active.name, "Development");
    }

    #[test]
    fn set_active_nonexistent_fails() {
        let mgr = EnvironmentManager::new();
        assert!(matches!(
            mgr.set_active("nope"),
            Err(EnvironmentError::NotFound { .. })
        ));
    }

    #[test]
    fn remove_environment() {
        let mgr = EnvironmentManager::new();
        mgr.register(make_dev_env()).unwrap();
        let removed = mgr.remove("dev").unwrap();
        assert_eq!(removed.name, "Development");
        assert!(mgr.is_empty());
    }

    #[test]
    fn remove_active_clears_active() {
        let mgr = EnvironmentManager::new();
        mgr.register(make_dev_env()).unwrap();
        mgr.set_active("dev").unwrap();
        mgr.remove("dev").unwrap();
        assert!(mgr.active_id().is_none());
    }

    #[test]
    fn create_standard_set() {
        let mgr = EnvironmentManager::new();
        mgr.create_standard_set().unwrap();
        assert_eq!(mgr.len(), 3);

        let dev = mgr.get("dev").unwrap();
        assert!((dev.governance.risk_threshold - 0.9).abs() < f64::EPSILON);
        assert_eq!(dev.governance.learning_mode, LearningMode::Explore);

        let prod = mgr.get("prod").unwrap();
        assert!((prod.governance.risk_threshold - 0.3).abs() < f64::EPSILON);
        assert!(prod.governance.human_approval_required);
        assert_eq!(prod.governance.learning_mode, LearningMode::Exploit);
    }

    #[test]
    fn environment_serde_roundtrip() {
        let env = make_dev_env();
        let json = serde_json::to_string(&env).unwrap();
        let restored: Environment = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, "dev");
        assert_eq!(restored.class, EnvironmentClass::Development);
    }

    #[test]
    fn environment_error_display() {
        let err = EnvironmentError::NotFound { id: "prod".into() };
        assert!(err.to_string().contains("prod"));

        let err = EnvironmentError::InvalidRiskThreshold { value: 1.5 };
        assert!(err.to_string().contains("1.5"));
    }

    #[test]
    fn audit_level_default() {
        assert_eq!(AuditLevel::default(), AuditLevel::SessionSummary);
    }

    #[test]
    fn learning_mode_default() {
        assert_eq!(LearningMode::default(), LearningMode::Explore);
    }
}
