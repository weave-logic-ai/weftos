//! Loading sources and resolving `source:cog-id` references.

use serde::Serialize;
use weftos_cog_market::CognitumRegistry;
use weftos_cog_repo::{Registry, SCHEMA};

use crate::config::{valid_cog_id, CogSource, EffectiveSources, SourceKind};
use crate::error::{Result, SourceError};
use crate::fetch::{join, parent, Reader, MAX_REGISTRY_BYTES};

/// A source's parsed listing.
#[derive(Debug, Clone)]
pub enum Listing {
    /// COG-008 registry (weftos and private sources).
    Signed {
        /// Directory or URL prefix artifact paths are relative to.
        base: String,
        /// The registry.
        registry: Registry,
    },
    /// Cognitum `app-registry.json`.
    Cognitum(CognitumRegistry),
}

/// One cog as a source lists it, normalised across kinds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceCog {
    /// Cog id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Version.
    pub version: String,
    /// Category.
    pub category: String,
    /// Description.
    pub description: String,
    /// `hardware_requirement` when the registry carries it (WeftOS / private only).
    pub hardware_requirement: Vec<String>,
    /// Arches with a binary: `arm`, `arm64`.
    pub arches: Vec<String>,
    /// Binary size in KiB.
    pub size_kb: Option<u64>,
    /// True when the artifact is Ed25519-signed (weftos / private).
    pub signed: bool,
}

/// A configured source with its listing.
#[derive(Debug, Clone)]
pub struct LoadedSource {
    /// The configuration entry.
    pub source: CogSource,
    /// Its listing.
    pub listing: Listing,
}

impl LoadedSource {
    /// Every cog the source lists, sorted by id.
    pub fn cogs(&self) -> Vec<SourceCog> {
        let mut v: Vec<SourceCog> = match &self.listing {
            Listing::Signed { registry, .. } => registry
                .cogs
                .iter()
                .map(|c| SourceCog {
                    id: c.id.clone(),
                    name: if c.name.is_empty() { c.id.clone() } else { c.name.clone() },
                    version: c.version.clone(),
                    category: c.category.clone(),
                    description: c.description.clone(),
                    hardware_requirement: c.hardware_requirement.clone(),
                    arches: c.artifacts.keys().cloned().collect(),
                    size_kb: c.artifacts.get("arm").or_else(|| c.artifacts.values().next()).map(|a| a.size / 1024),
                    signed: true,
                })
                .collect(),
            Listing::Cognitum(reg) => reg
                .cogs
                .iter()
                .map(|c| SourceCog {
                    id: c.id.clone(),
                    name: if c.name.is_empty() { c.id.clone() } else { c.name.clone() },
                    version: if c.version.is_empty() { "0.0.0".into() } else { c.version.clone() },
                    category: c.category.clone(),
                    description: c.description.clone(),
                    hardware_requirement: vec![],
                    arches: vec!["arm".into()],
                    size_kb: c.size_kb,
                    signed: false,
                })
                .collect(),
        };
        v.retain(|c| valid_cog_id(&c.id));
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// One cog by id.
    pub fn cog(&self, id: &str) -> Option<SourceCog> {
        self.cogs().into_iter().find(|c| c.id == id)
    }
}

/// Where a signed source's `registry.json` lives.
pub fn registry_location(url: &str) -> String {
    if url.ends_with(".json") {
        url.to_string()
    } else {
        join(url, "registry.json")
    }
}

/// Load one enabled-or-not source (the caller decides which to load).
pub fn load_source(src: &CogSource, reader: &dyn Reader) -> Result<LoadedSource> {
    if src.kind == SourceKind::Cognitum && !src.allow_insecure && !src.url.starts_with("https://") {
        return Err(SourceError::Config(format!(
            "source '{}': a cognitum registry location must be https:// (allow_insecure applies only from the user file)",
            src.name
        )));
    }
    let loc = match src.kind {
        SourceKind::Cognitum => src.url.clone(),
        _ => registry_location(&src.url),
    };
    let bytes = reader.read(&loc, MAX_REGISTRY_BYTES)?;
    let listing = match src.kind {
        SourceKind::Cognitum => Listing::Cognitum(
            CognitumRegistry::parse(&bytes).map_err(|msg| SourceError::Parse { what: loc.clone(), msg })?,
        ),
        _ => {
            let registry: Registry = serde_json::from_slice(&bytes)
                .map_err(|e| SourceError::Parse { what: loc.clone(), msg: e.to_string() })?;
            if registry.schema != SCHEMA {
                return Err(SourceError::Parse { what: loc, msg: format!("registry schema {} is not {SCHEMA}", registry.schema) });
            }
            Listing::Signed { base: parent(&loc), registry }
        }
    };
    Ok(LoadedSource { source: src.clone(), listing })
}

/// Result of loading every enabled source: one failure does not hide the rest.
#[derive(Debug, Default)]
pub struct LoadedSources {
    /// Sources that loaded.
    pub loaded: Vec<LoadedSource>,
    /// `(source name, error)` for the ones that did not.
    pub failures: Vec<(String, SourceError)>,
}

/// Load every enabled source.
pub fn load_all(eff: &EffectiveSources, reader: &dyn Reader) -> LoadedSources {
    let mut out = LoadedSources::default();
    for s in eff.enabled() {
        match load_source(s, reader) {
            Ok(l) => out.loaded.push(l),
            Err(e) => out.failures.push((s.name.clone(), e)),
        }
    }
    out
}

/// A parsed `[source:]cog-id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CogRef {
    /// Namespace, when given.
    pub source: Option<String>,
    /// Cog id.
    pub id: String,
}

