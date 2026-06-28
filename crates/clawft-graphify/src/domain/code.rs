//! Code analysis domain configuration.
//!
//! Defines entity types specific to source code analysis (Module, Class,
//! Function, etc.) and the edge types that connect them (calls, imports,
//! depends_on, etc.).
//!
//! This domain integrates with the existing 8 analyzers in the WeftOS
//! assessment pipeline. Graphify findings complement them:
//!
//! - **ComplexityAnalyzer** -- graphify god-node detection provides a
//!   structural view of coupling, while complexity counts cyclomatic paths.
//! - **DependencyAnalyzer** -- graphify maps cross-file imports as graph
//!   edges; the dependency analyzer tracks crate/package-level deps.
//! - **SecurityAnalyzer** -- graphify identifies sensitive data flows via
//!   call chains; security analyzer checks for known vulnerability patterns.
//! - **TopologyAnalyzer** -- graphify community detection reveals module
//!   clusters; topology reports on overall architecture shape.
//! - **DataSourceAnalyzer** -- graphify can tag database-connected entities
//!   so data source findings get richer context.
//! - **NetworkAnalyzer** -- graphify endpoint entities connect to network
//!   analyzer's port/protocol findings.
//! - **RabbitMQAnalyzer** -- graphify can map message producers/consumers
//!   as service nodes with "publishes_to" edges.
//! - **TerraformAnalyzer** -- graphify config entities cross-reference
//!   Terraform resource declarations.

use crate::domain::Domain;
use crate::entity::EntityType;
use crate::relationship::RelationType;

// ---------------------------------------------------------------------------
// CodeDomainConfig
// ---------------------------------------------------------------------------

/// Domain configuration for source code analysis.
///
/// Maps the standard code entity types (Module, Class, Function, Import,
/// Config, Service, Endpoint, Interface, Struct, Enum, Constant, Package)
/// and the 10 code relationship types.
pub struct CodeDomainConfig {
    entity_types: Vec<EntityType>,
    edge_types: Vec<RelationType>,
}

impl CodeDomainConfig {
    /// Create the default code domain configuration.
    pub fn new() -> Self {
        Self {
            entity_types: vec![
                EntityType::Module,
                EntityType::Class,
                EntityType::Function,
                EntityType::Import,
                EntityType::Config,
                EntityType::Service,
                EntityType::Endpoint,
                EntityType::Interface,
                EntityType::Struct,
                EntityType::Enum,
                EntityType::Constant,
                EntityType::Package,
                EntityType::File,
            ],
            edge_types: vec![
                RelationType::Calls,
                RelationType::Imports,
                RelationType::ImportsFrom,
                RelationType::DependsOn,
                RelationType::Contains,
                RelationType::Implements,
                RelationType::Configures,
                RelationType::Extends,
                RelationType::MethodOf,
                RelationType::Instantiates,
                RelationType::RelatedTo,
            ],
        }
    }
}

impl Default for CodeDomainConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl Domain for CodeDomainConfig {
    fn domain_tag(&self) -> &str {
        "code"
    }

    fn display_name(&self) -> &str {
        "Code Analysis"
    }

    fn entity_types(&self) -> &[EntityType] {
        &self.entity_types
    }

    fn edge_types(&self) -> &[RelationType] {
        &self.edge_types
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_domain_entity_types() {
        let d = CodeDomainConfig::new();
        assert!(d.accepts_entity(&EntityType::Module));
        assert!(d.accepts_entity(&EntityType::Function));
        assert!(d.accepts_entity(&EntityType::Class));
        assert!(d.accepts_entity(&EntityType::Struct));
        assert!(!d.accepts_entity(&EntityType::Person));
        assert!(!d.accepts_entity(&EntityType::Evidence));
    }

    #[test]
    fn code_domain_edge_types() {
        let d = CodeDomainConfig::new();
        assert!(d.accepts_edge(&RelationType::Calls));
        assert!(d.accepts_edge(&RelationType::Imports));
        assert!(d.accepts_edge(&RelationType::Contains));
        assert!(!d.accepts_edge(&RelationType::Contradicts));
        assert!(!d.accepts_edge(&RelationType::WitnessedBy));
    }

    #[test]
    fn code_domain_tag() {
        let d = CodeDomainConfig::new();
        assert_eq!(d.domain_tag(), "code");
        assert_eq!(d.display_name(), "Code Analysis");
    }

    #[test]
    fn code_domain_counts() {
        let d = CodeDomainConfig::new();
        assert_eq!(d.entity_types().len(), 13); // 12 code + File
        assert_eq!(d.edge_types().len(), 11); // 10 code + RelatedTo
    }
}
