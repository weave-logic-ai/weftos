//! The Fleet roster (COG-010): edge nodes (ESP32 &c.) that **check in** to the host, since they
//! aren't tailnet or Cognitum-mesh peers. Soft state — a heartbeat upserts a node; nodes age to
//! offline after `ONLINE_TTL` and are dropped after `DROP_TTL`. Surfaced under `/network` → `fleet`.
//!
//! The roster is **display-only**: heartbeats are unauthenticated (edge firmware cannot hold a
//! token), so nothing may treat the roster as trusted input (no placement, scheduling or policy
//! decisions read it; only `/network` does). Inputs are therefore bounded: at most [`MAX_NODES`]
//! nodes (oldest evicted), string fields capped, numbers range-checked.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const ONLINE_TTL: u64 = 60; // seen within 60 s = online
const DROP_TTL: u64 = 3600; // forget after 1 h unseen
/// Roster size cap; the least recently seen node is evicted for a new one.
pub const MAX_NODES: usize = 256;
const ID_CAP: usize = 64;
const FIELD_CAP: usize = 128;
/// Request body cap for `/fleet/heartbeat`.
pub const HEARTBEAT_BODY_CAP: usize = 4 * 1024;

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
}

#[derive(Clone, Default)]
struct Node {
    kind: String,
    sensor: String,
    ip: String,
    rssi: Option<i32>,
    battery: Option<f64>,
    fw: String,
    last_seen: u64,
}

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
                        "age_s": age,
                        "online": online,
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
            &Heartbeat { id: "esp-01".into(), kind: Some("esp32".into()), sensor: Some("ecg".into()), rssi: Some(-55), battery: Some(3.9), fw: Some("0.1".into()), ip: None },
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
        Heartbeat { id: id.into(), kind: None, sensor: None, rssi: None, battery: None, fw: None, ip: None }
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
            &Heartbeat { id: format!("{long}\n"), kind: Some(long.clone()), sensor: Some(format!("a\x1b[0mb{long}")), rssi: Some(9999), battery: Some(1e9), fw: Some(long.clone()), ip: Some(long.clone()) },
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
}
