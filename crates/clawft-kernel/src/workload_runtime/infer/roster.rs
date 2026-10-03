//! Import of the model lab's roster (`~/llm/docs/models/queue.yaml`, card
//! mesh-placement-20; ADR-101 section 8 step 2) into [`InferenceSpec`]s.
//!
//! The roster is read, never written, and `~/llm` stays a separate repo:
//! the caller names the file. What the roster holds maps directly (role id,
//! backend, port, `ram_gb`); what it does not hold (co-residency
//! `excludes`, latency class, stickiness) comes from an operator
//! [`RosterOverlay`], with the ADR's defaults otherwise: inference is sticky
//! (warm KV), roles are `batch` unless the overlay or the role's text says
//! voice. Entries the adapters cannot run (vision, embeddings, STT, TTS
//! through `mlx-audio`) and aliases of an earlier entry are reported in
//! [`ImportedRoster::skipped`] with the reason, not dropped silently.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::residency::GB;
use super::spec::{InferFlavor, InferenceSpec, LatencyClass};
use crate::workload_pkg::manifest::valid_token;

/// Largest roster file read.
pub const MAX_ROSTER_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 256;

/// Operator-supplied facts the roster does not carry.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterOverlay {
    /// Roles each role cannot be resident beside (`role:planner` or `planner`).
    #[serde(default)]
    pub excludes: BTreeMap<String, Vec<String>>,
    /// Latency class per role.
    #[serde(default)]
    pub latency_class: BTreeMap<String, LatencyClass>,
    /// Stickiness per role (default true).
    #[serde(default)]
    pub sticky: BTreeMap<String, bool>,
}

/// One roster entry turned into a spec, with what was left behind.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedSpec {
    /// The inference spec (`model` is the roster's display name: adopt the
    /// weights under that name in the model registry).
    pub spec: InferenceSpec,
    /// Roster `repo` (where the weights came from).
    pub repo: String,
    /// Roster `status` (`live`, `ready`, ...).
    pub status: String,
}

/// The result of an import.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportedRoster {
    /// Importable entries, in roster order.
    pub specs: Vec<ImportedSpec>,
    /// `(id, reason)` for each entry not imported.
    pub skipped: Vec<(String, String)>,
}

impl ImportedRoster {
    /// The spec for roster id `id`.
    pub fn get(&self, id: &str) -> Option<&ImportedSpec> {
        self.specs.iter().find(|s| s.spec.role == id)
    }
}

#[derive(Debug, Deserialize)]
struct File {
    #[serde(default)]
    roster: Vec<Raw>,
}

#[derive(Debug, Deserialize)]
struct Raw {
    id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    repo: String,
    #[serde(default)]
    ram_gb: serde_yaml::Value,
    #[serde(default)]
    backend: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    status: String,
}

fn ram_bytes(v: &serde_yaml::Value) -> Option<u64> {
    let gb = match v {
        serde_yaml::Value::Number(n) => n.as_f64()?,
        // The roster writes "~10" for unmeasured entries.
        serde_yaml::Value::String(s) => s.trim().trim_start_matches('~').parse::<f64>().ok()?,
        _ => return None,
    };
    (gb.is_finite() && (0.0..=4096.0).contains(&gb)).then_some((gb * GB as f64) as u64)
}

/// The adapter flavor the roster's `backend` text names (the first listed
/// when it offers alternatives).
pub fn flavor_for_backend(backend: &str) -> Option<InferFlavor> {
    let first = backend.split_whitespace().next()?.to_ascii_lowercase();
    match first.as_str() {
        "mlx_lm" | "mlx-lm" => Some(InferFlavor::MlxLm),
        "llamacpp" | "llama.cpp" | "llama-server" => Some(InferFlavor::LlamaCpp),
        "ollama" => Some(InferFlavor::Ollama),
        _ => None,
    }
}

/// Parse roster YAML. `overlay` supplies excludes, latency and stickiness.
pub fn import_roster(yaml: &str, overlay: &RosterOverlay) -> Result<ImportedRoster, String> {
    if yaml.len() > MAX_ROSTER_BYTES {
        return Err(format!("roster is over {MAX_ROSTER_BYTES} bytes"));
    }
    let file: File = serde_yaml::from_str(yaml).map_err(|e| format!("roster: {e}"))?;
    if file.roster.len() > MAX_ENTRIES {
        return Err(format!("roster lists more than {MAX_ENTRIES} entries"));
    }
    let mut out = ImportedRoster::default();
    // (flavor, port, repo) -> id of the first entry, to spot aliases.
    let mut seen: BTreeMap<(String, Option<u16>, String), String> = BTreeMap::new();
    for r in file.roster {
        let skip = |out: &mut ImportedRoster, why: String| out.skipped.push((r.id.clone(), why));
        if !valid_token(&r.id, 64) {
            skip(&mut out, "id is not a plain token".into());
            continue;
        }
        let Some(flavor) = flavor_for_backend(&r.backend) else {
            skip(&mut out, format!("backend {:?} has no inference adapter", r.backend));
            continue;
        };
        let key = (flavor.id().to_string(), r.port, r.repo.clone());
        if let Some(first) = seen.get(&key) {
            skip(&mut out, format!("alias of {first} (same server, port and weights)"));
            continue;
        }
        let Some(weights) = ram_bytes(&r.ram_gb) else {
            skip(&mut out, "ram_gb is missing or not a number".into());
            continue;
        };
        let mut spec = InferenceSpec::new(&r.id, flavor);
        spec.model = valid_token(&r.name, 128).then(|| r.name.clone());
        spec.serve.port = r.port;
        spec.memory.weights_bytes = weights;
        spec.sticky = overlay.sticky.get(&r.id).copied().unwrap_or(true);
        spec.latency_class = overlay.latency_class.get(&r.id).copied().unwrap_or_else(|| {
            let t = format!("{} {}", r.id, r.role).to_ascii_lowercase();
            if t.contains("voice") || t.contains("realtime") {
                LatencyClass::Interactive
            } else {
                LatencyClass::Batch
            }
        });
        spec.excludes = overlay.excludes.get(&r.id).cloned().unwrap_or_default();
        if let Err(e) = spec.validate() {
            skip(&mut out, format!("not a valid inference spec: {e}"));
            continue;
        }
        seen.insert(key, r.id.clone());
        out.specs.push(ImportedSpec { spec, repo: r.repo, status: r.status });
    }
    Ok(out)
}
