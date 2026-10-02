//! Workload effect fields (ADR-099 section 4) and their mapping onto the
//! governance engine's 5D [`EffectVector`].
//!
//! Callers describe a workload action in domain terms (package trust, node
//! trust tier, network policy, secrets, emulation, accelerator use, resource
//! cost). The gate derives the 5D vector itself from those fields, so a caller
//! cannot lower its own score by sending a hand-written `effect` object.

use serde::{Deserialize, Serialize};

use crate::governance::EffectVector;

/// Maximum length of a workload kind string (`cog`, `inference`, ...).
pub const MAX_KIND_LEN: usize = 64;
/// Maximum length of an accelerator capability id.
pub const MAX_ACCEL_LEN: usize = 128;
/// Maximum number of signer keys or artifact hashes in one request.
pub const MAX_REFS: usize = 64;

/// How far the package's provenance can be trusted (ADR-099 section 8).
///
/// Ordered from least to most trusted, so `>=` comparisons express
/// "at least this trusted".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageTrust {
    /// No valid signature.
    Unsigned,
    /// Authorised by a project certificate chained to the user key (the
    /// `project` kind, ADR-103 A6). Ordered just above `Unsigned` so it never
    /// satisfies the signed-package minimums cog permits ask for; only a
    /// permit that names it (`min_package_trust = project_cert`) matches.
    ProjectCert,
    /// Signed, but not by a pinned signer.
    SignedUnpinned,
    /// Operator attestation over a hash manifest (model weights, ADR-101).
    OperatorAttested,
    /// At least one valid signature from a pinned signer.
    PinnedSigner,
}

/// Node trust tier (ADR-099 section 8.3).
///
/// Local placeholder: card 02 owns the capability vocabulary
/// (`trust.tier.{discovered,paired,pinned}`); this enum mirrors those ids
/// until that type lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeTrustTier {
    /// Seen on the mesh, not paired. Runs nothing by default.
    Discovered,
    /// Operator-paired. Runs operator-signed workloads.
    Paired,
    /// Pinned. May run workloads that carry secrets.
    Pinned,
}

/// Network exposure the workload asks for, ordered by exposure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    /// No network access.
    None,
    /// Local network only (sensor feeds, mesh peers).
    Lan,
    /// Outbound internet access.
    Egress,
}

/// The effect fields of one workload governance request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkloadEffect {
    /// Workload kind (open string: `cog`, `inference`, `wasm-module`, ...).
    pub kind: String,
    /// Package provenance.
    pub package_trust: PackageTrust,
    /// Trust tier of the target node.
    pub node_tier: NodeTrustTier,
    /// Requested network exposure.
    pub network: NetworkPolicy,
    /// Whether secrets are delivered to the instance.
    #[serde(default)]
    pub secrets: bool,
    /// Whether the placement runs under emulation (operator opt-in only).
    #[serde(default)]
    pub emulated: bool,
    /// Accelerator capability id used, if any (e.g. `accel.gpu.metal`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accelerator: Option<String>,
    /// Normalised resource cost in `[0.0, 1.0]` (share of the node).
    #[serde(default)]
    pub resource_cost: f64,
}

/// Revocable references carried with a workload request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadRefs {
    /// Package id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    /// Hex Ed25519 public keys of the package's signers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signer_keys: Vec<String>,
    /// BLAKE3 hex hashes of the manifest and payload artifacts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_hashes: Vec<String>,
}

/// Parsed and validated gate context for a workload action.
///
/// Wire shape (inside the gate `context` JSON):
/// `{"workload": {<WorkloadEffect fields>, "package_id": .., "signer_keys": [..],
/// "artifact_hashes": [..]}}`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadRequest {
    /// Effect fields.
    pub effect: WorkloadEffect,
    /// Revocable references.
    pub refs: WorkloadRefs,
}

fn is_kind_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
}

fn is_capability_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ACCEL_LEN
        && !s.starts_with('.')
        && !s.ends_with('.')
        && !s.contains("..")
        && s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

impl WorkloadEffect {
    /// Validate field ranges and formats (system-boundary check).
    pub fn validate(&self) -> Result<(), String> {
        if self.kind.is_empty() || self.kind.len() > MAX_KIND_LEN {
            return Err(format!("workload kind must be 1..={MAX_KIND_LEN} chars"));
        }
        if !self.kind.chars().all(is_kind_char) {
            return Err(format!(
                "workload kind '{}' must be lowercase [a-z0-9_-]",
                self.kind
            ));
        }
        if let Some(acc) = &self.accelerator
            && !is_capability_id(acc)
        {
            return Err(format!("accelerator '{acc}' is not a dotted lowercase capability id"));
        }
        if !self.resource_cost.is_finite() || !(0.0..=1.0).contains(&self.resource_cost) {
            return Err(format!(
                "resource_cost {} must be a finite value in [0.0, 1.0]",
                self.resource_cost
            ));
        }
        Ok(())
    }

