//! Mesh sensor topics for LeWM observational frames (WEFT-526).
//!
//! Registers the three `mesh.sensor.v1.*` topics on the kernel mesh wire,
//! frames payloads as [`FrameType::SensorObservation`], and indexes every
//! publish onto an append-only ExoChain stand-in (production hosts bridge to
//! [`crate::chain::ChainManager`]).
//!
//! Wire CBOR + Ed25519 signing lives in `weftos-sensor-pipeline-wire`; this
//! module owns **mesh registration + publish/subscribe + chain index** without
//! a hard dependency on the wire crate (payload is opaque CBOR bytes).

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[cfg(feature = "exochain")]
use crate::chain::EVENT_KIND_MESH_SENSOR_FRAME;

/// Same value as `chain::EVENT_KIND_MESH_SENSOR_FRAME`, which only exists
/// with `exochain`; this keeps the mesh modules buildable without it
/// (`check-mesh-only`). A test under `exochain` guards drift.
#[cfg(not(feature = "exochain"))]
const EVENT_KIND_MESH_SENSOR_FRAME: &str = "mesh.sensor.v1.frame";
use crate::mesh_framing::{FrameType, MeshFrame};
use crate::process::{Pid, ProcessTable};
use crate::topic::TopicRouter;

/// Primary observation stream (`mesh.sensor.v1.encoded`).
pub const TOPIC_ENCODED: &str = "mesh.sensor.v1.encoded";
/// Conditional consensus stream (`mesh.sensor.v1.consensus`).
pub const TOPIC_CONSENSUS: &str = "mesh.sensor.v1.consensus";
/// Multi-publisher control stream (`mesh.sensor.v1.control`).
pub const TOPIC_CONTROL: &str = "mesh.sensor.v1.control";

/// The three observation topics in stable order (matches wire crate).
pub const SENSOR_TOPICS: &[&str] = &[TOPIC_ENCODED, TOPIC_CONSENSUS, TOPIC_CONTROL];

/// Which of the three LeWM sensor topics a frame targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MeshSensorTopic {
    /// `mesh.sensor.v1.encoded`
    Encoded,
    /// `mesh.sensor.v1.consensus`
    Consensus,
    /// `mesh.sensor.v1.control`
    Control,
}

impl MeshSensorTopic {
    /// Bare topic string without cluster suffix.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Encoded => TOPIC_ENCODED,
            Self::Consensus => TOPIC_CONSENSUS,
            Self::Control => TOPIC_CONTROL,
        }
    }

    /// Topic with `.{cluster}` suffix for mesh routing.
    pub fn with_cluster(self, cluster: &str) -> String {
        format!("{}.{}", self.as_str(), cluster)
    }

    /// Parse from bare or cluster-suffixed topic string.
    pub fn parse(s: &str) -> Option<Self> {
        if s == TOPIC_ENCODED || s.starts_with(&format!("{TOPIC_ENCODED}.")) {
            Some(Self::Encoded)
        } else if s == TOPIC_CONSENSUS || s.starts_with(&format!("{TOPIC_CONSENSUS}.")) {
            Some(Self::Consensus)
        } else if s == TOPIC_CONTROL || s.starts_with(&format!("{TOPIC_CONTROL}.")) {
            Some(Self::Control)
        } else {
            None
        }
    }

    /// All variants in stable order.
    pub const ALL: &'static [MeshSensorTopic] =
        &[Self::Encoded, Self::Consensus, Self::Control];
}

/// Envelope carried on the mesh wire + local topic bus for one sensor frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorMeshEnvelope {
    /// Target topic kind.
    pub topic: MeshSensorTopic,
    /// Logical cluster name (topic suffix).
    pub cluster: String,
    /// Emitting node id.
    pub node_id: u64,
    /// Observation / emission timestamp (ms).
    pub timestamp_ms: u64,
    /// Opaque CBOR (typically a signed wire frame from the sensor pipeline).
    pub payload: Vec<u8>,
}

impl SensorMeshEnvelope {
    /// Full topic string including cluster.
    pub fn topic_name(&self) -> String {
        self.topic.with_cluster(&self.cluster)
    }

    /// Build a mesh frame (`FrameType::SensorObservation`) for transport.
    pub fn to_mesh_frame(&self) -> Result<MeshFrame, String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        Ok(MeshFrame {
            frame_type: FrameType::SensorObservation,
            payload: bytes,
        })
    }

    /// Decode from a mesh frame payload.
    pub fn from_mesh_frame(frame: &MeshFrame) -> Result<Self, String> {
        if frame.frame_type != FrameType::SensorObservation {
            return Err(format!(
                "expected SensorObservation frame, got {:?}",
                frame.frame_type
            ));
        }
        serde_json::from_slice(&frame.payload).map_err(|e| e.to_string())
    }

    /// Stable hex digest of the payload for ExoChain indexing (FNV-1a 64).
    pub fn payload_digest_hex(&self) -> String {
        // Lightweight non-crypto fingerprint so this path stays free of an
        // extra hash dep. Production bridges may re-hash with blake3.
        let mut hash: u64 = 0xcbf29ce484222325;
        for &b in &self.payload {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{hash:016x}")
    }
}

