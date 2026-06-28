//! Workspace types for the global workspace registry.
//!
//! The registry lives at `~/.clawft/workspaces.json` and tracks
//! all known workspaces by name and filesystem path.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Entry in the global workspace registry (`~/.clawft/workspaces.json`).
///
/// Timestamps are stored as `DateTime<Utc>`. For backward compatibility with
/// registries that stored ISO 8601 strings, the custom deserializer accepts
/// both RFC 3339 strings and native `DateTime<Utc>` JSON representations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    /// Human-readable workspace name (unique within registry).
    pub name: String,

    /// Absolute path to the workspace root directory.
    pub path: PathBuf,

    /// UTC timestamp of the last time this workspace was accessed.
    #[serde(default, deserialize_with = "deserialize_optional_datetime")]
    pub last_accessed: Option<DateTime<Utc>>,

    /// UTC timestamp of when the workspace was first created.
    #[serde(default, deserialize_with = "deserialize_optional_datetime")]
    pub created_at: Option<DateTime<Utc>>,
}

/// Deserialize an `Option<DateTime<Utc>>` that accepts both:
/// - A native chrono `DateTime<Utc>` JSON value (RFC 3339 string from chrono's Serialize)
/// - A plain ISO 8601 / RFC 3339 string (legacy format)
/// - `null` / missing field -> `None`
fn deserialize_optional_datetime<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(s) => {
            // Try RFC 3339 parsing (covers both chrono's output and legacy strings).
            match DateTime::parse_from_rfc3339(&s) {
                Ok(dt) => Ok(Some(dt.with_timezone(&Utc))),
                Err(_) => {
                    // Try a more relaxed ISO 8601 parse as fallback.
                    match s.parse::<DateTime<Utc>>() {
                        Ok(dt) => Ok(Some(dt)),
                        Err(_) => Ok(None), // Gracefully degrade: treat unparseable as missing.
                    }
                }
            }
        }
    }
}

/// Global workspace registry.
///
/// Serialized to / deserialized from `~/.clawft/workspaces.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceRegistry {
    /// All known workspace entries.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
}

impl WorkspaceRegistry {
    /// Load the registry from a JSON file, returning `Default` if it does
    /// not exist.
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Persist the registry to a JSON file, creating parent directories
    /// as needed.
    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let dir = path.parent().ok_or("registry path has no parent")?;
        std::fs::create_dir_all(dir)?;
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Find an entry by workspace name.
    pub fn find_by_name(&self, name: &str) -> Option<&WorkspaceEntry> {
        self.workspaces.iter().find(|e| e.name == name)
    }

    /// Find an entry by filesystem path.
    pub fn find_by_path(&self, path: &Path) -> Option<&WorkspaceEntry> {
        self.workspaces.iter().find(|e| e.path == path)
    }

    /// Register a workspace entry.
    ///
    /// If an entry with the same name already exists, it is replaced.
    pub fn register(&mut self, entry: WorkspaceEntry) {
        self.remove_by_name(&entry.name);
        self.workspaces.push(entry);
    }

    /// Remove a workspace entry by name.
    ///
    /// Returns `true` if an entry was found and removed.
    pub fn remove_by_name(&mut self, name: &str) -> bool {
        let before = self.workspaces.len();
        self.workspaces.retain(|e| e.name != name);
        self.workspaces.len() < before
    }
}

// ── Registry trait implementation ────────────────────────────────────

impl crate::Registry for WorkspaceRegistry {
    type Value = WorkspaceEntry;

    fn get(&self, key: &str) -> Option<Self::Value> {
        self.find_by_name(key).cloned()
    }

    fn list_keys(&self) -> Vec<String> {
        self.workspaces.iter().map(|e| e.name.clone()).collect()
    }

    fn count(&self) -> usize {
        self.workspaces.len()
    }

