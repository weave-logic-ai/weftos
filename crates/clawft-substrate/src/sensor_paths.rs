//! Sensor substrate path naming — canonical node-scoped layout + legacy
//! flat migration helpers (WEFT-438).
//!
//! # Decision
//!
//! **Canonical** layout for every physical-sensor emission:
//!
//! ```text
//! substrate/<node-id>/sensor/<sensor-name>/<leaf>
//! ```
//!
//! Examples:
//!
//! | Emission | Canonical path |
//! |----------|----------------|
//! | Mic level object | `substrate/<node-id>/sensor/mic/summary` |
//! | Mic RMS scalar (gauge) | `substrate/<node-id>/sensor/mic/rms` |
//! | Mic PCM window | `substrate/<node-id>/sensor/mic/pcm_chunk` |
//! | Presence | `substrate/<node-id>/sensor/presence/state` |
//! | Rfkill | `substrate/<node-id>/sensor/rfkill/state` |
//!
//! **Legacy flat** layout (pre-node-identity):
//!
//! ```text
//! substrate/sensor/<sensor-name>
//! substrate/sensor/<sensor-name>/<leaf>
//! ```
//!
//! # Why node-scoped wins
//!
//! 1. Multi-node meshes need per-source attribution (two mics on two
//!    ESP32s must not clobber each other).
//! 2. Per-node write gate (task 24 / JOURNALED-NODE-ESP32 §3) only works
//!    when the first segment after `substrate/` is a node id.
//! 3. ACL read rules (ADR-057) already key off the node-scoped form.
//! 4. GUI discovery (`mic_discovery`) and whisper/classify services already
//!    consume node-scoped paths.
//!
//! Mesh-owned namespaces (`kernel`, `cluster`, `chain`, `meta`,
//! `_derived`, `actor`) are **not** sensors and stay unscoped under
//! `substrate/<mesh-section>/…`.
//!
//! # Migration window
//!
//! Emitters dual-write legacy flat + canonical during the window ending
//! at [`LEGACY_FLAT_REMOVAL_VERSION`] / [`LEGACY_FLAT_REMOVAL_DATE`].
//! After that cut, dual-emit flags default to off and the legacy open
//! path is rejected. See `.planning/ontology/ADOPTION.md` §16.
//!
//! Recorded by WEFT-438. Full mic adapter cutover (pcm windowed-Append
//! topic under the same node-scoped tree) is WEFT-418.

use std::fmt;

/// Version at which legacy flat sensor paths are removed from dual-emit
/// defaults and rejected at the write gate.
///
/// Removals land in the 0.9.0 cycle — not mid-0.8.x — so 0.8 consumers
/// keep working for one full minor release after this decision ships.
pub const LEGACY_FLAT_REMOVAL_VERSION: &str = "0.9.0";

/// Calendar target for the same cut (ISO date). Soft target aligned with
/// the 0.9.0 release train; the version constant is authoritative if the
/// two diverge.
pub const LEGACY_FLAT_REMOVAL_DATE: &str = "2026-10-01";

/// Default node-id used by host-local adapters (file-backed mic,
/// presence stub, etc.) when the host has not yet loaded a provisioned
/// daemon node identity.
///
/// Production ESP32 / daemon-hosted sensors MUST pass their real node
/// id. This constant exists so unit tests and single-host previews have
/// a stable, readable segment that is clearly not a provisioned id
/// (32-hex node-id style).
pub const HOST_LOCAL_NODE_ID: &str = "host-local";

/// Mic sensor name segment.
pub const SENSOR_MIC: &str = "mic";
/// Presence sensor name segment.
pub const SENSOR_PRESENCE: &str = "presence";
/// Rfkill sensor name segment.
pub const SENSOR_RFKILL: &str = "rfkill";

/// Canonical mic level-object leaf (full `{rms_db, peak_db, …}` shape).
pub const LEAF_SUMMARY: &str = "summary";
/// Mic RMS scalar leaf — gauge / tray-chip discovery target
/// (`mic_discovery` matches `/sensor/mic/rms`).
pub const LEAF_RMS: &str = "rms";
/// Mic raw-PCM window leaf consumed by whisper / classify.
pub const LEAF_PCM_CHUNK: &str = "pcm_chunk";
/// Generic state leaf for enumerated / presence sensors.
pub const LEAF_STATE: &str = "state";

