//! Permission persistence and approval for WASM plugin upgrades.
//!
//! Provides [`PermissionStore`] for persisting user-approved permissions per
//! plugin, [`ApprovedRecord`] for the on-disk format, and the
//! [`PermissionApprover`] trait for requesting user consent on new permissions.
//!
//! # Storage Layout
//!
//! Approved permissions are stored as JSON files:
//! ```text
//! {base_dir}/{plugin_id}/approved_permissions.json
//! ```
//!
//! # Version Upgrade Flow
//!
//! 1. Load the [`ApprovedRecord`] for the plugin (if any).
//! 2. Compute [`PermissionDiff`] between approved and requested permissions.
//! 3. If the diff is non-empty, invoke [`PermissionApprover::approve`].
//! 4. On approval, save the updated [`ApprovedRecord`].

use std::path::PathBuf;

use clawft_plugin::{PermissionDiff, PluginPermissions};
use serde::{Deserialize, Serialize};

/// File name for persisted approved permissions.
const APPROVED_FILE: &str = "approved_permissions.json";

/// Record of user-approved permissions for a plugin version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovedRecord {
    /// Plugin version at the time of approval.
    pub version: String,
    /// The permissions that were approved by the user.
    pub permissions: PluginPermissions,
    /// ISO 8601 timestamp of when the approval was granted.
    pub approved_at: String,
}

/// Persists user-approved permissions per plugin.
///
/// Each plugin's approved permissions are stored as a JSON file under
/// `{base_dir}/{plugin_id}/approved_permissions.json`.
pub struct PermissionStore {
    base_dir: PathBuf,
}

impl PermissionStore {
    /// Create a new permission store rooted at the given directory.
    ///
    /// The directory does not need to exist yet; it will be created on
    /// the first [`save`](Self::save) call.
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Defense-in-depth: verify a constructed path stays within `base_dir`.
    ///
    /// Returns the canonical form of `path` on success, or a
    /// `PermissionDenied` error if the path escapes the base directory.
    fn ensure_within_base(
        &self,
        path: &std::path::Path,
        plugin_id: &str,
    ) -> Result<PathBuf, std::io::Error> {
        let canonical_base = self
            .base_dir
            .canonicalize()
            .unwrap_or_else(|_| self.base_dir.clone());
        let canonical_path = path.canonicalize()?;
        if !canonical_path.starts_with(&canonical_base) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("plugin id would escape base directory: {}", plugin_id),
            ));
        }
        Ok(canonical_path)
    }

    /// Path to the approved permissions file for a plugin.
    fn record_path(&self, plugin_id: &str) -> Result<PathBuf, std::io::Error> {
        let dir = self.base_dir.join(plugin_id);
        // The directory must already exist to canonicalize; callers that
        // create the directory (save) call ensure_within_base separately.
        if dir.exists() {
            let canonical = self.ensure_within_base(&dir, plugin_id)?;
            Ok(canonical.join(APPROVED_FILE))
        } else {
            Ok(dir.join(APPROVED_FILE))
        }
    }

    /// Load the previously approved permissions for a plugin.
    ///
    /// Returns `None` if the plugin has never been approved or the file
    /// cannot be read/parsed. Returns `None` if the plugin id would
    /// escape the base directory.
    pub fn load(&self, plugin_id: &str) -> Option<ApprovedRecord> {
        let path = self.record_path(plugin_id).ok()?;
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Save approved permissions after user consent.
    ///
    /// Creates the plugin directory if it does not exist. Returns an error
    /// if the resulting path would escape the base directory.
    pub fn save(&self, plugin_id: &str, record: &ApprovedRecord) -> Result<(), std::io::Error> {
        let dir = self.base_dir.join(plugin_id);
        std::fs::create_dir_all(&dir)?;
        // Defense-in-depth: ensure the created directory is still under base_dir
        let canonical_dir = self.ensure_within_base(&dir, plugin_id)?;
        let path = canonical_dir.join(APPROVED_FILE);
        let json = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }
}

// ---------------------------------------------------------------------------
// PermissionApprover trait and implementations
// ---------------------------------------------------------------------------

/// Callback for requesting user approval of new permissions during a
/// plugin version upgrade.
///
/// Implementations may prompt the user interactively, auto-approve for
/// non-interactive mode, or record requests for testing.
pub trait PermissionApprover: Send + Sync {
    /// Ask the user whether to approve the given permission diff for a plugin.
    ///
    /// Returns `true` if the user approves, `false` to deny the upgrade.
    fn approve(&self, plugin_id: &str, diff: &PermissionDiff) -> bool;
}

/// Auto-approves all permission requests.
///
/// Intended for non-interactive mode or testing scenarios where prompting
/// is not desired.
pub struct AutoApprover;

