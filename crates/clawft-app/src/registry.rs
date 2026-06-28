//! Local app registry — JSON-backed for M1.5.
//!
//! ADR-015 §Install specifies a SQLite table; for M1.5 the ROADMAP
//! and session-10 §7 acceptance explicitly allow a JSON file as a
//! first cut (SQLite lands with migrations in M1.6+). The schema
//! matches ADR-015 §Install step 6 fields in spirit: manifest,
//! `installed_at`, enabled state. Manifest-hash / consent-id / cached
//! compiled-surfaces columns are left for the SQLite migration.
//!
//! Default path:
//!
//! * `$XDG_DATA_HOME/weftos/apps.json` if set
//! * otherwise `~/.weftos/apps.json`

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
// `std::time` panics under wasm32-unknown-unknown; `web-time` is a
// drop-in that uses std on native and `performance.now()` in the
// browser.
use web_time::{SystemTime, UNIX_EPOCH};

use crate::manifest::AppManifest;
use crate::validation::{ValidationError, validate};

/// A row in the local app registry.
///
/// `Eq` is intentionally omitted — see [`AppManifest`] for the reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledApp {
    /// The full parsed manifest.
    pub manifest: AppManifest,
    /// Unix epoch seconds when `install()` was called.
    pub installed_at: u64,
    /// `false` means the app is registered but not launchable (e.g.
    /// missing-adapter dependency error per ADR-015 rule 10; owner
    /// turned it off manually).
    pub enabled: bool,
}

/// The registry file's on-disk schema. Kept as a struct for forward
/// compat — adding fields is an additive change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    apps: Vec<InstalledApp>,
}

/// JSON-backed persistent app registry.
///
/// Single-process, single-writer. Thread-safety is the caller's
/// problem (wrap in a `Mutex` if you need it). Every mutation calls
/// [`AppRegistry::save`] before returning, so a crash mid-call at
/// worst loses the most-recent op.
#[derive(Debug, Clone)]
pub struct AppRegistry {
    path: PathBuf,
    apps: Vec<InstalledApp>,
}

