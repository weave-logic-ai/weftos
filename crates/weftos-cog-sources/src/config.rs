//! Cog source configuration: `[[cog_source]]` and `[[cog_licence]]` tables.
//!
//! One file shape serves two places:
//!
//! - project: `<project root>/.weftos/cog-sources.toml`
//! - user default: `~/.weftos/cog-sources.toml`
//!
//! The effective list for a project is the user default overlaid with the
//! project file (same `name` => the project entry replaces the user one).
//! The file holds public keys, URLs and licence declarations. It never holds
//! a private key or a secret.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use weftos_cog_repo::WEAVELOGIC_PUBKEY_HEX;

use crate::error::{Result, SourceError};

/// File name of a sources file.
pub const SOURCES_FILE: &str = "cog-sources.toml";

/// Where Cognitum publishes `app-registry.json`.
pub const COGNITUM_DEFAULT_URL: &str = "https://storage.googleapis.com/cognitum-apps/app-registry.json";

/// What a source is, which decides how its cogs are trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// WeftOS / WeaveLogic COG-008 signed registry. Keys default to the
    /// pinned WeaveLogic release key plus the compiled-in WeftOS signers.
    Weftos,
    /// Cognitum `app-registry.json`: listable always, installable only with
    /// a licence entitlement. Binaries are sha256-checked, not signed.
    Cognitum,
    /// A project's own COG-008 registry signed by a key the project pins.
    Private,
}

impl SourceKind {
    /// Lower-case label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Weftos => "weftos",
            Self::Cognitum => "cognitum",
            Self::Private => "private",
        }
    }
}

/// One `[[cog_source]]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CogSource {
    /// Namespace: `[a-z0-9][a-z0-9_-]{0,31}`. Used as `name:cog-id`.
    pub name: String,
    /// Source kind.
    pub kind: SourceKind,
    /// `registry.json` / `app-registry.json` URL or file path, or the
    /// repository directory (a COG-008 repo directory holds `registry.json`).
    pub url: String,
    /// Ed25519 public keys (64 hex) that may sign this source's binaries.
    /// Required for `private`. Refused on `weftos`: the WeftOS trust anchors are
    /// compiled in, and a project file must not be able to add its own key to
    /// them (put your own key in a `private` source). Accepted but unused today
    /// on `cognitum` (reserved for the optional release-record verifier).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_keys: Vec<String>,
    /// Higher wins when a bare id is listed by several enabled sources.
    #[serde(default)]
    pub priority: i32,
    /// Disabled sources are skipped by search, resolution and install.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Development and tests only: let a `cognitum` source use a local path or
    /// `http://`, for the registry and for the binaries it lists. Cognitum
    /// binaries are trusted by sha256 alone, so over plain http the hash and
    /// the binary can both be replaced in transit.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_insecure: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn yes() -> bool {
    true
}

impl CogSource {
    /// Keys that can verify this source's signed artifacts. For `weftos`
    /// that is the WeaveLogic release key + `extra_weftos` (the caller passes
    /// the compiled-in `WEFTOS_PINNED_SIGNERS` keys; this crate does not link
    /// the kernel). Nothing from the config file is added to it. For
    /// `private` it is `pinned_keys` only: the WeaveLogic key is never
    /// implicitly trusted by a private source. For `cognitum` it is empty
    /// (binaries are not signed).
    pub fn effective_keys(&self, extra_weftos: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |k: &str| {
            let k = k.trim().to_ascii_lowercase();
            if !out.contains(&k) {
                out.push(k);
            }
        };
        match self.kind {
            SourceKind::Weftos => {
                push(WEAVELOGIC_PUBKEY_HEX);
                extra_weftos.iter().for_each(|k| push(k));
            }
            SourceKind::Private => self.pinned_keys.iter().for_each(|k| push(k)),
            SourceKind::Cognitum => {}
        }
        out
    }

    fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(SourceError::Config(format!("source '{}': {m}", self.name)));
        if !valid_name(&self.name) {
            return Err(SourceError::Config(format!(
                "bad source name {:?} (use [a-z0-9][a-z0-9_-]{{0,31}}, no ':')",
                self.name
            )));
        }
        if self.url.trim().is_empty() || self.url.chars().any(|c| c.is_control()) {
            return bad("url is empty or has control characters".into());
        }
        for k in &self.pinned_keys {
            if !valid_pubkey(k) {
                return bad(format!("pinned key {k:?} is not a 64-hex Ed25519 public key"));
            }
        }
        if self.kind == SourceKind::Weftos && !self.pinned_keys.is_empty() {
            return bad("a weftos source cannot carry pinned_keys: the WeftOS anchors are compiled in. Put your own key in a `private` source".into());
        }
        if self.kind == SourceKind::Cognitum && !self.allow_insecure && !self.url.starts_with("https://") {
            return bad("a cognitum source must use an https:// url (its binaries are trusted by sha256 alone); allow_insecure = true is for development only".into());
        }
        if self.kind == SourceKind::Private && self.pinned_keys.is_empty() {
            return bad("a private source must pin at least one key (the project's own signing key)".into());
        }
        Ok(())
    }
}

/// Which cogs a licence covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LicensedCogs {
    /// The literal string `"all"`.
    All(String),
    /// An explicit list of cog ids.
    List(Vec<String>),
}

/// One `[[cog_licence]]`: a declared entitlement, not a payment check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CogLicence {
    /// Name of the source this licence is for (usually `cognitum`).
    pub source: String,
    /// `"all"` or a list of cog ids.
    pub cogs: LicensedCogs,
    /// Licensee account label (informational; recorded in provenance).
    #[serde(default)]
    pub account: String,
    /// `YYYY-MM-DD` (valid through the end of that UTC day) or RFC 3339.
    /// Absent means no expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
}

/// Contents of one sources file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcesFile {
    /// `[[cog_source]]` entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cog_source: Vec<CogSource>,
    /// `[[cog_licence]]` entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cog_licence: Vec<CogLicence>,
}

fn valid_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

/// A cog id as the catalog and `cogpkg` use it.
pub fn valid_cog_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// True when `key_hex` is the WeaveLogic release key or one of `extra_weftos`
/// (the compiled-in WeftOS signers): the only keys that may label a cog WeaveLogic.
pub fn is_weftos_anchor(key_hex: &str, extra_weftos: &[String]) -> bool {
    let k = key_hex.trim().to_ascii_lowercase();
    k == WEAVELOGIC_PUBKEY_HEX || extra_weftos.iter().any(|e| e.trim().to_ascii_lowercase() == k)
}

/// 64 hex chars that decode to a valid Ed25519 point.
pub fn valid_pubkey(k: &str) -> bool {
    let Ok(raw) = hex::decode(k.trim()) else { return false };
    let Ok(raw): std::result::Result<[u8; 32], _> = raw.try_into() else { return false };
    ed25519_dalek::VerifyingKey::from_bytes(&raw).is_ok()
}

impl CogLicence {
    fn validate(&self) -> Result<()> {
        if !valid_name(&self.source) {
            return Err(SourceError::Config(format!("licence: bad source name {:?}", self.source)));
        }
        match &self.cogs {
            LicensedCogs::All(s) if s == "all" => {}
            LicensedCogs::All(s) => {
                return Err(SourceError::Config(format!("licence for '{}': cogs must be \"all\" or a list, got {s:?}", self.source)))
            }
            LicensedCogs::List(v) => {
                if v.is_empty() {
                    return Err(SourceError::Config(format!("licence for '{}': empty cogs list", self.source)));
                }
                if let Some(bad) = v.iter().find(|c| !valid_cog_id(c)) {
                    return Err(SourceError::Config(format!("licence for '{}': bad cog id {bad:?}", self.source)));
                }
            }
        }
        if let Some(e) = &self.expires {
            crate::licence::parse_expiry(e)?;
        }
        Ok(())
    }
}

