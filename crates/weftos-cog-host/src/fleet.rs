//! The Fleet roster (COG-010): edge nodes (ESP32 &c.) that **check in** to the host, since they
//! aren't tailnet or Cognitum-mesh peers. Soft state — a heartbeat upserts a node; nodes age to
//! offline after `ONLINE_TTL` and are dropped after `DROP_TTL`. Surfaced under `/network` → `fleet`.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const ONLINE_TTL: u64 = 60; // seen within 60 s = online
const DROP_TTL: u64 = 3600; // forget after 1 h unseen

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
        let n = self.nodes.entry(h.id.clone()).or_default();
        if let Some(k) = &h.kind {
            n.kind = k.clone();
        }
        if let Some(s) = &h.sensor {
            n.sensor = s.clone();
        }
        if let Some(f) = &h.fw {
            n.fw = f.clone();
        }
        if h.rssi.is_some() {
            n.rssi = h.rssi;
        }
        if h.battery.is_some() {
            n.battery = h.battery;
        }
        if let Some(ip) = h.ip.clone().or(peer_ip) {
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
}
