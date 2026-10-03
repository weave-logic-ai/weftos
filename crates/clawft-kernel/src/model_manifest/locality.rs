//! Locality-aware fetch-versus-relocate decision (ADR-101 section 3,
//! ADR-099 section 6).
//!
//! Weights of tens of gigabytes are never re-shipped when a node already
//! holds them. [`decide`] is a pure function over what nodes advertise:
//!
//! 1. an eligible node that holds every shard wins (relocate the workload to
//!    the bytes);
//! 2. otherwise, if the [`TransferPolicy`] allows it, the manifest is
//!    redistributable ([`Sharing`]) and a complete holder exists, the
//!    eligible node with the fewest missing bytes that has the space fetches
//!    the rest (through the artifact exchange, chunked and resumable);
//! 3. otherwise the workload is unplaceable, with the reason named.
//!
//! The plan carries `--explain` notes. Placement-engine integration is the
//! [`locality_preference`] (soft, prefers the holder) and
//! [`model_present_requirement`] (hard, place only where the bytes are).

use std::collections::BTreeSet;

use clawft_types::placement::engine::Preference;
use clawft_types::placement::{
    AttrPredicate, AttrValue, Capability, CapabilityId, CapabilityState, Requirement,
};

use super::body::{MODEL_MARKER_PREFIX, ModelPackageBody};

/// Operator policy for moving weights between nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPolicy {
    /// Weights may be fetched from another node at all. Default false:
    /// place where the bytes are, or report `Unplaceable`.
    pub allow_weight_transfer: bool,
    /// Largest transfer allowed. `None` means no ceiling (v1 default).
    pub max_bytes: Option<u64>,
    /// Free space that must remain on the target after the transfer.
    pub free_headroom_bytes: u64,
}

impl Default for TransferPolicy {
    fn default() -> Self {
        Self {
            allow_weight_transfer: false,
            max_bytes: None,
            free_headroom_bytes: 1024 * 1024 * 1024,
        }
    }
}

/// Whether the weights may be handed to other nodes. Fail closed: there is
/// no default, and nothing but an explicit opt-in is `Allowed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sharing {
    /// The signer opted in and the redistribution policy allows it.
    Allowed,
    /// Not opted in (or vetoed): weights stay where they are.
    Refused,
}

impl Sharing {
    /// From the manifest's own `redistributable` flag alone.
    pub fn from_manifest(body: &ModelPackageBody) -> Self {
        if body.redistributable { Self::Allowed } else { Self::Refused }
    }
}

/// What one node holds of one model, and whether the placer could use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHolding {
    /// Node id.
    pub node_id: String,
    /// The node passes every other placement constraint for the workload.
    pub eligible: bool,
    /// Usable shard hashes held (excludes an offline, detached holding).
    pub shards: BTreeSet<String>,
    /// Holds the whole attested model and offers it.
    pub complete: bool,
    /// Free bytes where a fetched model would land; `None` when unknown.
    pub free_bytes: Option<u64>,
}