impl SourcesFile {
    /// Parse and validate TOML text.
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > 256 * 1024 {
            return Err(SourceError::Config("sources file larger than 256 KiB".into()));
        }
        let f: Self = toml::from_str(text).map_err(|e| SourceError::Config(e.to_string()))?;
        f.validate()?;
        Ok(f)
    }

    /// Validate names, keys, uniqueness and licences.
    pub fn validate(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for s in &self.cog_source {
            s.validate()?;
            if !seen.insert(s.name.clone()) {
                return Err(SourceError::Config(format!("duplicate source name '{}'", s.name)));
            }
        }
        self.cog_licence.iter().try_for_each(CogLicence::validate)
    }

    /// Serialize to TOML.
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| SourceError::Config(e.to_string()))
    }

    /// Read a sources file; a missing file is an empty config.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(t) => Self::parse(&t).map_err(|e| match e {
                SourceError::Config(m) => SourceError::Config(format!("{}: {m}", path.display())),
                e => e,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(SourceError::Io { path: path.display().to_string(), msg: e.to_string() }),
        }
    }

    /// Validate, then write atomically (temp file + rename), mode 0644.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let io = |e: std::io::Error| SourceError::Io { path: path.display().to_string(), msg: e.to_string() };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let tmp = path.with_extension(format!("toml.{}.{nanos}.tmp", std::process::id()));
        std::fs::write(&tmp, self.to_toml()?).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(e)
        })
    }

    /// Add a source; refuses a duplicate name.
    pub fn add_source(&mut self, s: CogSource) -> Result<()> {
        if self.cog_source.iter().any(|x| x.name == s.name) {
            return Err(SourceError::Config(format!("source '{}' already exists", s.name)));
        }
        s.validate()?;
        self.cog_source.push(s);
        Ok(())
    }

    /// Remove a source (and nothing else; licences stay).
    pub fn remove_source(&mut self, name: &str) -> Result<()> {
        let n = self.cog_source.len();
        self.cog_source.retain(|s| s.name != name);
        if self.cog_source.len() == n {
            return Err(SourceError::UnknownSource(name.into()));
        }
        Ok(())
    }

    /// Enable or disable a source.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<()> {
        let s = self
            .cog_source
            .iter_mut()
            .find(|s| s.name == name)
            .ok_or_else(|| SourceError::UnknownSource(name.into()))?;
        s.enabled = enabled;
        Ok(())
    }

    /// Add a licence; a licence for the same source and exact cog coverage
    /// replaces the earlier one.
    pub fn add_licence(&mut self, l: CogLicence) -> Result<()> {
        l.validate()?;
        self.cog_licence.retain(|x| !(x.source == l.source && x.cogs == l.cogs));
        self.cog_licence.push(l);
        Ok(())
    }

    /// Remove every licence for `source`; errors when none exists.
    pub fn remove_licences(&mut self, source: &str) -> Result<()> {
        let n = self.cog_licence.len();
        self.cog_licence.retain(|l| l.source != source);
        if self.cog_licence.len() == n {
            return Err(SourceError::Config(format!("no licence recorded for '{source}'")));
        }
        Ok(())
    }
}

/// The merged view: user default overlaid with the project file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveSources {
    /// Sources sorted by name.
    pub sources: Vec<CogSource>,
    /// Licences: project entries first, then user entries.
    pub licences: Vec<CogLicence>,
    /// Names of sources whose effective entry comes from the project file.
    pub project_names: BTreeSet<String>,
    /// Things the operator should see: a project entry replacing a user entry
    /// and changing its url or keys, and plain-http sources.
    pub warnings: Vec<String>,
}

