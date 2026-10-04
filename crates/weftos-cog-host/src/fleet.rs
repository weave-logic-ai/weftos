//! The Fleet roster (COG-010): edge nodes (ESP32 &c.) that **check in** to the host, since they
//! aren't tailnet or Cognitum-mesh peers. Soft state — a heartbeat upserts a node; nodes age to
//! offline after `ONLINE_TTL` and are dropped after `DROP_TTL`. Surfaced under `/network` → `fleet`.
//!
//! The roster is **display-only**: heartbeats are unauthenticated (edge firmware cannot hold a
//! token), so nothing may treat the roster as trusted input (no placement, scheduling or policy
//! decisions read it; only `/network` does). Inputs are therefore bounded: at most [`MAX_NODES`]
//! nodes (oldest evicted), string fields capped, numbers range-checked.
//!
//! Heartbeat v2 adds optional `uptime_s`, `load`, `free_heap`, `reset_reason`, `chip`, `mac`,
//! `channel` and `sample_hz`. A v1 heartbeat (without them) is accepted unchanged. Every roster row
//! carries `"provenance": "self_reported"`: the node said it, nothing here was verified, and the
//! console must label it so.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Label on every roster row: the node reported it about itself and nothing verified it.
pub const PROVENANCE: &str = "self_reported";
const ONLINE_TTL: u64 = 60; // seen within 60 s = online
const DROP_TTL: u64 = 3600; // forget after 1 h unseen
/// Roster size cap; the least recently seen node is evicted for a new one.
pub const MAX_NODES: usize = 256;
const ID_CAP: usize = 64;
const FIELD_CAP: usize = 128;
/// Request body cap for `/fleet/heartbeat`.
pub const HEARTBEAT_BODY_CAP: usize = 4 * 1024;

/// `aa:bb:cc:dd:ee:ff` (colons or dashes in, lowercase colons out); anything else is dropped.
fn normalize_mac(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.trim().split([':', '-']).collect();
    if parts.len() != 6 || !parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit())) {
        return None;
    }
    Some(parts.join(":").to_ascii_lowercase())
}

fn clean(s: &str, cap: usize) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(cap).collect()
}

/// A heartbeat POSTed by an edge node to `/fleet/heartbeat`.
#[derive(Clone, Debug, Deserialize)]
pub struct Heartbeat {
    pub id: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub sensor: Option<String>,
    #[serde(default)]
    pub rssi: Option<i32>,
    #[serde(default)]
    pub battery: Option<f64>,
    #[serde(default)]
    pub fw: Option<String>,
    #[serde(default)]
    pub ip: Option<String>,
    // ---- v2 (all optional, self-reported) ----
    #[serde(default)]
    pub uptime_s: Option<u64>,
    /// Load as a fraction of one core (0.0 idle, 1.0 one core busy); may exceed 1 on multi-core.
    #[serde(default)]
    pub load: Option<f64>,
    #[serde(default)]
    pub free_heap: Option<u64>,
    #[serde(default)]
    pub reset_reason: Option<String>,
    #[serde(default)]
    pub chip: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
    /// WiFi channel.
    #[serde(default)]
    pub channel: Option<u32>,
    #[serde(default)]
    pub sample_hz: Option<f64>,
}

#[derive(Clone, Default)]
struct Node {
    kind: String,
    sensor: String,
    ip: String,
    rssi: Option<i32>,
    battery: Option<f64>,
    fw: String,
    uptime_s: Option<u64>,
    load: Option<f64>,
    free_heap: Option<u64>,
    reset_reason: String,
    chip: String,
    mac: String,
    channel: Option<u32>,
    sample_hz: Option<f64>,
    last_seen: u64,
}

/// Upper bounds for self-reported numbers; anything past them is ignored, not stored.
const MAX_UPTIME_S: u64 = 10 * 365 * 86_400;
const MAX_LOAD: f64 = 1000.0;
const MAX_HEAP: u64 = 1 << 40;
const MAX_CHANNEL: u32 = 196;
const MAX_SAMPLE_HZ: f64 = 1_000_000.0;

#[derive(Default)]
pub struct Fleet {
    nodes: HashMap<String, Node>,
}