// ─── Legacy flat constants ─────────────────────────────────────────────

/// Legacy flat mic level path (pre-node-identity). Dual-emitted during
/// the migration window; removed at [`LEGACY_FLAT_REMOVAL_VERSION`].
pub const LEGACY_MIC_PATH: &str = "substrate/sensor/mic";
/// Legacy flat mic PCM path.
pub const LEGACY_MIC_PCM_PATH: &str = "substrate/sensor/mic/pcm_chunk";
/// Legacy flat presence path.
pub const LEGACY_PRESENCE_PATH: &str = "substrate/sensor/presence";
/// Legacy flat rfkill path.
pub const LEGACY_RFKILL_PATH: &str = "substrate/sensor/rfkill";

// ─── Mesh-owned top-level segments (never node-scoped) ─────────────────

/// Top-level path segments under `substrate/` that are mesh-owned (or
/// meta) rather than node-scoped sensor trees.
pub const MESH_OWNED_SEGMENTS: &[&str] = &[
    "kernel",
    "cluster",
    "chain",
    "meta",
    "_derived",
    "actor",
    "sensor", // legacy flat root — not a node id
    "ui",
];

// ─── Builders (canonical) ──────────────────────────────────────────────

/// Build `substrate/<node-id>/sensor/<sensor>/<leaf>`.
pub fn sensor_leaf_path(node_id: &str, sensor: &str, leaf: &str) -> String {
    format!("substrate/{node_id}/sensor/{sensor}/{leaf}")
}

/// Build `substrate/<node-id>/sensor/<sensor>/summary`.
pub fn sensor_summary_path(node_id: &str, sensor: &str) -> String {
    sensor_leaf_path(node_id, sensor, LEAF_SUMMARY)
}

/// Build `substrate/<node-id>/sensor/<sensor>/rms`.
pub fn sensor_rms_path(node_id: &str, sensor: &str) -> String {
    sensor_leaf_path(node_id, sensor, LEAF_RMS)
}

/// Build `substrate/<node-id>/sensor/<sensor>/pcm_chunk`.
pub fn sensor_pcm_chunk_path(node_id: &str, sensor: &str) -> String {
    sensor_leaf_path(node_id, sensor, LEAF_PCM_CHUNK)
}

/// Build `substrate/<node-id>/sensor/<sensor>/state`.
pub fn sensor_state_path(node_id: &str, sensor: &str) -> String {
    sensor_leaf_path(node_id, sensor, LEAF_STATE)
}

/// Mic summary object path.
pub fn mic_summary_path(node_id: &str) -> String {
    sensor_summary_path(node_id, SENSOR_MIC)
}

/// Mic RMS scalar path (tray chip / gauge discovery).
pub fn mic_rms_path(node_id: &str) -> String {
    sensor_rms_path(node_id, SENSOR_MIC)
}

/// Mic PCM chunk path (whisper / classify input).
pub fn mic_pcm_chunk_path(node_id: &str) -> String {
    sensor_pcm_chunk_path(node_id, SENSOR_MIC)
}

/// Host-local mic summary (compile-time-stable for TopicDecl).
pub const HOST_LOCAL_MIC_SUMMARY: &str = "substrate/host-local/sensor/mic/summary";
/// Host-local mic rms.
pub const HOST_LOCAL_MIC_RMS: &str = "substrate/host-local/sensor/mic/rms";
/// Host-local mic pcm_chunk.
pub const HOST_LOCAL_MIC_PCM_CHUNK: &str = "substrate/host-local/sensor/mic/pcm_chunk";

// ─── Legacy builders ───────────────────────────────────────────────────

/// Build legacy flat `substrate/sensor/<sensor>`.
pub fn legacy_sensor_path(sensor: &str) -> String {
    format!("substrate/sensor/{sensor}")
}