impl NodeHolding {
    /// Read a node's holding of `body` (identified by `package_id`) from its
    /// advertised capabilities. `free_bytes` is the largest `store.tier.*`
    /// `free` attribute (fetches land on the tier with the most room).
    pub fn from_capabilities(
        node_id: &str,
        package_id: &str,
        body: &ModelPackageBody,
        caps: &[Capability],
        eligible: bool,
    ) -> Self {
        let marker = format!("{MODEL_MARKER_PREFIX}{package_id}");
        let wanted: BTreeSet<&str> = body.shard_hashes().collect();
        let mut shards = BTreeSet::new();
        let mut complete = false;
        for c in caps.iter().filter(|c| c.id.as_str() == "model.present") {
            let list = match c.attrs.get("shards") {
                Some(AttrValue::List(l)) => l,
                _ => continue,
            };
            let items: Vec<&str> = list
                .iter()
                .filter_map(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                })
                .collect();
            let has_marker = items.contains(&marker.as_str());
            match (c.state, has_marker) {
                (CapabilityState::Available | CapabilityState::Busy, true) => {
                    complete = true;
                    shards.extend(wanted.iter().map(|s| s.to_string()));
                }
                // Offline (marker, degraded): nothing usable.
                (_, true) => {}
                (_, false) => {
                    shards.extend(items.into_iter().filter(|s| wanted.contains(s)).map(String::from));
                }
            }
        }
        let free_bytes = caps
            .iter()
            .filter(|c| c.id.as_str().starts_with("store.tier."))
            .filter(|c| c.state == CapabilityState::Available)
            .filter_map(|c| c.attrs.get("free").and_then(|v| v.as_f64()))
            .filter(|f| *f >= 0.0)
            .map(|f| f as u64)
            .max();
        Self {
            node_id: node_id.to_string(),
            eligible,
            shards,
            complete,
            free_bytes,
        }
    }

    fn missing_bytes(&self, body: &ModelPackageBody) -> u64 {
        body.shards
            .iter()
            .filter(|s| !self.shards.contains(&s.blake3))
            .map(|s| s.size)
            .fold(0, u64::saturating_add)
    }
}

/// Why a model cannot be placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unplaceable {
    /// No node offers these weights, so there is nothing to fetch from.
    NoHolder,
    /// No node passes the other placement constraints.
    NoEligibleNode,
    /// The bytes are elsewhere and weight transfer is not allowed.
    TransferDisabled,
    /// The bytes are elsewhere and the manifest is not redistributable.
    NotRedistributable,
    /// The transfer exceeds the policy ceiling.
    OverCeiling {
        /// Bytes that would move.
        bytes: u64,
        /// The ceiling.
        max: u64,
    },
    /// The best target lacks room (or its free space is unknown).
    NoSpace {
        /// Bytes that would move plus headroom.
        needed: u64,
        /// Free bytes, when known.
        free: Option<u64>,
    },
}

impl std::fmt::Display for Unplaceable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHolder => write!(f, "no node holds these weights; adopt them on a node first"),
            Self::NoEligibleNode => write!(f, "no node passes the other placement constraints"),
            Self::TransferDisabled => {
                write!(f, "weights are held elsewhere and allow_weight_transfer is off")
            }
            Self::NotRedistributable => {
                write!(f, "weights are held elsewhere and the manifest is not marked redistributable")
            }
            Self::OverCeiling { bytes, max } => {
                write!(f, "transfer of {bytes} bytes exceeds the {max} byte ceiling")
            }
            Self::NoSpace { needed, free: Some(free) } => {
                write!(f, "target needs {needed} bytes free, has {free}")
            }
            Self::NoSpace { needed, free: None } => {
                write!(f, "target needs {needed} bytes free and its free space is unknown")
            }
        }
    }
}

/// The decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalityDecision {
    /// Run on a node that already holds every shard; nothing moves.
    PlaceAtHolder {
        /// The chosen node.
        node_id: String,
    },
    /// Fetch the missing shards to `target`, then run there.
    FetchThenPlace {
        /// The node that receives the shards.
        target: String,
        /// Complete holders that can serve them, by node id.
        sources: Vec<String>,
        /// Bytes that will move.
        bytes: u64,
        /// Shards that will move.
        missing_shards: usize,
    },
    /// Not placeable, with the reason.
    Unplaceable(Unplaceable),
}

/// A decision plus the notes behind it (for `--explain`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalityPlan {
    /// The decision.
    pub decision: LocalityDecision,
    /// One line per consideration, in order.
    pub notes: Vec<String>,
}