impl Fleet {
    /// Upsert a node from a heartbeat. `peer_ip` is the socket's address, used if the body omits one.
    pub fn heartbeat(&mut self, h: &Heartbeat, peer_ip: Option<String>) {
        let id = clean(&h.id, ID_CAP);
        if id.is_empty() {
            return;
        }
        if !self.nodes.contains_key(&id) && self.nodes.len() >= MAX_NODES {
            // evict the least recently seen node to make room
            if let Some(oldest) = self.nodes.iter().min_by_key(|(_, v)| v.last_seen).map(|(k, _)| k.clone()) {
                self.nodes.remove(&oldest);
            }
        }
        let n = self.nodes.entry(id).or_default();
        if let Some(k) = &h.kind {
            n.kind = clean(k, FIELD_CAP);
        }
        if let Some(s) = &h.sensor {
            n.sensor = clean(s, FIELD_CAP);
        }
        if let Some(f) = &h.fw {
            n.fw = clean(f, FIELD_CAP);
        }
        // numbers outside a sane range are ignored rather than stored
        if let Some(r) = h.rssi.filter(|r| (-127..=20).contains(r)) {
            n.rssi = Some(r);
        }
        if let Some(b) = h.battery.filter(|b| b.is_finite() && (0.0..=100.0).contains(b)) {
            n.battery = Some(b);
        }
        // v2: a heartbeat is a full report, so a field it omits or sends out of
        // range is cleared rather than left stale from an earlier one.
        n.uptime_s = h.uptime_s.filter(|u| *u <= MAX_UPTIME_S);
        n.load = h.load.filter(|l| l.is_finite() && (0.0..=MAX_LOAD).contains(l));
        n.free_heap = h.free_heap.filter(|m| *m <= MAX_HEAP);
        n.channel = h.channel.filter(|c| (1..=MAX_CHANNEL).contains(c));
        n.sample_hz = h.sample_hz.filter(|r| r.is_finite() && (0.0..=MAX_SAMPLE_HZ).contains(r));
        n.reset_reason = h.reset_reason.as_deref().map(|r| clean(r, FIELD_CAP)).unwrap_or_default();
        n.chip = h.chip.as_deref().map(|c| clean(c, FIELD_CAP)).unwrap_or_default();
        n.mac = h.mac.as_deref().and_then(normalize_mac).unwrap_or_default();
        if let Some(ip) = h.ip.as_deref().map(|s| clean(s, FIELD_CAP)).filter(|s| !s.is_empty()).or(peer_ip) {
            n.ip = ip;
        }
        n.last_seen = now();
    }

    /// The roster as JSON, newest-online first; prunes nodes past `DROP_TTL`.
    pub fn roster(&mut self) -> Value {
        let now = now();
        self.nodes.retain(|_, v| now.saturating_sub(v.last_seen) < DROP_TTL);
        let mut rows: Vec<(bool, String, Value)> = self
            .nodes
            .iter()
            .map(|(id, v)| {
                let age = now.saturating_sub(v.last_seen);
                let online = age < ONLINE_TTL;
                (
                    online,
                    id.clone(),
                    json!({
                        "id": id,
                        "kind": if v.kind.is_empty() { "node" } else { v.kind.as_str() },
                        "sensor": v.sensor,
                        "ip": v.ip,
                        "rssi": v.rssi,
                        "battery": v.battery,
                        "fw": v.fw,
                        "uptime_s": v.uptime_s,
                        "load": v.load,
                        "free_heap": v.free_heap,
                        "reset_reason": v.reset_reason,
                        "chip": v.chip,
                        "mac": v.mac,
                        "channel": v.channel,
                        "sample_hz": v.sample_hz,
                        "age_s": age,
                        "online": online,
                        "provenance": PROVENANCE,
                    }),
                )
            })
            .collect();
        // online first, then by id
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        Value::Array(rows.into_iter().map(|(_, _, v)| v).collect())
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_upserts_and_reports_online() {
        let mut f = Fleet::default();
        f.heartbeat(
            &Heartbeat { kind: Some("esp32".into()), sensor: Some("ecg".into()), rssi: Some(-55), battery: Some(3.9), fw: Some("0.1".into()), ..hb("esp-01") },
            Some("192.168.1.50".into()),
        );
        let r = f.roster();
        let arr = r.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "esp-01");
        assert_eq!(arr[0]["kind"], "esp32");
        assert_eq!(arr[0]["ip"], "192.168.1.50");
        assert_eq!(arr[0]["online"], true);
        assert_eq!(arr[0]["rssi"], -55);
    }

    fn hb(id: &str) -> Heartbeat {
        Heartbeat {
            id: id.into(),
            kind: None,
            sensor: None,
            rssi: None,
            battery: None,
            fw: None,
            ip: None,
            uptime_s: None,
            load: None,
            free_heap: None,
            reset_reason: None,
            chip: None,
            mac: None,
            channel: None,
            sample_hz: None,
        }
    }

    #[test]
    fn roster_is_capped_and_evicts_the_oldest() {
        let mut f = Fleet::default();
        for i in 0..MAX_NODES {
            f.heartbeat(&hb(&format!("n{i}")), None);
            f.nodes.get_mut(&format!("n{i}")).unwrap().last_seen = 1000 + i as u64; // n0 is the oldest
        }
        assert_eq!(f.len(), MAX_NODES);
        f.heartbeat(&hb("fresh"), None);
        assert_eq!(f.len(), MAX_NODES);
        assert!(f.nodes.contains_key("fresh") && !f.nodes.contains_key("n0") && f.nodes.contains_key("n1"));
        // an existing node updates in place without evicting anyone
        f.heartbeat(&hb("n1"), None);
        assert_eq!(f.len(), MAX_NODES);
        assert!(f.nodes.contains_key("n2"));
    }

