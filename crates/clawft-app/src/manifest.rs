//! App manifest schema — ADR-015 §Schema.
//!
//! The manifest is the declaration; the interpreters (surface format,
//! adapter contract, narration rule language) live in sibling ADRs
//! (016 / 017 / 019) and their crates. Fields here match the TOML
//! schema in ADR-015 §Schema; enum variants use the wire names from
//! the ADR (e.g. `"single-app"` → [`Mode::SingleApp`]).

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};

/// Presentation mode declared by an app.
///
/// ADR-015 §Schema (2): `supported_modes` is a non-empty subset of
/// `{single-app, desktop, ide}`. Session 10 §2.1 establishes that
/// `ide` is a superset of `desktop` semantically, but structurally
/// the manifest lists them as peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Locked kiosk: exactly one top-level surface, no escape.
    SingleApp,
    /// Full chrome (wallpaper, launcher, tray, windowed surfaces).
    Desktop,
    /// Desktop + IDE bridge module (see ADR-018).
    Ide,
}

/// Interaction modality declared by an app.
///
/// ADR-015 §Schema (2): `supported_inputs` is a non-empty subset of
/// `{pointer, touch, voice, hybrid}`. See ADR-019 for how the
/// compositor adapts canon rendering per input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Input {
    Pointer,
    Touch,
    Voice,
    /// Multiple channels active simultaneously.
    Hybrid,
}

/// How a host launches this app (ADR-015 §Schema (3)).
///
/// The `kind` tag is the discriminant; each variant owns its payload
/// fields. The set is deliberately open — new hosts (AR, car,
/// voice-only speakers) add new variants as additive minor schema
/// bumps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EntryPoint {
    /// `weaver gui --app <flag>` — the CLI path.
    Cli {
        flag: String,
    },
    /// A VSCode / Cursor command id, e.g. `weft.admin.open`.
    VscodeCommand {
        command: String,
    },
    /// Voice wake-word (requires [`Input::Voice`] in
    /// `supported_inputs`; see ADR-015 validation rule 7).
    WakeWord {
        phrase: String,
    },
}

/// A reference to a surface description.
///
/// For M1.5 we keep this as a raw string — path for declarative TOML
/// apps, Rust type id for crate-form apps (ADR-015 §Authoring
/// surfaces). The `clawft-surface` crate (M1.5-B) owns the actual
/// resolution and SurfaceTree IR (ADR-016).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SurfaceRef(pub String);

impl SurfaceRef {
    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SurfaceRef {
    fn from(s: &str) -> Self {
        SurfaceRef(s.to_string())
    }
}

impl From<String> for SurfaceRef {
    fn from(s: String) -> Self {
        SurfaceRef(s)
    }
}

/// A capture-channel permission request.
///
/// ADR-015 §Schema (7) and ADR-012 capture-channel grammar:
///
/// * `camera` / `mic` / `screen` — coarse channels
/// * `fs:<path-prefix>` — filesystem read scoped to a path
/// * `net:<domain>` — network egress to a single domain
///
/// The full governance gate (ADR-012) resolves each entry at install
/// time; this crate only records the declaration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Permission {
    Camera,
    Mic,
    Screen,
    FsPath(String),
    NetDomain(String),
}

impl Permission {
    /// Parse ADR-012 capture-channel grammar from a string token.
    pub fn parse(token: &str) -> Result<Self, PermissionParseError> {
        if let Some(rest) = token.strip_prefix("fs:") {
            if rest.is_empty() {
                return Err(PermissionParseError::EmptyPath);
            }
            return Ok(Permission::FsPath(rest.to_string()));
        }
        if let Some(rest) = token.strip_prefix("net:") {
            if rest.is_empty() {
                return Err(PermissionParseError::EmptyDomain);
            }
            return Ok(Permission::NetDomain(rest.to_string()));
        }
        match token {
            "camera" => Ok(Permission::Camera),
            "mic" => Ok(Permission::Mic),
            "screen" => Ok(Permission::Screen),
            other => Err(PermissionParseError::Unknown(other.to_string())),
        }
    }

