//! Domain trait and registry for pluggable domain configurations.
//!
//! Domains define which entity types and edge types are valid for a
//! particular analysis context (code, forensic, custom).

#[cfg(feature = "code-domain")]
pub mod code;
#[cfg(feature = "forensic-domain")]
pub mod forensic;

use crate::entity::EntityType;
use crate::relationship::RelationType;

// ---------------------------------------------------------------------------
// Domain trait
// ---------------------------------------------------------------------------

/// A domain defines the set of entity and edge types relevant to a
/// particular analysis context.
pub trait Domain: Send + Sync {
    /// Machine-readable tag for this domain (e.g. "code", "forensic").
    fn domain_tag(&self) -> &str;

    /// Human-readable display name.
    fn display_name(&self) -> &str;

    /// The set of entity types valid in this domain.
    fn entity_types(&self) -> &[EntityType];

    /// The set of relationship types valid in this domain.
    fn edge_types(&self) -> &[RelationType];

    /// Whether the given entity type belongs to this domain.
    fn accepts_entity(&self, et: &EntityType) -> bool {
        self.entity_types().contains(et)
    }

    /// Whether the given relationship type belongs to this domain.
    fn accepts_edge(&self, rt: &RelationType) -> bool {
        self.edge_types().contains(rt)
    }
}

// ---------------------------------------------------------------------------
// DomainRegistry
// ---------------------------------------------------------------------------

/// Registry for domain configurations.
///
/// The registry allows registering code, forensic, and custom domains.
/// During extraction and analysis, the active domain determines which
/// entity and relationship types are recognized.
pub struct DomainRegistry {
    domains: Vec<Box<dyn Domain>>,
}

impl DomainRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            domains: Vec::new(),
        }
    }

    /// Create a registry pre-loaded with all built-in domains
    /// (based on enabled features).
    pub fn with_defaults() -> Self {
        #[allow(unused_mut)]
        let mut reg = Self::new();

        #[cfg(feature = "code-domain")]
        reg.register(Box::new(code::CodeDomainConfig::new()));

        #[cfg(feature = "forensic-domain")]
        reg.register(Box::new(forensic::ForensicDomainConfig::new()));

        reg
    }

    /// Register a new domain.
    pub fn register(&mut self, domain: Box<dyn Domain>) {
        self.domains.push(domain);
    }

    /// Look up a domain by tag.
    pub fn get(&self, tag: &str) -> Option<&dyn Domain> {
        self.domains
            .iter()
            .find(|d| d.domain_tag() == tag)
            .map(|d| d.as_ref())
    }

    /// Return the tags of all registered domains.
    pub fn tags(&self) -> Vec<&str> {
        self.domains.iter().map(|d| d.domain_tag()).collect()
    }

    /// Return all registered domains.
    pub fn all(&self) -> &[Box<dyn Domain>] {
        &self.domains
    }

    /// Check if an entity type is accepted by any registered domain.
    pub fn accepts_entity(&self, et: &EntityType) -> bool {
        self.domains.iter().any(|d| d.accepts_entity(et))
    }

    /// Check if a relationship type is accepted by any registered domain.
    pub fn accepts_edge(&self, rt: &RelationType) -> bool {
        self.domains.iter().any(|d| d.accepts_edge(rt))
    }
}

impl Default for DomainRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial test domain for unit testing.
    struct TestDomain;

    impl Domain for TestDomain {
        fn domain_tag(&self) -> &str {
            "test"
        }

        fn display_name(&self) -> &str {
            "Test Domain"
        }

        fn entity_types(&self) -> &[EntityType] {
            &[EntityType::Module, EntityType::Function]
        }

        fn edge_types(&self) -> &[RelationType] {
            &[RelationType::Calls]
        }
    }

    #[test]
    fn registry_basics() {
        let mut reg = DomainRegistry::new();
        reg.register(Box::new(TestDomain));
        assert_eq!(reg.tags(), vec!["test"]);
        assert!(reg.get("test").is_some());
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn domain_accepts() {
        let d = TestDomain;
        assert!(d.accepts_entity(&EntityType::Module));
        assert!(!d.accepts_entity(&EntityType::Person));
        assert!(d.accepts_edge(&RelationType::Calls));
        assert!(!d.accepts_edge(&RelationType::Contradicts));
    }

    #[test]
    fn registry_accepts_across_domains() {
        let mut reg = DomainRegistry::new();
        reg.register(Box::new(TestDomain));
        assert!(reg.accepts_entity(&EntityType::Module));
        assert!(!reg.accepts_entity(&EntityType::Person));
    }
}