    /// Derive the 5D effect vector.
    ///
    /// - risk: package provenance, raised by emulation.
    /// - security: node tier plus network exposure.
    /// - privacy: secrets plus internet egress.
    /// - novelty: emulation and accelerator use.
    /// - fairness: resource cost (share of a shared node).
    pub fn to_effect_vector(&self) -> EffectVector {
        let clamp = |v: f64| v.clamp(0.0, 1.0);
        let trust_risk = match self.package_trust {
            PackageTrust::Unsigned => 0.9,
            PackageTrust::SignedUnpinned => 0.6,
            PackageTrust::OperatorAttested => 0.3,
            PackageTrust::PinnedSigner => 0.1,
            // Chained to the user's own key: between a pinned signer and an
            // operator attestation.
            PackageTrust::ProjectCert => 0.2,
        };
        let tier_sec = match self.node_tier {
            NodeTrustTier::Discovered => 0.8,
            NodeTrustTier::Paired => 0.3,
            NodeTrustTier::Pinned => 0.1,
        };
        let net_sec = match self.network {
            NetworkPolicy::None => 0.0,
            NetworkPolicy::Lan => 0.1,
            NetworkPolicy::Egress => 0.3,
        };
        let egress_priv = if self.network == NetworkPolicy::Egress { 0.2 } else { 0.0 };
        EffectVector {
            risk: clamp(trust_risk + if self.emulated { 0.1 } else { 0.0 }),
            fairness: clamp(self.resource_cost),
            privacy: clamp(if self.secrets { 0.5 } else { 0.0 } + egress_priv),
            novelty: clamp(
                if self.emulated { 0.4 } else { 0.0 }
                    + if self.accelerator.is_some() { 0.2 } else { 0.0 },
            ),
            security: clamp(tier_sec + net_sec),
        }
    }

    /// Flatten the fields into the string context map the engine takes.
    pub fn context_map(&self) -> std::collections::HashMap<String, String> {
        let to_s = |v: serde_json::Value| v.as_str().map(str::to_owned).unwrap_or_default();
        let mut m = std::collections::HashMap::new();
        m.insert("kind".into(), self.kind.clone());
        m.insert("package_trust".into(), to_s(serde_json::json!(self.package_trust)));
        m.insert("node_tier".into(), to_s(serde_json::json!(self.node_tier)));
        m.insert("network".into(), to_s(serde_json::json!(self.network)));
        m.insert("secrets".into(), self.secrets.to_string());
        m.insert("emulated".into(), self.emulated.to_string());
        m.insert(
            "accelerator".into(),
            self.accelerator.clone().unwrap_or_default(),
        );
        m.insert("resource_cost".into(), self.resource_cost.to_string());
        m
    }
}

impl WorkloadRequest {
    /// Parse and validate the `workload` object from a gate context.
    ///
    /// Any top-level `effect` object is ignored on purpose: the vector is
    /// always derived from these fields.
    pub fn from_context(context: &serde_json::Value) -> Result<Self, String> {
        let obj = context
            .get("workload")
            .ok_or_else(|| "missing 'workload' object in gate context".to_owned())?;
        if !obj.is_object() {
            return Err("'workload' must be a JSON object".into());
        }
        let effect: WorkloadEffect =
            serde_json::from_value(obj.clone()).map_err(|e| format!("invalid workload effect: {e}"))?;
        let refs: WorkloadRefs =
            serde_json::from_value(obj.clone()).map_err(|e| format!("invalid workload refs: {e}"))?;
        effect.validate()?;
        if refs.signer_keys.len() > MAX_REFS || refs.artifact_hashes.len() > MAX_REFS {
            return Err(format!("at most {MAX_REFS} signer keys and artifact hashes"));
        }
        use crate::revocation::RevocationKind;
        if let Some(p) = &refs.package_id {
            RevocationKind::Package.normalize(p).map_err(|e| e.to_string())?;
        }
        for k in &refs.signer_keys {
            RevocationKind::SignerKey.normalize(k).map_err(|e| e.to_string())?;
        }
        for h in &refs.artifact_hashes {
            RevocationKind::ArtifactHash.normalize(h).map_err(|e| e.to_string())?;
        }
        Ok(Self { effect, refs })
    }
}