/// Build legacy flat `substrate/sensor/<sensor>/<leaf>`.
pub fn legacy_sensor_leaf_path(sensor: &str, leaf: &str) -> String {
    format!("substrate/sensor/{sensor}/{leaf}")
}

// ─── Classification ────────────────────────────────────────────────────

/// Result of classifying a substrate path against the sensor naming
/// scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensorPathKind {
    /// Canonical node-scoped form.
    Canonical {
        /// Publishing node id.
        node_id: String,
        /// Sensor name (`mic`, `presence`, …).
        sensor: String,
        /// Leaf segment (`summary`, `rms`, `pcm_chunk`, …).
        leaf: String,
    },
    /// Pre-migration flat form `substrate/sensor/<sensor>[/<leaf>]`.
    LegacyFlat {
        /// Sensor name.
        sensor: String,
        /// Optional leaf (None when path is exactly `substrate/sensor/<sensor>`).
        leaf: Option<String>,
    },
    /// Not a sensor path (mesh-owned, actor, unknown).
    Other,
}

impl fmt::Display for SensorPathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SensorPathKind::Canonical {
                node_id,
                sensor,
                leaf,
            } => write!(f, "canonical:{node_id}/sensor/{sensor}/{leaf}"),
            SensorPathKind::LegacyFlat { sensor, leaf: None } => {
                write!(f, "legacy:sensor/{sensor}")
            }
            SensorPathKind::LegacyFlat {
                sensor,
                leaf: Some(leaf),
            } => write!(f, "legacy:sensor/{sensor}/{leaf}"),
            SensorPathKind::Other => write!(f, "other"),
        }
    }
}

/// Classify `path` as canonical node-scoped, legacy flat, or other.
///
/// Rules:
/// - `substrate/sensor/<sensor>` or `substrate/sensor/<sensor>/<leaf…>`
///   → [`SensorPathKind::LegacyFlat`]
/// - `substrate/<node>/sensor/<sensor>/<leaf>` where `<node>` is not a
///   mesh-owned segment → [`SensorPathKind::Canonical`]
/// - everything else → [`SensorPathKind::Other`]
pub fn classify_sensor_path(path: &str) -> SensorPathKind {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.first().copied() != Some("substrate") {
        return SensorPathKind::Other;
    }
    match parts.as_slice() {
        // substrate/sensor/<name>
        ["substrate", "sensor", sensor] => SensorPathKind::LegacyFlat {
            sensor: (*sensor).to_string(),
            leaf: None,
        },
        // substrate/sensor/<name>/<leaf…>
        ["substrate", "sensor", sensor, leaf, ..] => SensorPathKind::LegacyFlat {
            sensor: (*sensor).to_string(),
            leaf: Some((*leaf).to_string()),
        },
        // substrate/<node>/sensor/<name>/<leaf>
        ["substrate", node, "sensor", sensor, leaf, ..]
            if !MESH_OWNED_SEGMENTS.contains(node) =>
        {
            SensorPathKind::Canonical {
                node_id: (*node).to_string(),
                sensor: (*sensor).to_string(),
                leaf: (*leaf).to_string(),
            }
        }
        _ => SensorPathKind::Other,
    }
}

/// True when `path` is a pre-migration flat sensor path.
pub fn is_legacy_flat_sensor_path(path: &str) -> bool {
    matches!(classify_sensor_path(path), SensorPathKind::LegacyFlat { .. })
}

/// True when `path` is a canonical node-scoped sensor path.
pub fn is_node_scoped_sensor_path(path: &str) -> bool {
    matches!(classify_sensor_path(path), SensorPathKind::Canonical { .. })
}

/// Map a legacy flat path to its canonical equivalent under `node_id`.
///
/// Returns `None` when `path` is not a legacy flat sensor path.
///
/// Mapping rules:
/// - `substrate/sensor/<s>` → `…/sensor/<s>/summary` (object root → summary)
/// - `substrate/sensor/<s>/<leaf>` → `…/sensor/<s>/<leaf>`
pub fn legacy_to_canonical(path: &str, node_id: &str) -> Option<String> {
    match classify_sensor_path(path) {
        SensorPathKind::LegacyFlat {
            sensor,
            leaf: None,
        } => Some(sensor_summary_path(node_id, &sensor)),
        SensorPathKind::LegacyFlat {
            sensor,
            leaf: Some(leaf),
        } => Some(sensor_leaf_path(node_id, &sensor, &leaf)),
        _ => None,
    }
}