/// Choose between relocating to the bytes and fetching them.
pub fn decide(
    body: &ModelPackageBody,
    nodes: &[NodeHolding],
    policy: &TransferPolicy,
    sharing: Sharing,
) -> LocalityPlan {
    let mut notes = vec![format!(
        "model {} ({} shards, {} bytes)",
        body.name,
        body.shards.len(),
        body.total_bytes()
    )];
    let mut sorted: Vec<&NodeHolding> = nodes.iter().collect();
    sorted.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    for n in &sorted {
        notes.push(format!(
            "node {}: eligible={} complete={} holds {}/{} shards",
            n.node_id,
            n.eligible,
            n.complete,
            n.shards.len(),
            body.shards.len()
        ));
    }
    let done = |decision, mut notes: Vec<String>, line: String| {
        notes.push(line);
        LocalityPlan { decision, notes }
    };
    if let Some(n) = sorted.iter().find(|n| n.eligible && n.complete) {
        return done(
            LocalityDecision::PlaceAtHolder { node_id: n.node_id.clone() },
            notes,
            format!("relocate: {} already holds every shard, nothing is copied", n.node_id),
        );
    }
    let sources: Vec<String> = sorted
        .iter()
        .filter(|n| n.complete)
        .map(|n| n.node_id.clone())
        .collect();
    let fail = |u: Unplaceable, notes: Vec<String>| {
        let line = format!("unplaceable: {u}");
        done(LocalityDecision::Unplaceable(u), notes, line)
    };
    let eligible: Vec<&&NodeHolding> = sorted.iter().filter(|n| n.eligible).collect();
    if eligible.is_empty() {
        return fail(Unplaceable::NoEligibleNode, notes);
    }
    if sources.is_empty() {
        return fail(Unplaceable::NoHolder, notes);
    }
    if !policy.allow_weight_transfer {
        return fail(Unplaceable::TransferDisabled, notes);
    }
    if sharing != Sharing::Allowed {
        return fail(Unplaceable::NotRedistributable, notes);
    }
    // Best target first: fewest missing bytes, then node id.
    let mut targets: Vec<(&NodeHolding, u64)> =
        eligible.iter().map(|n| (**n, n.missing_bytes(body))).collect();
    targets.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.node_id.cmp(&b.0.node_id)));
    let mut first_failure = None;
    for (n, bytes) in targets {
        let verdict = (|| {
            if let Some(max) = policy.max_bytes
                && bytes > max
            {
                return Err(Unplaceable::OverCeiling { bytes, max });
            }
            let needed = bytes.saturating_add(policy.free_headroom_bytes);
            match n.free_bytes {
                Some(free) if free >= needed => Ok(()),
                free => Err(Unplaceable::NoSpace { needed, free }),
            }
        })();
        match verdict {
            Ok(()) => {
                let missing_shards = body
                    .shards
                    .iter()
                    .filter(|s| !n.shards.contains(&s.blake3))
                    .count();
                let line = format!(
                    "fetch: {} gets {missing_shards} shard(s), {bytes} bytes, from {}",
                    n.node_id,
                    sources.join(",")
                );
                return done(
                    LocalityDecision::FetchThenPlace {
                        target: n.node_id.clone(),
                        sources,
                        bytes,
                        missing_shards,
                    },
                    notes,
                    line,
                );
            }
            Err(u) => {
                notes.push(format!("target {} rejected: {u}", n.node_id));
                first_failure.get_or_insert(u);
            }
        }
    }
    fail(first_failure.unwrap_or(Unplaceable::NoEligibleNode), notes)
}

/// Requirement: the node advertises this exact attested model as present
/// and available (`model.present` with the `model:<id>` marker).
pub fn model_present_requirement(package_id: &str) -> Requirement {
    Requirement::exact(CapabilityId::new("model.present").expect("valid id"))
        .with_where(AttrPredicate::has(
            "shards",
            AttrValue::Str(format!("{MODEL_MARKER_PREFIX}{package_id}")),
        ))
}

/// Soft preference of weight `weight` for nodes that already hold the model,
/// to push into a workload spec's `policy.preferences` so the placer prefers
/// the node holding the shards. Name carries the model for `--explain`.
pub fn locality_preference(body: &ModelPackageBody, package_id: &str, weight: f64) -> Preference {
    Preference {
        name: format!("model-local:{}", body.name),
        requirement: model_present_requirement(package_id),
        weight,
    }
}