impl EffectiveSources {
    /// Overlay `project` on `user`: a project source replaces a user source
    /// of the same name (including to disable it). Use [`check_overrides`]
    /// (done by [`load_effective`]) to refuse a changed kind.
    pub fn merge(user: &SourcesFile, project: &SourcesFile) -> Self {
        let mut sources: Vec<CogSource> = project.cog_source.clone();
        let mut warnings = Vec::new();
        // `allow_insecure` is honoured only from the user's own file: a cloned project must not
        // be able to re-open http, file:// or local paths for a Cognitum source.
        for s in sources.iter_mut().filter(|s| s.allow_insecure) {
            s.allow_insecure = false;
            warnings.push(format!("ignoring allow_insecure on source '{}' in this project's cog-sources.toml (only your user file may set it)", s.name));
        }
        for u in &user.cog_source {
            match project.cog_source.iter().find(|p| p.name == u.name) {
                None => sources.push(u.clone()),
                Some(p) => {
                    let mut changed = Vec::new();
                    if p.url != u.url {
                        changed.push(format!("url {} -> {}", u.url, p.url));
                    }
                    if p.pinned_keys != u.pinned_keys {
                        changed.push("pinned keys".to_string());
                    }
                    if p.kind != u.kind {
                        changed.push(format!("kind {} -> {}", u.kind.label(), p.kind.label()));
                    }
                    if !changed.is_empty() {
                        warnings.push(format!(
                            "this project's cog-sources.toml replaces your user source '{}' and changes its {}",
                            u.name,
                            changed.join(", ")
                        ));
                    }
                }
            }
        }
        sources.sort_by(|a, b| a.name.cmp(&b.name));
        for s in &sources {
            if s.allow_insecure {
                warnings.push(format!("source '{}' has allow_insecure in effect: {} is accepted without https (development only)", s.name, s.url));
            }
            if s.url.starts_with("http://") {
                warnings.push(format!("source '{}' uses plain http://; registry and binaries can be altered in transit (signed sources still verify signatures)", s.name));
            }
        }
        let mut licences = project.cog_licence.clone();
        licences.extend(user.cog_licence.iter().cloned());
        let project_names = project.cog_source.iter().map(|s| s.name.clone()).collect();
        Self { sources, licences, project_names, warnings }
    }

    /// True when the effective entry for `name` came from the project file.
    pub fn from_project(&self, name: &str) -> bool {
        self.project_names.contains(name)
    }

    /// Enabled sources only.
    pub fn enabled(&self) -> impl Iterator<Item = &CogSource> {
        self.sources.iter().filter(|s| s.enabled)
    }

    /// Source by name.
    pub fn source(&self, name: &str) -> Option<&CogSource> {
        self.sources.iter().find(|s| s.name == name)
    }
}

/// Refuse a project entry that replaces a user entry with a different kind:
/// a clone must not be able to turn the user's `weftos` source into something else.
pub fn check_overrides(user: &SourcesFile, project: &SourcesFile) -> Result<()> {
    for u in &user.cog_source {
        if let Some(p) = project.cog_source.iter().find(|p| p.name == u.name)
            && p.kind != u.kind
        {
            return Err(SourceError::Config(format!(
                "the project file redefines your user source '{}' with a different kind ({} -> {}); rename it",
                u.name,
                u.kind.label(),
                p.kind.label()
            )));
        }
    }
    Ok(())
}

/// `<project root>/.weftos/cog-sources.toml`.
pub fn project_sources_path(project_root: &Path) -> PathBuf {
    project_root.join(".weftos").join(SOURCES_FILE)
}

/// `<home>/.weftos/cog-sources.toml`.
pub fn user_sources_path(home: &Path) -> PathBuf {
    home.join(".weftos").join(SOURCES_FILE)
}

/// Load the merged config. Both paths are explicit so tests and callers
/// choose the roots; a missing file counts as empty.
pub fn load_effective(user_file: Option<&Path>, project_file: Option<&Path>) -> Result<EffectiveSources> {
    let user = user_file.map(SourcesFile::load).transpose()?.unwrap_or_default();
    let project = project_file.map(SourcesFile::load).transpose()?.unwrap_or_default();
    check_overrides(&user, &project)?;
    Ok(EffectiveSources::merge(&user, &project))
}