    #[test]
    fn strings_are_capped_and_numbers_sanity_checked() {
        let mut f = Fleet::default();
        let long = "x".repeat(1000);
        f.heartbeat(
            &Heartbeat { kind: Some(long.clone()), sensor: Some(format!("a\x1b[0mb{long}")), rssi: Some(9999), battery: Some(1e9), fw: Some(long.clone()), ip: Some(long.clone()), ..hb(&format!("{long}\n")) },
            None,
        );
        let r = f.roster();
        let n = &r.as_array().unwrap()[0];
        assert_eq!(n["id"].as_str().unwrap().len(), ID_CAP);
        for k in ["kind", "sensor", "fw", "ip"] {
            assert!(n[k].as_str().unwrap().chars().count() <= FIELD_CAP, "{k}");
        }
        assert!(!n["sensor"].as_str().unwrap().contains('\x1b'));
        assert!(n["rssi"].is_null() && n["battery"].is_null());
        f.heartbeat(&Heartbeat { rssi: Some(-60), battery: Some(f64::NAN), ..hb(&"x".repeat(ID_CAP)) }, None);
        let r = f.roster();
        assert_eq!(r[0]["rssi"], -60);
        assert!(r[0]["battery"].is_null());
        f.heartbeat(&hb("   "), None); // empty id ignored
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn a_v1_heartbeat_still_parses_and_v2_fields_are_stored_and_labelled_self_reported() {
        let v1: Heartbeat = serde_json::from_str(r#"{"id":"esp-01","kind":"esp32","rssi":-60}"#).unwrap();
        assert!(v1.uptime_s.is_none() && v1.load.is_none() && v1.mac.is_none());
        let mut f = Fleet::default();
        f.heartbeat(&v1, None);
        let r = f.roster();
        assert_eq!(r[0]["provenance"], "self_reported");
        assert!(r[0]["uptime_s"].is_null() && r[0]["channel"].is_null());

        let v2: Heartbeat = serde_json::from_str(
            r#"{"id":"esp-01","fw":"0.4.3","uptime_s":3600,"load":0.35,"free_heap":123456,
                "reset_reason":"poweron","chip":"esp32-c6","mac":"20:6e:f1:0b:05:44","channel":6,"sample_hz":10.5,
                "future_field":"ignored"}"#,
        )
        .unwrap();
        f.heartbeat(&v2, None);
        let n = &f.roster()[0];
        assert_eq!((n["uptime_s"].as_u64(), n["free_heap"].as_u64(), n["channel"].as_u64()), (Some(3600), Some(123456), Some(6)));
        assert_eq!((n["load"].as_f64(), n["sample_hz"].as_f64()), (Some(0.35), Some(10.5)));
        assert_eq!((n["chip"].as_str(), n["mac"].as_str(), n["reset_reason"].as_str()), (Some("esp32-c6"), Some("20:6e:f1:0b:05:44"), Some("poweron")));
        assert_eq!(n["fw"], "0.4.3");
        assert_eq!(n["provenance"], "self_reported");
    }

    #[test]
    fn v2_numbers_out_of_range_are_ignored_and_strings_capped() {
        let mut f = Fleet::default();
        let long = "m".repeat(500);
        f.heartbeat(
            &Heartbeat {
                uptime_s: Some(u64::MAX),
                load: Some(f64::INFINITY),
                free_heap: Some(u64::MAX),
                channel: Some(0),
                sample_hz: Some(-1.0),
                mac: Some(format!("{long}\x1b")),
                reset_reason: Some(long.clone()),
                chip: Some(long),
                ..hb("esp-02")
            },
            None,
        );
        let n = &f.roster()[0];
        for k in ["uptime_s", "load", "free_heap", "channel", "sample_hz"] {
            assert!(n[k].is_null(), "{k} should be dropped");
        }
        assert_eq!(n["mac"], "", "a malformed mac is dropped");
        assert!(n["reset_reason"].as_str().unwrap().chars().count() <= FIELD_CAP);
        // each heartbeat replaces the v2 report: an omitted or bad field is cleared
        f.heartbeat(&Heartbeat { channel: Some(11), mac: Some("20-6E-F1-0B-05-44".into()), ..hb("esp-02") }, None);
        assert_eq!((f.roster()[0]["channel"].as_u64(), f.roster()[0]["mac"].as_str()), (Some(11), Some("20:6e:f1:0b:05:44")));
        f.heartbeat(&hb("esp-02"), None);
        assert!(f.roster()[0]["channel"].is_null());
        assert_eq!(f.roster()[0]["mac"], "");
    }
}