// ─── Dual-emit plan (migration window) ─────────────────────────────────

/// Paths a sensor level emitter should write during the dual-emit
/// migration window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicLevelEmitPlan {
    /// Canonical summary object path (always present).
    pub summary: String,
    /// Canonical rms scalar path (always present — GUI discovery).
    pub rms: String,
    /// Legacy flat path when dual-emit is enabled.
    pub legacy: Option<String>,
}

/// Build the dual-emit plan for a mic level payload under `node_id`.
///
/// When `dual_emit_legacy` is true (default during the migration
/// window), the plan includes [`LEGACY_MIC_PATH`]. After
/// [`LEGACY_FLAT_REMOVAL_VERSION`], callers should pass `false`.
pub fn mic_level_emit_plan(node_id: &str, dual_emit_legacy: bool) -> MicLevelEmitPlan {
    MicLevelEmitPlan {
        summary: mic_summary_path(node_id),
        rms: mic_rms_path(node_id),
        legacy: dual_emit_legacy.then(|| LEGACY_MIC_PATH.to_string()),
    }
}

/// Paths a mic PCM window emitter should write (WEFT-418).
///
/// Canonical `pcm_chunk` is always present and is consumed as a
/// windowed-Append topic. Legacy flat dual-emit mirrors the level plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicPcmEmitPlan {
    /// Canonical pcm_chunk path (always present).
    pub pcm: String,
    /// Legacy flat pcm_chunk path when dual-emit is enabled.
    pub legacy: Option<String>,
}

/// Build the dual-emit plan for a mic PCM window under `node_id`.
///
/// When `dual_emit_legacy` is true, the plan includes
/// [`LEGACY_MIC_PCM_PATH`]. After [`LEGACY_FLAT_REMOVAL_VERSION`],
/// callers should pass `false`.
pub fn mic_pcm_emit_plan(node_id: &str, dual_emit_legacy: bool) -> MicPcmEmitPlan {
    MicPcmEmitPlan {
        pcm: mic_pcm_chunk_path(node_id),
        legacy: dual_emit_legacy.then(|| LEGACY_MIC_PCM_PATH.to_string()),
    }
}

/// Retained window length for the mic `pcm_chunk` Append topic.
///
/// At 2 Hz (500 ms windows) this is ~4 s of recent audio — enough for
/// a short barge-in buffer without unbounded growth. Substrate trims
/// the front of the array on each Append when this is registered via
/// [`crate::adapter::TopicDecl::max_len`].
pub const MIC_PCM_WINDOW_MAX_LEN: usize = 8;

/// Default dual-emit flag for new adapters: `true` until the removal
/// version ships. Flip this constant (and re-release) to turn dual-emit
/// off repo-wide without chasing call sites.
pub const DEFAULT_DUAL_EMIT_LEGACY: bool = true;

/// True when a topic string is an accepted open-target for the host-local
/// mic level subscription (legacy flat **or** any host-local/canonical
/// summary/rms path). Used by [`crate::mic::MicrophoneAdapter::open`].
pub fn is_mic_level_open_topic(topic: &str, node_id: &str) -> bool {
    if topic == LEGACY_MIC_PATH {
        return true;
    }
    if topic == mic_summary_path(node_id) || topic == mic_rms_path(node_id) {
        return true;
    }
    // Accept any node-scoped mic summary/rms so multi-node open works.
    match classify_sensor_path(topic) {
        SensorPathKind::Canonical {
            sensor,
            leaf,
            ..
        } => sensor == SENSOR_MIC && (leaf == LEAF_SUMMARY || leaf == LEAF_RMS),
        _ => false,
    }
}