/// Parse `cog-id` or `source:cog-id`.
pub fn parse_ref(s: &str) -> Result<CogRef> {
    let (source, id) = match s.split_once(':') {
        Some((src, id)) => (Some(src.to_string()), id),
        None => (None, s),
    };
    if !valid_cog_id(id) {
        return Err(SourceError::Config(format!("bad cog id {id:?} in {s:?}")));
    }
    if let Some(src) = &source
        && (src.is_empty() || id.contains(':'))
    {
        return Err(SourceError::Config(format!("bad cog reference {s:?} (use source:cog-id)")));
    }
    Ok(CogRef { source, id: id.to_string() })
}

/// The outcome of resolving a reference.
#[derive(Debug, Clone)]
pub struct Resolved<'a> {
    /// The source that supplies the cog.
    pub loaded: &'a LoadedSource,
    /// The cog as that source lists it.
    pub cog: SourceCog,
}

impl Resolved<'_> {
    /// `source:id`.
    pub fn namespaced(&self) -> String {
        format!("{}:{}", self.loaded.source.name, self.cog.id)
    }
}

/// Resolve a reference against the loaded sources.
///
/// Namespaced ids go to that source only. A bare id is tried in every
/// enabled source that lists it: the highest `priority` wins, and if two
/// sources tie at the top the answer is an explicit `Ambiguous` error rather
/// than a silent pick.
pub fn resolve<'a>(eff: &EffectiveSources, loaded: &'a [LoadedSource], r: &CogRef) -> Result<Resolved<'a>> {
    if let Some(name) = &r.source {
        let cfg = eff.source(name).ok_or_else(|| SourceError::UnknownSource(name.clone()))?;
        if !cfg.enabled {
            return Err(SourceError::SourceDisabled(name.clone()));
        }
        let l = loaded
            .iter()
            .find(|l| &l.source.name == name)
            .ok_or_else(|| SourceError::NotFound { source_name: Some(name.clone()), id: r.id.clone() })?;
        let cog = l.cog(&r.id).ok_or_else(|| SourceError::NotFound { source_name: Some(name.clone()), id: r.id.clone() })?;
        return Ok(Resolved { loaded: l, cog });
    }
    let mut hits: Vec<(&LoadedSource, SourceCog)> = loaded
        .iter()
        .filter(|l| l.source.enabled)
        .filter_map(|l| l.cog(&r.id).map(|c| (l, c)))
        .collect();
    if hits.is_empty() {
        return Err(SourceError::NotFound { source_name: None, id: r.id.clone() });
    }
    let top = hits.iter().map(|(l, _)| l.source.priority).max().unwrap_or(0);
    hits.retain(|(l, _)| l.source.priority == top);
    if hits.len() > 1 {
        let mut sources: Vec<String> = hits.iter().map(|(l, _)| l.source.name.clone()).collect();
        sources.sort();
        return Err(SourceError::Ambiguous { id: r.id.clone(), sources });
    }
    let (l, cog) = hits.remove(0);
    Ok(Resolved { loaded: l, cog })
}

/// The guard in front of `--enable`: a bare id that resolved to a source defined only in this
/// project's file must be given namespaced (or confirmed), because a cloned repository can
/// define its own sources and a bare id would otherwise start whatever they serve.
pub fn enable_guard(eff: &EffectiveSources, cref: &CogRef, resolved: &Resolved<'_>, enable: bool, confirmed: bool) -> Result<()> {
    if enable && cref.source.is_none() && eff.from_project(&resolved.loaded.source.name) && !confirmed {
        return Err(SourceError::NeedsNamespacedId { reference: cref.id.clone(), namespaced: resolved.namespaced() });
    }
    Ok(())
}
