//! Core types for the resource tree.

use std::collections::HashMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::scoring::NodeScoring;

/// A resource identifier, path-like (e.g. "/kernel/services/cron").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceId(pub String);

impl ResourceId {
    /// Create a new resource ID from a string.
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// Return the parent path, or None if this is the root "/".
    pub fn parent(&self) -> Option<ResourceId> {
        if self.0 == "/" {
            return None;
        }
        match self.0.rfind('/') {
            Some(0) => Some(ResourceId::root()),
            Some(pos) => Some(ResourceId(self.0[..pos].to_string())),
            None => None,
        }
    }

    /// The root resource ID.
    pub fn root() -> Self {
        Self("/".to_string())
    }

    /// Whether this is the root node.
    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }

    /// Return the last segment of the path (the "name").
    pub fn name(&self) -> &str {
        if self.0 == "/" {
            return "/";
        }
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for ResourceId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for ResourceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// The kind of resource in the tree.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceKind {
    Namespace,
    Service,
    Agent,
    Device,
    Topic,
    Container,
    App,
    Tool,
    Environment,
    Custom(String),
}

/// A node in the resource tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceNode {
    /// The full path identifier.
    pub id: ResourceId,
    /// What kind of resource this is.
    pub kind: ResourceKind,
    /// Parent node (None only for root).
    pub parent: Option<ResourceId>,
    /// Ordered list of child IDs.
    pub children: Vec<ResourceId>,
    /// Arbitrary key-value metadata.
    pub metadata: HashMap<String, serde_json::Value>,
    /// Merkle hash of this subtree (SHAKE-256 via rvf-crypto).
    #[serde(with = "hash_serde")]
    pub merkle_hash: [u8; 32],
    /// Trust/performance scoring vector (6 dimensions, 24 bytes).
    #[serde(default)]
    pub scoring: NodeScoring,
    /// When this node was created.
    pub created_at: DateTime<Utc>,
    /// When this node was last modified.
    pub updated_at: DateTime<Utc>,
}

impl ResourceNode {
    /// Create a new node with default metadata and zeroed Merkle hash.
    pub fn new(id: ResourceId, kind: ResourceKind, parent: Option<ResourceId>) -> Self {
        let now = Utc::now();
        Self {
            id,
            kind,
            parent,
            children: Vec::new(),
            metadata: HashMap::new(),
            merkle_hash: [0u8; 32],
            scoring: NodeScoring::default(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// Role an agent may hold on a resource.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    Owner,
    Admin,
    Operator,
    Viewer,
    Custom(String),
}

/// An action that may be performed on a resource.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Read,
    Write,
    Execute,
    Admin,
    Create,
    Delete,
    Custom(String),
}

/// Serde support for [u8; 32] as hex strings.
mod hash_serde {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(hash: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        serializer.serialize_str(&hex)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes: Vec<u8> = (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(serde::de::Error::custom))
            .collect::<Result<Vec<u8>, _>>()?;
        let mut arr = [0u8; 32];
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom(format!(
                "expected 32 bytes, got {}",
                bytes.len()
            )));
        }
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_id_root() {
        let root = ResourceId::root();
        assert!(root.is_root());
        assert_eq!(root.parent(), None);
        assert_eq!(root.name(), "/");
    }

    #[test]
    fn resource_id_parent() {
        let id = ResourceId::new("/kernel/services/cron");
        assert_eq!(id.parent(), Some(ResourceId::new("/kernel/services")));

        let id2 = ResourceId::new("/kernel");
        assert_eq!(id2.parent(), Some(ResourceId::root()));
    }

    #[test]
    fn resource_id_name() {
        let id = ResourceId::new("/kernel/services/cron");
        assert_eq!(id.name(), "cron");
    }

    #[test]
    fn resource_id_display() {
        let id = ResourceId::new("/apps/myapp");
        assert_eq!(format!("{id}"), "/apps/myapp");
    }

    #[test]
    fn resource_kind_serde_roundtrip() {
        let kind = ResourceKind::Custom("gpu".to_string());
        let json = serde_json::to_string(&kind).unwrap();
        let back: ResourceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn resource_kind_tool_serde_roundtrip() {
        let kind = ResourceKind::Tool;
        let json = serde_json::to_string(&kind).unwrap();
        let back: ResourceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn resource_node_serde_roundtrip() {
        let node = ResourceNode::new(
            ResourceId::new("/test"),
            ResourceKind::Namespace,
            Some(ResourceId::root()),
        );
        let json = serde_json::to_string(&node).unwrap();
        let back: ResourceNode = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, node.id);
        assert_eq!(back.kind, node.kind);
        assert_eq!(back.merkle_hash, [0u8; 32]);
    }

    #[test]
    fn role_and_action_serde() {
        let role = Role::Custom("deployer".to_string());
        let json = serde_json::to_string(&role).unwrap();
        let back: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(back, role);

        let action = Action::Execute;
        let json = serde_json::to_string(&action).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back, action);
    }
}