/// True when `topic` is an accepted open-target for the mic PCM stream
/// (legacy flat **or** any node-scoped `…/sensor/mic/pcm_chunk`).
pub fn is_mic_pcm_open_topic(topic: &str, node_id: &str) -> bool {
    if topic == LEGACY_MIC_PCM_PATH {
        return true;
    }
    if topic == mic_pcm_chunk_path(node_id) {
        return true;
    }
    match classify_sensor_path(topic) {
        SensorPathKind::Canonical {
            sensor,
            leaf,
            ..
        } => sensor == SENSOR_MIC && leaf == LEAF_PCM_CHUNK,
        _ => false,
    }
}

/// True when `topic` is any accepted mic open target (level, pcm, or
/// the shared producer surface). Healthcheck is checked separately by
/// the adapter.
pub fn is_mic_open_topic(topic: &str, node_id: &str) -> bool {
    is_mic_level_open_topic(topic, node_id) || is_mic_pcm_open_topic(topic, node_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_match_journaled_sensor_mic() {
        assert_eq!(
            mic_summary_path("n-6f3a9c"),
            "substrate/n-6f3a9c/sensor/mic/summary"
        );
        assert_eq!(
            mic_rms_path("n-6f3a9c"),
            "substrate/n-6f3a9c/sensor/mic/rms"
        );
        assert_eq!(
            mic_pcm_chunk_path("n-6f3a9c"),
            "substrate/n-6f3a9c/sensor/mic/pcm_chunk"
        );
    }

    #[test]
    fn host_local_constants_align_with_builders() {
        assert_eq!(
            mic_summary_path(HOST_LOCAL_NODE_ID),
            HOST_LOCAL_MIC_SUMMARY
        );
        assert_eq!(mic_rms_path(HOST_LOCAL_NODE_ID), HOST_LOCAL_MIC_RMS);
        assert_eq!(
            mic_pcm_chunk_path(HOST_LOCAL_NODE_ID),
            HOST_LOCAL_MIC_PCM_CHUNK
        );
    }

    #[test]
    fn classify_canonical() {
        let k = classify_sensor_path("substrate/n-bfc4cd/sensor/mic/rms");
        assert_eq!(
            k,
            SensorPathKind::Canonical {
                node_id: "n-bfc4cd".into(),
                sensor: "mic".into(),
                leaf: "rms".into(),
            }
        );
        assert!(is_node_scoped_sensor_path(
            "substrate/n-bfc4cd/sensor/mic/rms"
        ));
        assert!(!is_legacy_flat_sensor_path(
            "substrate/n-bfc4cd/sensor/mic/rms"
        ));
    }

    #[test]
    fn classify_legacy_root_and_leaf() {
        assert_eq!(
            classify_sensor_path("substrate/sensor/mic"),
            SensorPathKind::LegacyFlat {
                sensor: "mic".into(),
                leaf: None,
            }
        );
        assert_eq!(
            classify_sensor_path("substrate/sensor/mic/pcm_chunk"),
            SensorPathKind::LegacyFlat {
                sensor: "mic".into(),
                leaf: Some("pcm_chunk".into()),
            }
        );
        assert!(is_legacy_flat_sensor_path("substrate/sensor/mic"));
        assert!(!is_node_scoped_sensor_path("substrate/sensor/mic"));
    }

    #[test]
    fn classify_mesh_owned_is_other() {
        assert_eq!(
            classify_sensor_path("substrate/kernel/processes"),
            SensorPathKind::Other
        );
        assert_eq!(
            classify_sensor_path("substrate/meta/adapter/mic/healthcheck"),
            SensorPathKind::Other
        );
        assert_eq!(
            classify_sensor_path("substrate/_derived/transcript/n-x/mic"),
            SensorPathKind::Other
        );
    }

    #[test]
    fn legacy_to_canonical_maps_root_to_summary() {
        assert_eq!(
            legacy_to_canonical("substrate/sensor/mic", "host-local").as_deref(),
            Some("substrate/host-local/sensor/mic/summary")
        );
        assert_eq!(
            legacy_to_canonical("substrate/sensor/mic/pcm_chunk", "n-1").as_deref(),
            Some("substrate/n-1/sensor/mic/pcm_chunk")
        );
        assert_eq!(
            legacy_to_canonical("substrate/n-1/sensor/mic/rms", "n-1"),
            None
        );
    }

    #[test]
    fn mic_level_emit_plan_dual_and_single() {
        let dual = mic_level_emit_plan("host-local", true);
        assert_eq!(dual.summary, HOST_LOCAL_MIC_SUMMARY);
        assert_eq!(dual.rms, HOST_LOCAL_MIC_RMS);
        assert_eq!(dual.legacy.as_deref(), Some(LEGACY_MIC_PATH));

        let single = mic_level_emit_plan("n-abc", false);
        assert_eq!(single.summary, "substrate/n-abc/sensor/mic/summary");
        assert_eq!(single.rms, "substrate/n-abc/sensor/mic/rms");
        assert!(single.legacy.is_none());
    }

    #[test]
    fn is_mic_level_open_topic_accepts_legacy_and_canonical() {
        assert!(is_mic_level_open_topic(LEGACY_MIC_PATH, "host-local"));
        assert!(is_mic_level_open_topic(HOST_LOCAL_MIC_SUMMARY, "host-local"));
        assert!(is_mic_level_open_topic(HOST_LOCAL_MIC_RMS, "host-local"));
        assert!(is_mic_level_open_topic(
            "substrate/n-other/sensor/mic/summary",
            "host-local"
        ));
        assert!(!is_mic_level_open_topic(
            "substrate/n-other/sensor/mic/pcm_chunk",
            "host-local"
        ));
        assert!(!is_mic_level_open_topic(
            "substrate/sensor/tof",
            "host-local"
        ));
    }

    #[test]
    fn mic_pcm_emit_plan_dual_and_single() {
        let dual = mic_pcm_emit_plan("host-local", true);
        assert_eq!(dual.pcm, HOST_LOCAL_MIC_PCM_CHUNK);
        assert_eq!(dual.legacy.as_deref(), Some(LEGACY_MIC_PCM_PATH));

        let single = mic_pcm_emit_plan("n-abc", false);
        assert_eq!(single.pcm, "substrate/n-abc/sensor/mic/pcm_chunk");
        assert!(single.legacy.is_none());
    }

    #[test]
    fn is_mic_pcm_open_topic_accepts_legacy_and_canonical() {
        assert!(is_mic_pcm_open_topic(LEGACY_MIC_PCM_PATH, "host-local"));
        assert!(is_mic_pcm_open_topic(HOST_LOCAL_MIC_PCM_CHUNK, "host-local"));
        assert!(is_mic_pcm_open_topic(
            "substrate/n-other/sensor/mic/pcm_chunk",
            "host-local"
        ));
        assert!(!is_mic_pcm_open_topic(HOST_LOCAL_MIC_SUMMARY, "host-local"));
        assert!(!is_mic_pcm_open_topic(LEGACY_MIC_PATH, "host-local"));
    }

    #[test]
    fn is_mic_open_topic_covers_level_and_pcm() {
        assert!(is_mic_open_topic(HOST_LOCAL_MIC_SUMMARY, "host-local"));
        assert!(is_mic_open_topic(HOST_LOCAL_MIC_PCM_CHUNK, "host-local"));
        assert!(is_mic_open_topic(LEGACY_MIC_PATH, "host-local"));
        assert!(is_mic_open_topic(LEGACY_MIC_PCM_PATH, "host-local"));
        assert!(!is_mic_open_topic(
            "substrate/meta/adapter/mic/healthcheck",
            "host-local"
        ));
    }

    #[test]
    fn removal_constants_are_documented() {
        // Pin so a silent edit of the removal target shows up in review.
        assert_eq!(LEGACY_FLAT_REMOVAL_VERSION, "0.9.0");
        assert_eq!(LEGACY_FLAT_REMOVAL_DATE, "2026-10-01");
        const { assert!(DEFAULT_DUAL_EMIT_LEGACY) };
        assert_eq!(MIC_PCM_WINDOW_MAX_LEN, 8);
    }
}
