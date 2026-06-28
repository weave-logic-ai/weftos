//! Entity identity and type taxonomy for the knowledge graph.
//!
//! `EntityId` uses BLAKE3 to produce deterministic 32-byte identifiers from
//! (domain, entity_type, name, source_file). `EntityType` covers both the
//! code-analysis and forensic-analysis domains.

use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// DomainTag
// ---------------------------------------------------------------------------

/// Domain discriminator byte embedded in entity IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DomainTag {
    /// Source-code analysis domain (0x20).
    Code,
    /// Forensic / document analysis domain (0x21).
    Forensic,
    /// User-defined domain.
    Custom(u8),
}

impl DomainTag {
    /// Return the single-byte discriminator used in BLAKE3 hashing.
    pub fn as_u8(&self) -> u8 {
        match self {
            Self::Code => 0x20,
            Self::Forensic => 0x21,
            Self::Custom(v) => *v,
        }
    }
}

// ---------------------------------------------------------------------------
// EntityId
// ---------------------------------------------------------------------------

/// Unique, deterministic identifier for a knowledge graph entity.
///
/// Computed as `BLAKE3(domain_byte || entity_type_discriminant || name || source_file)`.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityId(pub [u8; 32]);

impl EntityId {
    /// Create a new deterministic entity ID.
    pub fn new(domain: &DomainTag, entity_type: &EntityType, name: &str, source: &str) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[domain.as_u8()]);
        hasher.update(entity_type.discriminant().as_bytes());
        hasher.update(name.as_bytes());
        hasher.update(source.as_bytes());
        Self(*hasher.finalize().as_bytes())
    }

    /// Build an `EntityId` from a legacy Python-style string ID by hashing it.
    ///
    /// This is *not* the same hash as `new()` -- it simply hashes the raw string
    /// so that legacy IDs can be used as lookup keys.
    pub fn from_legacy_string(s: &str) -> Self {
        let hash = blake3::hash(s.as_bytes());
        Self(*hash.as_bytes())
    }

    /// Return the raw 32-byte hash.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Hex-encode the ID (64 characters).
    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityId({})", &self.to_hex()[..16])
    }
}

/// Simple hex encoder (avoids pulling in the `hex` crate).
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ---------------------------------------------------------------------------
// EntityType
// ---------------------------------------------------------------------------

/// Unified entity type taxonomy covering code and forensic domains.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityType {
    // --- Code domain ---
    Module,
    Class,
    Function,
    Import,
    Config,
    Service,
    Endpoint,
    Interface,
    Struct,
    Enum,
    Constant,
    Package,

    // --- Forensic domain ---
    Person,
    Event,
    Evidence,
    Location,
    Timeline,
    Document,
    Hypothesis,
    Organization,
    PhysicalObject,
    DigitalArtifact,
    FinancialRecord,
    Communication,

    // --- Shared ---
    File,
    Concept,
    Custom(String),
}

impl EntityType {
    /// Stable string discriminant used in BLAKE3 ID hashing.
    ///
    /// These strings must never change once IDs have been persisted.
    pub fn discriminant(&self) -> &str {
        match self {
            Self::Module => "module",
            Self::Class => "class",
            Self::Function => "function",
            Self::Import => "import",
            Self::Config => "config",
            Self::Service => "service",
            Self::Endpoint => "endpoint",
            Self::Interface => "interface",
            Self::Struct => "struct_",
            Self::Enum => "enum_",
            Self::Constant => "constant",
            Self::Package => "package",
            Self::Person => "person",
            Self::Event => "event",
            Self::Evidence => "evidence",
            Self::Location => "location",
            Self::Timeline => "timeline",
            Self::Document => "document",
            Self::Hypothesis => "hypothesis",
            Self::Organization => "organization",
            Self::PhysicalObject => "physical_object",
            Self::DigitalArtifact => "digital_artifact",
            Self::FinancialRecord => "financial_record",
            Self::Communication => "communication",
            Self::File => "file",
            Self::Concept => "concept",
            Self::Custom(s) => s.as_str(),
        }
    }
}

// ---------------------------------------------------------------------------
// FileType
// ---------------------------------------------------------------------------

/// Classification of the source material that produced an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileType {
    Code,
    Document,
    Paper,
    Image,
    Config,
    Unknown,
}

impl FileType {
    /// All valid string representations (used by schema validation).
    pub const VALID_STRINGS: &[&str] = &["code", "document", "paper", "image", "config", "unknown"];

    /// Parse from a lowercase string.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s {
            "code" => Some(Self::Code),
            "document" => Some(Self::Document),
            "paper" => Some(Self::Paper),
            "image" => Some(Self::Image),
            "config" => Some(Self::Config),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_id_deterministic() {
        let a = EntityId::new(&DomainTag::Code, &EntityType::Function, "foo", "bar.py");
        let b = EntityId::new(&DomainTag::Code, &EntityType::Function, "foo", "bar.py");
        assert_eq!(a, b);
    }

    #[test]
    fn entity_id_different_inputs() {
        let a = EntityId::new(&DomainTag::Code, &EntityType::Function, "foo", "bar.py");
        let b = EntityId::new(&DomainTag::Code, &EntityType::Class, "foo", "bar.py");
        assert_ne!(a, b);
    }

    #[test]
    fn entity_id_hex_roundtrip() {
        let id = EntityId::new(&DomainTag::Code, &EntityType::Module, "auth", "auth.py");
        let hex = id.to_hex();
        assert_eq!(hex.len(), 64);
    }

    #[test]
    fn entity_id_serde_roundtrip() {
        let id = EntityId::new(&DomainTag::Code, &EntityType::Module, "test", "test.py");
        let json = serde_json::to_string(&id).unwrap();
        let back: EntityId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn domain_tag_bytes() {
        assert_eq!(DomainTag::Code.as_u8(), 0x20);
        assert_eq!(DomainTag::Forensic.as_u8(), 0x21);
        assert_eq!(DomainTag::Custom(0xFF).as_u8(), 0xFF);
    }

    #[test]
    fn file_type_serde() {
        let ft = FileType::Code;
        let json = serde_json::to_string(&ft).unwrap();
        assert_eq!(json, "\"code\"");
        let back: FileType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, FileType::Code);
    }

    #[test]
    fn entity_type_discriminants_stable() {
        assert_eq!(EntityType::Module.discriminant(), "module");
        assert_eq!(EntityType::Struct.discriminant(), "struct_");
        assert_eq!(EntityType::Enum.discriminant(), "enum_");
        assert_eq!(
            EntityType::Custom("rationale".into()).discriminant(),
            "rationale"
        );
    }

    #[test]
    fn from_legacy_string() {
        let a = EntityId::from_legacy_string("auth_authservice");
        let b = EntityId::from_legacy_string("auth_authservice");
        assert_eq!(a, b);
        let c = EntityId::from_legacy_string("other_id");
        assert_ne!(a, c);
    }
}