impl PermissionApprover for AutoApprover {
    fn approve(&self, _plugin_id: &str, _diff: &PermissionDiff) -> bool {
        true
    }
}

/// Records approval requests and returns a pre-configured response.
///
/// Useful for testing that the correct diff is presented to the user.
pub struct MockApprover {
    /// The response to return from [`approve`](PermissionApprover::approve).
    response: bool,
    /// Recorded calls: `(plugin_id, diff)`.
    calls: std::sync::Mutex<Vec<(String, PermissionDiff)>>,
}

impl MockApprover {
    /// Create a mock approver that always returns the given response.
    pub fn new(response: bool) -> Self {
        Self {
            response,
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Return all recorded calls in order.
    pub fn calls(&self) -> Vec<(String, PermissionDiff)> {
        self.calls.lock().expect("lock poisoned").clone()
    }
}

impl PermissionApprover for MockApprover {
    fn approve(&self, plugin_id: &str, diff: &PermissionDiff) -> bool {
        self.calls
            .lock()
            .expect("lock poisoned")
            .push((plugin_id.to_string(), diff.clone()));
        self.response
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(version: &str, perms: PluginPermissions) -> ApprovedRecord {
        ApprovedRecord {
            version: version.to_string(),
            permissions: perms,
            approved_at: "2026-02-20T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn store_save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("clawft_perm_store_roundtrip");
        let _ = std::fs::remove_dir_all(&dir);

        let store = PermissionStore::new(dir.clone());
        let perms = PluginPermissions {
            network: vec!["api.example.com".into()],
            filesystem: vec!["/tmp".into()],
            env_vars: vec!["HOME".into()],
            shell: false,
        };
        let record = make_record("1.0.0", perms);

        store.save("com.example.test", &record).unwrap();
        let loaded = store.load("com.example.test");
        assert_eq!(loaded, Some(record));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_load_returns_none_for_unknown_plugin() {
        let dir = std::env::temp_dir().join("clawft_perm_store_unknown");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);

        let store = PermissionStore::new(dir.clone());
        assert!(store.load("com.example.nonexistent").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_overwrite_on_upgrade() {
        let dir = std::env::temp_dir().join("clawft_perm_store_overwrite");
        let _ = std::fs::remove_dir_all(&dir);

        let store = PermissionStore::new(dir.clone());
        let plugin_id = "com.example.upgrade";

        // v1 approval
        let v1 = make_record("1.0.0", PluginPermissions::default());
        store.save(plugin_id, &v1).unwrap();

        // v2 approval with more permissions
        let v2_perms = PluginPermissions {
            network: vec!["api.example.com".into()],
            ..Default::default()
        };
        let v2 = make_record("2.0.0", v2_perms);
        store.save(plugin_id, &v2).unwrap();

        let loaded = store.load(plugin_id).unwrap();
        assert_eq!(loaded.version, "2.0.0");
        assert_eq!(loaded.permissions.network, vec!["api.example.com"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_approver_always_approves() {
        let approver = AutoApprover;
        let diff = PermissionDiff {
            new_network: vec!["evil.com".into()],
            shell_escalation: true,
            ..Default::default()
        };
        assert!(approver.approve("any-plugin", &diff));
    }

    #[test]
    fn mock_approver_records_calls_and_returns_configured_response() {
        let approver = MockApprover::new(false);
        let diff = PermissionDiff {
            new_network: vec!["api.example.com".into()],
            ..Default::default()
        };

        let result = approver.approve("com.example.test", &diff);
        assert!(!result, "mock should return configured response");

        let calls = approver.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "com.example.test");
        assert_eq!(calls[0].1, diff);
    }

    #[test]
    fn mock_approver_records_multiple_calls() {
        let approver = MockApprover::new(true);
        let diff1 = PermissionDiff {
            new_network: vec!["a.com".into()],
            ..Default::default()
        };
        let diff2 = PermissionDiff {
            shell_escalation: true,
            ..Default::default()
        };

        assert!(approver.approve("plugin-a", &diff1));
        assert!(approver.approve("plugin-b", &diff2));

        let calls = approver.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "plugin-a");
        assert_eq!(calls[1].0, "plugin-b");
    }

    /// T41: Plugin version upgrade permission re-prompt.
    ///
    /// Simulates a plugin upgrading from v1 to v2 with expanded permissions.
    /// Verifies that only the NEW permissions are presented for re-approval,
    /// not the entire permission set.
    #[test]
    fn t41_version_upgrade_permission_reprompt() {
        let dir = std::env::temp_dir().join("clawft_t41_reprompt");
        let _ = std::fs::remove_dir_all(&dir);

        let store = PermissionStore::new(dir.clone());
        let plugin_id = "com.example.upgrading-plugin";

        // --- Step 1: v1 manifest with minimal permissions ---
        let v1_perms = PluginPermissions {
            network: vec![],
            filesystem: vec!["/tmp".into()],
            env_vars: vec!["HOME".into()],
            shell: false,
        };

        // User approves v1 (simulated by saving directly)
        let v1_record = ApprovedRecord {
            version: "1.0.0".to_string(),
            permissions: v1_perms.clone(),
            approved_at: "2026-02-20T10:00:00Z".to_string(),
        };
        store.save(plugin_id, &v1_record).unwrap();

        // --- Step 2: v2 manifest with expanded permissions ---
        let v2_perms = PluginPermissions {
            network: vec!["api.example.com".into()],
            filesystem: vec!["/tmp".into()],
            env_vars: vec!["HOME".into(), "API_KEY".into()],
            shell: false,
        };

        // --- Step 3: Load approved and compute diff ---
        let approved = store.load(plugin_id).unwrap();
        assert_eq!(approved.version, "1.0.0");

        let diff = PluginPermissions::diff(&approved.permissions, &v2_perms);

        // --- Step 4: Verify diff contains ONLY the new permissions ---
        assert_eq!(
            diff.new_network,
            vec!["api.example.com"],
            "only the new network host should appear"
        );
        assert_eq!(
            diff.new_env_vars,
            vec!["API_KEY"],
            "only the new env var should appear"
        );
        assert!(
            diff.new_filesystem.is_empty(),
            "filesystem is unchanged, should be empty"
        );
        assert!(
            !diff.shell_escalation,
            "shell is still false, no escalation"
        );
        assert!(!diff.is_empty(), "diff should NOT be empty");

        // --- Step 5: MockApprover receives exactly the correct diff ---
        let mock = MockApprover::new(true);
        let approved_by_user = mock.approve(plugin_id, &diff);
        assert!(approved_by_user, "mock is configured to approve");

        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, plugin_id);
        assert_eq!(calls[0].1.new_network, vec!["api.example.com"]);
        assert_eq!(calls[0].1.new_env_vars, vec!["API_KEY"]);
        assert!(calls[0].1.new_filesystem.is_empty());
        assert!(!calls[0].1.shell_escalation);

        // --- Step 6: After approval, save updated record ---
        let v2_record = ApprovedRecord {
            version: "2.0.0".to_string(),
            permissions: v2_perms.clone(),
            approved_at: "2026-02-20T11:00:00Z".to_string(),
        };
        store.save(plugin_id, &v2_record).unwrap();

        // --- Step 7: Re-loading shows no diff against v2 ---
        let updated = store.load(plugin_id).unwrap();
        assert_eq!(updated.version, "2.0.0");
        let new_diff = PluginPermissions::diff(&updated.permissions, &v2_perms);
        assert!(
            new_diff.is_empty(),
            "after saving v2 approval, diff should be empty"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// T41 edge case: shell escalation triggers re-prompt.
    #[test]
    fn t41_shell_escalation_triggers_reprompt() {
        let dir = std::env::temp_dir().join("clawft_t41_shell_escalation");
        let _ = std::fs::remove_dir_all(&dir);

        let store = PermissionStore::new(dir.clone());
        let plugin_id = "com.example.shell-upgrade";

        let v1_perms = PluginPermissions {
            shell: false,
            ..Default::default()
        };
        let v1_record = ApprovedRecord {
            version: "1.0.0".to_string(),
            permissions: v1_perms,
            approved_at: "2026-02-20T10:00:00Z".to_string(),
        };
        store.save(plugin_id, &v1_record).unwrap();

        let v2_perms = PluginPermissions {
            shell: true,
            ..Default::default()
        };

        let approved = store.load(plugin_id).unwrap();
        let diff = PluginPermissions::diff(&approved.permissions, &v2_perms);

        assert!(diff.shell_escalation, "shell false->true should be flagged");
        assert!(!diff.is_empty());

        // MockApprover denies
        let mock = MockApprover::new(false);
        let result = mock.approve(plugin_id, &diff);
        assert!(!result, "user denied the shell escalation");

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// T41 edge case: same version with identical permissions = no prompt.
    #[test]
    fn t41_identical_permissions_no_prompt_needed() {
        let perms = PluginPermissions {
            network: vec!["api.example.com".into()],
            filesystem: vec!["/data".into()],
            env_vars: vec!["TOKEN".into()],
            shell: true,
        };

        let diff = PluginPermissions::diff(&perms, &perms);
        assert!(diff.is_empty(), "identical perms should produce empty diff");
    }
}