impl AppRegistry {
    /// Build a registry rooted at `path`. Does **not** read the file
    /// — call [`Self::load`] for that. (`new` is infallible so tests
    /// can construct registries against paths that don't exist yet.)
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            apps: Vec::new(),
        }
    }

    /// Resolve the default registry path:
    /// `$XDG_DATA_HOME/weftos/apps.json` or `~/.weftos/apps.json`.
    pub fn default_path() -> Result<PathBuf, RegistryError> {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            let p = PathBuf::from(xdg);
            if !p.as_os_str().is_empty() {
                return Ok(p.join("weftos").join("apps.json"));
            }
        }
        // `dirs` is native-only in this workspace; we fall back to
        // $HOME manually to stay minimal-deps on this new crate.
        if let Some(home) = std::env::var_os("HOME") {
            return Ok(PathBuf::from(home).join(".weftos").join("apps.json"));
        }
        Err(RegistryError::NoHomeDir)
    }

    /// Load the registry contents from disk. Missing file is treated
    /// as an empty registry (first-run is not an error).
    pub fn load(&mut self) -> Result<(), RegistryError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => {
                let parsed: RegistryFile = serde_json::from_str(&text)?;
                self.apps = parsed.apps;
                Ok(())
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                self.apps.clear();
                Ok(())
            }
            Err(err) => Err(RegistryError::Io(err)),
        }
    }

    /// Persist to disk, creating parent directories as needed. Writes
    /// via a temp file + rename to keep the on-disk state coherent
    /// through process death.
    pub fn save(&self) -> Result<(), RegistryError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = RegistryFile {
            apps: self.apps.clone(),
        };
        let body = serde_json::to_string_pretty(&file)?;
        let tmp = tmp_sibling(&self.path);
        fs::write(&tmp, body)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Install a manifest. Validates structurally (ADR-015 rules
    /// 1–9); rejects duplicates by `id`. Saves on success.
    pub fn install(&mut self, manifest: AppManifest) -> Result<&InstalledApp, RegistryError> {
        validate(&manifest)?;
        if self.apps.iter().any(|a| a.manifest.id == manifest.id) {
            return Err(RegistryError::AlreadyInstalled { id: manifest.id });
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.apps.push(InstalledApp {
            manifest,
            installed_at: now,
            enabled: true,
        });
        self.save()?;
        Ok(self.apps.last().expect("just-pushed app must exist"))
    }

    /// Remove the installed app with `id`. Returns the removed row or
    /// `NotFound`.
    pub fn uninstall(&mut self, id: &str) -> Result<InstalledApp, RegistryError> {
        let idx = self
            .apps
            .iter()
            .position(|a| a.manifest.id == id)
            .ok_or_else(|| RegistryError::NotFound { id: id.to_string() })?;
        let removed = self.apps.remove(idx);
        self.save()?;
        Ok(removed)
    }

    /// List all installed apps in install order.
    pub fn list(&self) -> &[InstalledApp] {
        &self.apps
    }

    /// Get one installed app by id.
    pub fn get(&self, id: &str) -> Option<&InstalledApp> {
        self.apps.iter().find(|a| a.manifest.id == id)
    }

    /// Enable an app (makes it launchable).
    pub fn enable(&mut self, id: &str) -> Result<(), RegistryError> {
        self.set_enabled(id, true)
    }

    /// Disable an app without uninstalling it.
    pub fn disable(&mut self, id: &str) -> Result<(), RegistryError> {
        self.set_enabled(id, false)
    }

    fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), RegistryError> {
        let app = self
            .apps
            .iter_mut()
            .find(|a| a.manifest.id == id)
            .ok_or_else(|| RegistryError::NotFound { id: id.to_string() })?;
        app.enabled = enabled;
        self.save()?;
        Ok(())
    }

    /// Borrow the backing-file path (useful for diagnostics/tests).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn tmp_sibling(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

/// Errors surfaced by [`AppRegistry`] operations.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("app `{id}` is already installed")]
    AlreadyInstalled { id: String },
    #[error("app `{id}` is not installed")]
    NotFound { id: String },
    #[error("manifest failed validation: {0}")]
    Invalid(#[from] ValidationError),
    #[error("could not resolve a home directory for the default registry path")]
    NoHomeDir,
    #[error("registry I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("registry JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use semver::Version;
    use tempfile::tempdir;

    use super::*;
    use crate::manifest::{AppManifest, EntryPoint, Input, Mode, SurfaceRef};

    fn mk_manifest(id: &str) -> AppManifest {
        AppManifest {
            id: id.to_string(),
            name: "Test App".to_string(),
            version: Version::new(0, 1, 0),
            icon: None,
            supported_modes: vec![Mode::Desktop],
            supported_inputs: vec![Input::Pointer],
            entry_points: vec![EntryPoint::Cli {
                flag: "test".to_string(),
            }],
            surfaces: vec![SurfaceRef::from("surfaces/main.toml")],
            surface_states: None,
            subscriptions: vec![],
            influences: vec![],
            permissions: vec![],
            narration: None,
        }
    }

    #[test]
    fn install_list_get_uninstall_roundtrip() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("apps.json");

        let mut reg = AppRegistry::new(&path);
        reg.load().expect("load empty is ok");
        assert!(reg.list().is_empty());

        let m = mk_manifest("app://weftos.admin");
        reg.install(m.clone()).expect("install");
        assert_eq!(reg.list().len(), 1);
        let got = reg.get("app://weftos.admin").expect("get");
        assert_eq!(got.manifest, m);
        assert!(got.enabled);

        // Persistence: load into a fresh registry from the same path.
        let mut reg2 = AppRegistry::new(&path);
        reg2.load().expect("load from disk");
        assert_eq!(reg2.list().len(), 1);
        assert_eq!(
            reg2.get("app://weftos.admin").map(|a| &a.manifest),
            Some(&m)
        );

        // Uninstall returns the row and removes it.
        let removed = reg2.uninstall("app://weftos.admin").expect("uninstall");
        assert_eq!(removed.manifest, m);
        assert!(reg2.list().is_empty());

        // And persists again.
        let mut reg3 = AppRegistry::new(&path);
        reg3.load().expect("load after uninstall");
        assert!(reg3.list().is_empty());
    }

    #[test]
    fn install_rejects_duplicate_id() {
        let dir = tempdir().unwrap();
        let mut reg = AppRegistry::new(dir.path().join("apps.json"));
        reg.load().unwrap();
        reg.install(mk_manifest("app://dup")).unwrap();
        let err = reg.install(mk_manifest("app://dup")).unwrap_err();
        assert!(matches!(err, RegistryError::AlreadyInstalled { .. }));
    }

    #[test]
    fn install_rejects_invalid_manifest() {
        let dir = tempdir().unwrap();
        let mut reg = AppRegistry::new(dir.path().join("apps.json"));
        reg.load().unwrap();
        let mut bad = mk_manifest("app://bad");
        bad.supported_modes.clear();
        let err = reg.install(bad).unwrap_err();
        assert!(matches!(err, RegistryError::Invalid(_)));
    }

    #[test]
    fn enable_disable_toggle_persists() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("apps.json");
        let mut reg = AppRegistry::new(&path);
        reg.load().unwrap();
        reg.install(mk_manifest("app://toggle")).unwrap();
        reg.disable("app://toggle").unwrap();
        assert!(!reg.get("app://toggle").unwrap().enabled);

        let mut reg2 = AppRegistry::new(&path);
        reg2.load().unwrap();
        assert!(!reg2.get("app://toggle").unwrap().enabled);

        reg2.enable("app://toggle").unwrap();
        assert!(reg2.get("app://toggle").unwrap().enabled);
    }

    #[test]
    fn uninstall_missing_returns_not_found() {
        let dir = tempdir().unwrap();
        let mut reg = AppRegistry::new(dir.path().join("apps.json"));
        reg.load().unwrap();
        let err = reg.uninstall("app://nope").unwrap_err();
        assert!(matches!(err, RegistryError::NotFound { .. }));
    }
}