    /// Serialise back to the ADR-012 wire form.
    pub fn to_token(&self) -> String {
        match self {
            Permission::Camera => "camera".to_string(),
            Permission::Mic => "mic".to_string(),
            Permission::Screen => "screen".to_string(),
            Permission::FsPath(p) => format!("fs:{p}"),
            Permission::NetDomain(d) => format!("net:{d}"),
        }
    }
}

/// Errors from parsing a [`Permission`] token.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PermissionParseError {
    #[error("unknown permission token: {0}")]
    Unknown(String),
    #[error("fs: permission requires a non-empty path prefix")]
    EmptyPath,
    #[error("net: permission requires a non-empty domain")]
    EmptyDomain,
}

impl Serialize for Permission {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_token())
    }
}

impl<'de> Deserialize<'de> for Permission {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Permission::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// The full ADR-015 manifest.
///
/// Construct via [`AppManifest::from_toml_str`] (TOML form) or
/// directly in Rust for crate-form apps. Validation lives in
/// [`crate::validation::validate`]; parsing here is schema-shape
/// only.
///
/// `Eq` is intentionally omitted — `surface_states` carries an
/// opaque `toml::Value` which does not implement `Eq` (TOML floats
/// are `PartialEq` only). `PartialEq` is sufficient for round-trip
/// tests; no consumer collects manifests into a hash-based set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppManifest {
    /// `app://<dotted-path>` IRI.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Semver (ADR-015 §Versioning).
    pub version: Version,
    /// Display icon — path relative to the manifest or an IRI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

    /// Non-empty subset of [`Mode`].
    pub supported_modes: Vec<Mode>,
    /// Non-empty subset of [`Input`].
    pub supported_inputs: Vec<Input>,

    /// How a host launches this app.
    #[serde(default)]
    pub entry_points: Vec<EntryPoint>,

    /// Surface refs — file paths, Rust type ids, or inline handles.
    ///
    /// TOML key is `surface_refs` (renamed from `surfaces`) so the
    /// `[surfaces.empty_state]` / `[surfaces.loading_state]` /
    /// `[surfaces.offline_state]` D-EM01 state sections required by
    /// DESIGN.md §5 can coexist with the refs list. The `surfaces`
    /// dotted-table tree is parsed opaquely into [`Self::surface_states`].
    #[serde(default, rename = "surface_refs")]
    pub surfaces: Vec<SurfaceRef>,

    /// D-EM01 state surface descriptions (DESIGN.md §5) — keyed by
    /// `empty_state` / `loading_state` / `offline_state` under the
    /// `[surfaces]` parent table in the manifest TOML. Opaque
    /// `toml::Value` for now: the surface composer reads them at
    /// render time once 0.8.x wires them up. Today they exist to
    /// satisfy `audit-surface.sh` D-EM01 and document the contract.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "surfaces")]
    pub surface_states: Option<toml::Value>,

    /// Ontology topic paths this app reads.
    #[serde(default)]
    pub subscriptions: Vec<String>,

    /// WSP verb names this app may invoke.
    #[serde(default)]
    pub influences: Vec<String>,

    /// Capture-channel permission requests.
    #[serde(default)]
    pub permissions: Vec<Permission>,

    /// Optional narration contract (ADR-015 §Schema (8), ADR-019
    /// narration rule language). Keyed by a topic path that must
    /// appear in `subscriptions`; values are speakable templates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narration: Option<BTreeMap<String, String>>,
}

impl AppManifest {
    /// Parse a manifest from its canonical TOML form.
    ///
    /// Returns a schema-shape error only; semantic validation is a
    /// separate step (see [`crate::validation::validate`]).
    pub fn from_toml_str(src: &str) -> Result<Self, ManifestParseError> {
        toml::from_str(src).map_err(ManifestParseError::Toml)
    }

    /// Serialise to canonical TOML.
    pub fn to_toml_string(&self) -> Result<String, ManifestParseError> {
        toml::to_string(self).map_err(ManifestParseError::Serialize)
    }
}

/// Errors from parsing or serialising an [`AppManifest`].
#[derive(Debug, thiserror::Error)]
pub enum ManifestParseError {
    #[error("failed to deserialize TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("failed to serialize TOML: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ADR-015 §concrete-example manifest, minus the `[narration]`
    /// table. Used by several tests in this module.
    pub(crate) const ADMIN_FIXTURE: &str =
        include_str!("../fixtures/weftos-admin.toml");