/// One ExoChain index record for a published sensor frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorFrameIndexEntry {
    /// Monotonic sequence.
    pub seq: u64,
    /// Event kind (`mesh.sensor.v1.frame`).
    pub kind: String,
    /// Topic string (with cluster).
    pub topic: String,
    /// Cluster name.
    pub cluster: String,
    /// Emitting node.
    pub node_id: u64,
    /// Timestamp ms.
    pub timestamp_ms: u64,
    /// Payload digest hex.
    pub digest: String,
    /// Payload byte length.
    pub payload_len: usize,
}

/// In-memory ExoChain indexer for sensor frames (tests + offline hosts).
#[derive(Debug, Default, Clone)]
pub struct SensorFrameChainIndex {
    /// Appended index entries.
    pub entries: Vec<SensorFrameIndexEntry>,
    next_seq: u64,
}

impl SensorFrameChainIndex {
    /// Empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of indexed frames.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append an envelope; returns assigned sequence.
    pub fn index(&mut self, env: &SensorMeshEnvelope) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        self.entries.push(SensorFrameIndexEntry {
            seq,
            kind: EVENT_KIND_MESH_SENSOR_FRAME.to_string(),
            topic: env.topic_name(),
            cluster: env.cluster.clone(),
            node_id: env.node_id,
            timestamp_ms: env.timestamp_ms,
            digest: env.payload_digest_hex(),
            payload_len: env.payload.len(),
        });
        seq
    }
}

/// Result of a sensor publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorPublishResult {
    /// Topic the frame was published on.
    pub topic: String,
    /// Chain index sequence.
    pub chain_seq: u64,
    /// Number of local in-process subscriber deliveries.
    pub local_deliveries: usize,
    /// Number of TopicRouter PID subscribers currently registered.
    pub router_subscribers: usize,
}

/// Kernel-side publisher: TopicRouter registration + local fan-out + ExoChain.
///
/// Subscribers attach via [`TopicRouter::subscribe`] on cluster-suffixed topic
/// names, and/or via [`SensorTopicBus::subscribe_local`] for in-process tests.
pub struct SensorTopicBus {
    /// Topic router for subscriber registration.
    pub router: Arc<TopicRouter>,
    /// ExoChain stand-in index.
    pub chain: SensorFrameChainIndex,
    /// Last payload published per full topic name.
    last_by_topic: HashMap<String, Vec<u8>>,
    /// Local in-process subscriber queues (topic → payloads).
    local_subs: HashMap<String, Vec<Vec<u8>>>,
}

impl SensorTopicBus {
    /// Construct a bus with a fresh process table for the topic router.
    pub fn new() -> Self {
        let pt = Arc::new(ProcessTable::new(64));
        Self::with_router(Arc::new(TopicRouter::new(pt)))
    }

    /// Construct around an existing topic router (kernel boot path).
    pub fn with_router(router: Arc<TopicRouter>) -> Self {
        Self {
            router,
            chain: SensorFrameChainIndex::new(),
            last_by_topic: HashMap::new(),
            local_subs: HashMap::new(),
        }
    }

    /// Register the three base topics on the router for discovery.
    ///
    /// TopicRouter is lazy (entries appear on subscribe). We touch each base
    /// topic with a sentinel subscribe/unsubscribe so `list_topics` can see
    /// them once real subscribers attach; the constants remain the source of
    /// truth either way.
    pub fn register_well_known_topics(&self) {
        let sentinel: Pid = 0;
        for t in SENSOR_TOPICS {
            self.router.subscribe(sentinel, t);
            self.router.unsubscribe(sentinel, t);
        }
    }

