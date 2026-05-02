//! Discover the live mic substrate path at runtime.
//!
//! Phase 3 retired the legacy flat path `substrate/sensor/mic`; every
//! source-node now publishes under `substrate/<node-id>/sensor/mic/...`
//! (e.g. `substrate/n-bfc4cd/sensor/mic/rms`). The relay used by the
//! native driver to inject externally-published values into the local
//! substrate (see `native_live::relay_external_paths`) needs to know
//! which node-prefixed path to poll.
//!
//! This module owns the discovery rule. The native driver calls
//! `find_mic_path` once on the first successful `substrate.list`
//! response after a (re)connection and uses the discovered path
//! going forward. If no mic is found, the relay simply skips the
//! mic path that tick and the tray-chip mic gauge renders a dimmed
//! "no mic" state — there is no fallback to the legacy path, which
//! would just re-introduce the bug we are fixing.

use serde_json::Value;

/// Suffix every per-node mic publish ends with. We pick `rms` because
/// the on-wire mic emission shape is `substrate/<node>/sensor/mic/rms`
/// (a scalar dB level), which is the gauge value the chip wants. The
/// daemon's `MicrophoneAdapter` and the ESP32 firmware both publish
/// at this leaf — see `clawft-substrate::mic` for the host-local case
/// and `.planning/sensors/JOURNALED-NODE-ESP32.md` §3 for the firmware
/// shape.
const MIC_RMS_SUFFIX: &str = "/sensor/mic/rms";

/// Find the first per-node mic path in a `substrate.list` response.
///
/// Returns the full path (e.g. `substrate/n-bfc4cd/sensor/mic/rms`)
/// of the first child whose path ends in `MIC_RMS_SUFFIX`. Returns
/// `None` when no such path exists in the response — the chip
/// renders a dimmed "no mic" state in that case.
///
/// "First" is defined by the order the daemon returned the children.
/// `substrate.list` sorts by path (per `SubstrateService::list`), so
/// this is deterministic across calls and equivalent to picking the
/// alphabetically-first node-id with a mic.
///
/// The response shape is whatever `substrate.list` returns:
///
/// ```json
/// {
///   "children": [
///     { "path": "substrate/n-bfc4cd/sensor/mic/rms", "has_value": true, "child_count": 0 },
///     ...
///   ],
///   "tick": 42
/// }
/// ```
///
/// We only care about the `path` field; `has_value` / `child_count`
/// are advisory and we don't filter on them. If a path is listed
/// but carries no value yet (the publisher just connected but hasn't
/// emitted anything), the relay will see `value: null` from
/// `substrate.read` and quietly skip that tick — same failure mode
/// as a missing mic, which is exactly what the chip should reflect.
pub fn find_mic_path(list_response: &Value) -> Option<String> {
    let children = list_response.get("children")?.as_array()?;
    for child in children {
        let path = child.get("path")?.as_str()?;
        if path.ends_with(MIC_RMS_SUFFIX) {
            return Some(path.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_mic_in_listing() {
        let resp = json!({
            "children": [
                { "path": "substrate/n-bfc4cd/sensor/mic/rms",
                  "has_value": true, "child_count": 0 },
            ],
            "tick": 1,
        });
        assert_eq!(
            find_mic_path(&resp),
            Some("substrate/n-bfc4cd/sensor/mic/rms".to_string()),
        );
    }

    #[test]
    fn empty_list_returns_none() {
        let resp = json!({ "children": [], "tick": 0 });
        assert_eq!(find_mic_path(&resp), None);
    }

    #[test]
    fn list_without_mic_returns_none() {
        let resp = json!({
            "children": [
                { "path": "substrate/n-bfc4cd/sensor/tof/depths_mm",
                  "has_value": true, "child_count": 0 },
                { "path": "substrate/n-bfc4cd/health",
                  "has_value": true, "child_count": 0 },
            ],
            "tick": 7,
        });
        assert_eq!(find_mic_path(&resp), None);
    }

    #[test]
    fn picks_first_when_multiple_nodes_have_mics() {
        // The daemon returns children sorted by path, so "first" maps
        // to the alphabetically-first node-id. We just verify we don't
        // skip the first match.
        let resp = json!({
            "children": [
                { "path": "substrate/n-aaaaaa/sensor/mic/rms",
                  "has_value": true, "child_count": 0 },
                { "path": "substrate/n-bbbbbb/sensor/mic/rms",
                  "has_value": true, "child_count": 0 },
            ],
            "tick": 9,
        });
        assert_eq!(
            find_mic_path(&resp),
            Some("substrate/n-aaaaaa/sensor/mic/rms".to_string()),
        );
    }

    #[test]
    fn ignores_other_mic_subpaths() {
        // Only `/sensor/mic/rms` is the gauge value. Sibling leaves
        // like `pcm_chunk` or `peak_db` are real but not what the
        // tray chip displays — let the chip stay focused on rms.
        let resp = json!({
            "children": [
                { "path": "substrate/n-x/sensor/mic/pcm_chunk",
                  "has_value": true, "child_count": 0 },
                { "path": "substrate/n-x/sensor/mic/peak_db",
                  "has_value": true, "child_count": 0 },
            ],
            "tick": 3,
        });
        assert_eq!(find_mic_path(&resp), None);
    }

    #[test]
    fn legacy_flat_path_does_not_match() {
        // Defensive: the legacy `substrate/sensor/mic` (no node
        // segment) must NOT match — that path is what we are
        // migrating away from. If it did match the chip would silently
        // keep using it and the migration would be a no-op.
        let resp = json!({
            "children": [
                { "path": "substrate/sensor/mic",
                  "has_value": true, "child_count": 0 },
            ],
            "tick": 1,
        });
        assert_eq!(find_mic_path(&resp), None);
    }

    #[test]
    fn malformed_response_returns_none() {
        // Whatever weird thing the daemon emits, we must not panic.
        assert_eq!(find_mic_path(&json!({})), None);
        assert_eq!(find_mic_path(&json!({ "children": "nope" })), None);
        assert_eq!(find_mic_path(&Value::Null), None);
        assert_eq!(
            find_mic_path(&json!({ "children": [{"not_a_path": 1}] })),
            None,
        );
    }
}