    #[test]
    fn parses_weftos_admin_fixture() {
        let m = AppManifest::from_toml_str(ADMIN_FIXTURE)
            .expect("fixture must parse");
        assert_eq!(m.id, "app://weftos.admin");
        assert_eq!(m.name, "WeftOS Admin");
        assert_eq!(m.version, Version::new(0, 1, 0));
        assert_eq!(m.supported_modes.len(), 3);
        assert!(m.supported_modes.contains(&Mode::Ide));
        assert!(m.supported_modes.contains(&Mode::Desktop));
        assert!(m.supported_modes.contains(&Mode::SingleApp));
        assert_eq!(m.supported_inputs.len(), 3);
        assert!(m.supported_inputs.contains(&Input::Voice));
        assert_eq!(m.entry_points.len(), 3);
        assert!(matches!(
            &m.entry_points[0],
            EntryPoint::Cli { flag } if flag == "admin"
        ));
        assert!(matches!(
            &m.entry_points[1],
            EntryPoint::VscodeCommand { command } if command == "weft.admin.open"
        ));
        assert!(matches!(
            &m.entry_points[2],
            EntryPoint::WakeWord { phrase } if phrase == "weft, admin status"
        ));
        assert_eq!(m.surfaces.len(), 2);
        assert_eq!(
            m.surfaces[0].as_str(),
            "surfaces/admin-main.toml"
        );
        assert_eq!(m.subscriptions.len(), 4);
        assert_eq!(m.influences.len(), 3);
        assert_eq!(m.permissions.len(), 1);
        assert_eq!(
            m.permissions[0],
            Permission::FsPath("/var/log/weftos".to_string())
        );
        let narration = m.narration.as_ref().expect("fixture has narration");
        assert_eq!(narration.len(), 2);
        assert!(narration.contains_key("substrate/kernel/services"));
    }

    #[test]
    fn manifest_roundtrips_through_toml() {
        // TOML can't round-trip a plain top-level table with a
        // `[narration]` child after regular fields mechanically, so
        // we construct the manifest in Rust and check the string
        // round-trips shape-equivalently.
        let mut narration = BTreeMap::new();
        narration.insert(
            "substrate/kernel/status".to_string(),
            "ok".to_string(),
        );
        let m = AppManifest {
            id: "app://weftos.admin".to_string(),
            name: "WeftOS Admin".to_string(),
            version: Version::new(0, 1, 0),
            icon: Some("assets/admin.svg".to_string()),
            supported_modes: vec![Mode::Desktop, Mode::Ide],
            supported_inputs: vec![Input::Pointer, Input::Voice],
            entry_points: vec![EntryPoint::Cli {
                flag: "admin".to_string(),
            }],
            surfaces: vec![SurfaceRef::from("surfaces/admin-main.toml")],
            surface_states: None,
            subscriptions: vec!["substrate/kernel/status".to_string()],
            influences: vec!["kernel.restart-service".to_string()],
            permissions: vec![Permission::FsPath("/var/log".to_string())],
            narration: Some(narration),
        };
        let serialised = m
            .to_toml_string()
            .expect("serialize manifest back to TOML");
        let reparsed = AppManifest::from_toml_str(&serialised)
            .expect("reparse self-serialized TOML");
        assert_eq!(m, reparsed);
    }

    #[test]
    fn permission_token_grammar() {
        assert_eq!(Permission::parse("camera").unwrap(), Permission::Camera);
        assert_eq!(Permission::parse("mic").unwrap(), Permission::Mic);
        assert_eq!(Permission::parse("screen").unwrap(), Permission::Screen);
        assert_eq!(
            Permission::parse("fs:/tmp").unwrap(),
            Permission::FsPath("/tmp".to_string())
        );
        assert_eq!(
            Permission::parse("net:api.github.com").unwrap(),
            Permission::NetDomain("api.github.com".to_string())
        );
        assert!(matches!(
            Permission::parse("bogus"),
            Err(PermissionParseError::Unknown(_))
        ));
        assert_eq!(
            Permission::parse("fs:"),
            Err(PermissionParseError::EmptyPath)
        );
        assert_eq!(
            Permission::parse("net:"),
            Err(PermissionParseError::EmptyDomain)
        );
    }
}