    fn is_empty(&self) -> bool {
        self.workspaces.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_entry(name: &str) -> WorkspaceEntry {
        WorkspaceEntry {
            name: name.into(),
            path: PathBuf::from(format!("/tmp/ws-{name}")),
            last_accessed: None,
            created_at: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
        }
    }

    #[test]
    fn default_registry_is_empty() {
        let reg = WorkspaceRegistry::default();
        assert!(reg.workspaces.is_empty());
    }

    #[test]
    fn register_and_find_by_name() {
        let mut reg = WorkspaceRegistry::default();
        reg.register(sample_entry("alpha"));
        assert!(reg.find_by_name("alpha").is_some());
        assert!(reg.find_by_name("beta").is_none());
    }

    #[test]
    fn register_and_find_by_path() {
        let mut reg = WorkspaceRegistry::default();
        reg.register(sample_entry("alpha"));
        assert!(reg.find_by_path(Path::new("/tmp/ws-alpha")).is_some());
        assert!(reg.find_by_path(Path::new("/tmp/ws-other")).is_none());
    }

    #[test]
    fn register_replaces_existing() {
        let mut reg = WorkspaceRegistry::default();
        let mut e1 = sample_entry("alpha");
        e1.path = PathBuf::from("/old");
        reg.register(e1);

        let mut e2 = sample_entry("alpha");
        e2.path = PathBuf::from("/new");
        reg.register(e2);

        assert_eq!(reg.workspaces.len(), 1);
        assert_eq!(reg.find_by_name("alpha").unwrap().path, Path::new("/new"));
    }

    #[test]
    fn remove_by_name_returns_true_when_found() {
        let mut reg = WorkspaceRegistry::default();
        reg.register(sample_entry("alpha"));
        assert!(reg.remove_by_name("alpha"));
        assert!(reg.workspaces.is_empty());
    }

    #[test]
    fn remove_by_name_returns_false_when_not_found() {
        let mut reg = WorkspaceRegistry::default();
        assert!(!reg.remove_by_name("missing"));
    }

    #[test]
    fn serde_roundtrip() {
        let mut reg = WorkspaceRegistry::default();
        reg.register(sample_entry("alpha"));
        reg.register(sample_entry("beta"));

        let json = serde_json::to_string(&reg).unwrap();
        let restored: WorkspaceRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.workspaces.len(), 2);
        assert!(restored.find_by_name("alpha").is_some());
        assert!(restored.find_by_name("beta").is_some());
    }

    #[test]
    fn load_returns_default_for_missing_file() {
        let reg = WorkspaceRegistry::load(Path::new("/nonexistent/workspaces.json")).unwrap();
        assert!(reg.workspaces.is_empty());
    }

    #[test]
    fn load_save_roundtrip() {
        let dir = std::env::temp_dir().join("clawft-test-ws-registry");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("workspaces.json");

        let mut reg = WorkspaceRegistry::default();
        reg.register(sample_entry("test-ws"));
        reg.save(&path).unwrap();

        let loaded = WorkspaceRegistry::load(&path).unwrap();
        assert_eq!(loaded.workspaces.len(), 1);
        assert_eq!(loaded.find_by_name("test-ws").unwrap().name, "test-ws");

        // Clean up
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_entry_optional_fields_default() {
        let json = r#"{"name": "ws", "path": "/tmp/ws"}"#;
        let entry: WorkspaceEntry = serde_json::from_str(json).unwrap();
        assert!(entry.last_accessed.is_none());
        assert!(entry.created_at.is_none());
    }

    #[test]
    fn backward_compat_string_timestamps() {
        // Legacy format: plain ISO 8601 strings in JSON.
        let json = r#"{
            "name": "legacy",
            "path": "/tmp/legacy",
            "last_accessed": "2026-01-15T10:30:00Z",
            "created_at": "2026-01-01T00:00:00Z"
        }"#;
        let entry: WorkspaceEntry = serde_json::from_str(json).unwrap();
        assert!(entry.last_accessed.is_some());
        assert!(entry.created_at.is_some());
        let created = entry.created_at.unwrap();
        assert_eq!(created, Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn unparseable_timestamp_becomes_none() {
        let json = r#"{
            "name": "bad-ts",
            "path": "/tmp/bad",
            "created_at": "not-a-date"
        }"#;
        let entry: WorkspaceEntry = serde_json::from_str(json).unwrap();
        assert!(entry.created_at.is_none());
    }
}