    /// Well-known topic names (always three).
    pub fn well_known_topics(&self) -> &'static [&'static str] {
        SENSOR_TOPICS
    }

    /// Subscribe a process to a sensor topic (with optional cluster suffix).
    pub fn subscribe_pid(&self, pid: Pid, topic: MeshSensorTopic, cluster: Option<&str>) {
        let name = match cluster {
            Some(c) if !c.is_empty() => topic.with_cluster(c),
            _ => topic.as_str().to_string(),
        };
        self.router.subscribe(pid, &name);
    }

    /// Local test subscriber: buffers published payloads for a topic.
    pub fn subscribe_local(&mut self, topic: &str) {
        self.local_subs.entry(topic.to_string()).or_default();
    }

    /// Drain local subscriber buffer for a topic.
    pub fn drain_local(&mut self, topic: &str) -> Vec<Vec<u8>> {
        self.local_subs
            .get_mut(topic)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Publish a sensor envelope: mesh frame encode check, local fan-out,
    /// TopicRouter subscriber count, ExoChain index.
    pub fn publish(&mut self, env: SensorMeshEnvelope) -> Result<SensorPublishResult, String> {
        // Cross-stream mesh link: prove FrameType::SensorObservation encodes.
        let mesh_frame = env.to_mesh_frame()?;
        let _wire = mesh_frame
            .encode()
            .map_err(|e| format!("mesh frame encode: {e}"))?;

        let topic = env.topic_name();
        let payload = env.payload.clone();
        let mut local_deliveries = 0usize;

        if let Some(q) = self.local_subs.get_mut(&topic) {
            q.push(payload.clone());
            local_deliveries += 1;
        }
        let base = env.topic.as_str();
        if base != topic {
            if let Some(q) = self.local_subs.get_mut(base) {
                q.push(payload.clone());
                local_deliveries += 1;
            }
        }

        self.last_by_topic.insert(topic.clone(), payload);
        let router_subscribers = self.router.subscribers(&topic).len();
        let chain_seq = self.chain.index(&env);

        Ok(SensorPublishResult {
            topic,
            chain_seq,
            local_deliveries,
            router_subscribers,
        })
    }

    /// Last payload for a topic (if any).
    pub fn last_payload(&self, topic: &str) -> Option<&[u8]> {
        self.last_by_topic.get(topic).map(|v| v.as_slice())
    }
}

impl Default for SensorTopicBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "exochain")]
    #[test]
    fn frame_event_kind_matches_non_exochain_copy() {
        assert_eq!(super::EVENT_KIND_MESH_SENSOR_FRAME, "mesh.sensor.v1.frame");
    }

    use super::*;

    #[test]
    fn three_topics_registered_and_published() {
        let mut bus = SensorTopicBus::new();
        bus.register_well_known_topics();
        assert_eq!(bus.well_known_topics().len(), 3);
        assert_eq!(MeshSensorTopic::ALL.len(), 3);

        for topic in MeshSensorTopic::ALL {
            let full = topic.with_cluster("alpha");
            bus.subscribe_local(&full);
        }

        let mut seqs = Vec::new();
        for (i, topic) in MeshSensorTopic::ALL.iter().enumerate() {
            let env = SensorMeshEnvelope {
                topic: *topic,
                cluster: "alpha".into(),
                node_id: 7,
                timestamp_ms: 1000 + i as u64,
                payload: format!("frame-{i}").into_bytes(),
            };
            let r = bus.publish(env).expect("publish");
            assert_eq!(r.topic, topic.with_cluster("alpha"));
            assert_eq!(r.local_deliveries, 1);
            seqs.push(r.chain_seq);
        }

        assert_eq!(bus.chain.len(), 3);
        assert_eq!(seqs, vec![0, 1, 2]);
        for e in &bus.chain.entries {
            assert_eq!(e.kind, EVENT_KIND_MESH_SENSOR_FRAME);
            assert!(e.topic.starts_with("mesh.sensor.v1."));
            assert_eq!(e.cluster, "alpha");
            assert_eq!(e.node_id, 7);
            assert!(!e.digest.is_empty());
        }

        // Subscriber path drained one payload per topic.
        for topic in MeshSensorTopic::ALL {
            let drained = bus.drain_local(&topic.with_cluster("alpha"));
            assert_eq!(drained.len(), 1);
        }
    }

    #[test]
    fn mesh_frame_roundtrip_sensor_observation() {
        let env = SensorMeshEnvelope {
            topic: MeshSensorTopic::Encoded,
            cluster: "c".into(),
            node_id: 1,
            timestamp_ms: 42,
            payload: b"cbor-placeholder".to_vec(),
        };
        let frame = env.to_mesh_frame().unwrap();
        assert_eq!(frame.frame_type, FrameType::SensorObservation);
        let wire = frame.encode().unwrap();
        let decoded = MeshFrame::decode(&wire[4..]).unwrap();
        let back = SensorMeshEnvelope::from_mesh_frame(&decoded).unwrap();
        assert_eq!(back, env);
    }

    #[test]
    fn topic_parse_with_cluster() {
        assert_eq!(
            MeshSensorTopic::parse("mesh.sensor.v1.control.east"),
            Some(MeshSensorTopic::Control)
        );
        assert_eq!(MeshSensorTopic::parse("other"), None);
        assert_eq!(TOPIC_ENCODED, "mesh.sensor.v1.encoded");
        assert_eq!(TOPIC_CONSENSUS, "mesh.sensor.v1.consensus");
        assert_eq!(TOPIC_CONTROL, "mesh.sensor.v1.control");
    }
}
