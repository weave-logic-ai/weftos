//! SWIM-style heartbeat and failure detection for mesh (K6.5).
//!
//! Implements a simplified SWIM protocol for detecting node failures
//! in the mesh network. Each tick, a random peer is pinged directly;
//! if no response, indirect probes are sent via other peers.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Heartbeat configuration.
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// How often to probe a peer (default 1s).
    pub probe_interval: Duration,
    /// Direct ping timeout (default 500ms).
    pub ping_timeout: Duration,
    /// Indirect probe timeout (default 1s).
    pub indirect_timeout: Duration,
    /// Number of indirect probe witnesses (default 3).
    pub indirect_witnesses: usize,
    /// Time in suspect state before marking unreachable (default 5s).
    pub suspect_timeout: Duration,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            probe_interval: Duration::from_secs(1),
            ping_timeout: Duration::from_millis(500),
            indirect_timeout: Duration::from_secs(1),
            indirect_witnesses: 3,
            suspect_timeout: Duration::from_secs(5),
        }
    }
}

/// Ping request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingRequest {
    /// Source node ID.
    pub from_node: String,
    /// Sequence number for correlation.
    pub sequence: u64,
    /// Whether this is an indirect ping (on behalf of another node).
    pub indirect: bool,
    /// If indirect, the original requester.
    pub on_behalf_of: Option<String>,
    /// Mesh-synchronized time from the sender (microseconds since epoch).
    #[serde(default)]
    pub mesh_time_us: u64,
    /// Clock source quality of the sender.
    #[serde(default)]
    pub clock_source: ClockSource,
}

/// Ping response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResponse {
    /// Responding node ID.
    pub from_node: String,
    /// Matching sequence number.
    pub sequence: u64,
    /// Process count on responding node.
    pub process_count: u32,
    /// Service count on responding node.
    pub service_count: u32,
    /// Mesh-synchronized time from the responder (microseconds since epoch).
    #[serde(default)]
    pub mesh_time_us: u64,
    /// Clock source quality of the responder.
    #[serde(default)]
    pub clock_source: ClockSource,
}

// ── Time synchronization ──────────────────────────────────────────

/// Clock source quality (higher = better, wins authority election).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ClockSource {
    /// Local monotonic clock only (no external sync).
    #[default]
    Local = 0,
    /// Synchronized via mesh heartbeat from another node.
    Mesh = 1,
    /// NTP-synchronized system clock.
    Ntp = 2,
    /// WiFi TSF counter (sub-microsecond, same AP only).
    Tsf = 3,
    /// GPS PPS (pulse-per-second) disciplined clock.
    Gps = 4,
}


/// Mesh time synchronization state for a node.
#[derive(Debug, Clone)]
pub struct MeshClockSync {
    /// Current clock offset from authority (microseconds, signed).
    pub offset_us: i64,
    /// Smoothed offset (EMA).
    smoothed_offset: f64,
    /// ID of the current time authority node.
    pub authority_id: Option<String>,
    /// Clock source of the authority.
    pub authority_source: ClockSource,
    /// Our own clock source.
    pub local_source: ClockSource,
    /// Number of sync samples received.
    pub sync_count: u64,
    /// Estimated clock uncertainty (microseconds).
    pub uncertainty_us: u64,
}

impl MeshClockSync {
    /// Create a new clock sync state.
    pub fn new(local_source: ClockSource) -> Self {
        Self {
            offset_us: 0,
            smoothed_offset: 0.0,
            authority_id: None,
            authority_source: ClockSource::Local,
            local_source,
            sync_count: 0,
            uncertainty_us: 1_000_000, // 1 second initial uncertainty
        }
    }

    /// Get the current mesh-synchronized time in microseconds since epoch.
    pub fn mesh_time_us(&self) -> u64 {
        let local = system_time_us();
        (local as i64 + self.offset_us) as u64
    }

    /// Process a time sync sample from a heartbeat.
    ///
    /// If the sender has a better clock source, update our offset.
    /// Uses exponential moving average to smooth jitter.
    pub fn process_sync(
        &mut self,
        sender_id: &str,
        sender_time_us: u64,
        sender_source: ClockSource,
        local_receive_time_us: u64,
    ) {
        // Only sync from equal or better clock sources.
        if sender_source < self.local_source && self.sync_count > 0 {
            return;
        }

        // If this sender has a better source than current authority, switch.
        if sender_source > self.authority_source
            || self.authority_id.is_none()
            || self.authority_id.as_deref() == Some(sender_id)
        {
            let raw_offset = sender_time_us as i64 - local_receive_time_us as i64;

            // Reject outliers: >100ms jump is likely network delay, not drift.
            if self.sync_count > 5 && (raw_offset - self.offset_us).unsigned_abs() > 100_000 {
                return;
            }

            // EMA smoothing: alpha=0.1 for stability.
            let alpha = if self.sync_count < 5 { 0.5 } else { 0.1 };
            self.smoothed_offset =
                (1.0 - alpha) * self.smoothed_offset + alpha * raw_offset as f64;
            self.offset_us = self.smoothed_offset as i64;

            self.authority_id = Some(sender_id.to_string());
            self.authority_source = sender_source;
            self.sync_count += 1;

            // Uncertainty decreases with more samples.
            self.uncertainty_us = match self.sync_count {
                0..=5 => 10_000,    // 10ms
                6..=20 => 1_000,    // 1ms
                21..=100 => 100,    // 100µs
                _ => 50,            // 50µs steady state
            };
        }
    }

    /// Check if this node should be the time authority.
    pub fn is_authority(&self, our_node_id: &str) -> bool {
        self.authority_id.as_deref() == Some(our_node_id)
            || self.authority_id.is_none()
    }

    /// Whether we have a better clock source than the given source.
    pub fn is_better_source(&self, other: ClockSource) -> bool {
        self.local_source > other
    }
}

/// Get current system time in microseconds since Unix epoch.
pub fn system_time_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Node health state from heartbeat monitoring.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatState {
    /// Node is healthy (responding to pings).
    Alive,
    /// Node missed direct ping, awaiting indirect probe.
    Suspect,
    /// Node confirmed unreachable (indirect probes also failed).
    Dead,
}

/// Heartbeat tracker for a single peer.
#[derive(Debug, Clone)]
pub struct PeerHeartbeat {
    /// Current health state.
    pub state: HeartbeatState,
    /// Last successful heartbeat time.
    pub last_seen: Instant,
    /// When suspect state was entered (if applicable).
    pub suspect_since: Option<Instant>,
    /// Consecutive missed pings.
    pub missed_count: u32,
    /// Last ping sequence number sent.
    pub last_ping_seq: u64,
}

impl PeerHeartbeat {
    pub fn new() -> Self {
        Self {
            state: HeartbeatState::Alive,
            last_seen: Instant::now(),
            suspect_since: None,
            missed_count: 0,
            last_ping_seq: 0,
        }
    }

    /// Record a successful heartbeat.
    pub fn record_alive(&mut self) {
        self.state = HeartbeatState::Alive;
        self.last_seen = Instant::now();
        self.suspect_since = None;
        self.missed_count = 0;
    }

    /// Record a missed heartbeat.
    pub fn record_miss(&mut self, config: &HeartbeatConfig) {
        self.missed_count += 1;
        match self.state {
            HeartbeatState::Alive => {
                self.state = HeartbeatState::Suspect;
                self.suspect_since = Some(Instant::now());
            }
            HeartbeatState::Suspect => {
                if let Some(since) = self.suspect_since
                    && since.elapsed() > config.suspect_timeout
                {
                    self.state = HeartbeatState::Dead;
                }
            }
            HeartbeatState::Dead => {} // already dead
        }
    }

    /// Check if this peer should be considered unreachable.
    pub fn is_unreachable(&self, config: &HeartbeatConfig) -> bool {
        self.state == HeartbeatState::Dead
            || self.last_seen.elapsed() > config.suspect_timeout * 3
    }
}

impl Default for PeerHeartbeat {
    fn default() -> Self {
        Self::new()
    }
}

/// Heartbeat tracker managing all peers.
pub struct HeartbeatTracker {
    config: HeartbeatConfig,
    peers: HashMap<String, PeerHeartbeat>,
    next_sequence: u64,
}

impl HeartbeatTracker {
    pub fn new(config: HeartbeatConfig) -> Self {
        Self {
            config,
            peers: HashMap::new(),
            next_sequence: 1,
        }
    }

    /// Start tracking a new peer.
    pub fn add_peer(&mut self, node_id: String) {
        self.peers
            .entry(node_id)
            .or_default();
    }

    /// Remove a peer from tracking.
    pub fn remove_peer(&mut self, node_id: &str) {
        self.peers.remove(node_id);
    }

    /// Record a successful ping response.
    pub fn record_alive(&mut self, node_id: &str) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            peer.record_alive();
        }
    }

    /// Record a missed ping.
    pub fn record_miss(&mut self, node_id: &str) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            peer.record_miss(&self.config);
        }
    }

    /// Get the next ping sequence number.
    pub fn next_sequence(&mut self) -> u64 {
        let seq = self.next_sequence;
        self.next_sequence += 1;
        seq
    }

    /// Get all peers in suspect state.
    pub fn suspect_peers(&self) -> Vec<&str> {
        self.peers
            .iter()
            .filter(|(_, p)| p.state == HeartbeatState::Suspect)
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// Get all peers confirmed dead.
    pub fn dead_peers(&self) -> Vec<&str> {
        self.peers
            .iter()
            .filter(|(_, p)| p.state == HeartbeatState::Dead)
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// Number of tracked peers.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Get health state for a specific peer.
    pub fn peer_state(&self, node_id: &str) -> Option<HeartbeatState> {
        self.peers.get(node_id).map(|p| p.state)
    }
}

/// Observability metrics for a mesh peer, used for affinity scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerMetrics {
    /// Node identifier.
    pub node_id: String,
    /// Smoothed round-trip time in milliseconds.
    pub rtt_ms: f64,
    /// Total messages sent to this peer.
    pub messages_sent: u64,
    /// Total messages received from this peer.
    pub messages_received: u64,
    /// Total bytes sent to this peer.
    pub bytes_sent: u64,
    /// Total bytes received from this peer.
    pub bytes_received: u64,
    /// Total errors encountered with this peer.
    pub errors: u64,
    /// Last update timestamp (epoch millis).
    pub last_updated: u64,
}

impl PeerMetrics {
    /// Create new zeroed metrics for a peer.
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            rtt_ms: 0.0,
            messages_sent: 0,
            messages_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            errors: 0,
            last_updated: 0,
        }
    }

    /// Affinity score (lower is better). Used for service resolution preference.
    pub fn affinity_score(&self) -> f64 {
        let error_rate = if self.messages_sent > 0 {
            self.errors as f64 / self.messages_sent as f64
        } else {
            0.0
        };
        self.rtt_ms + error_rate * 1000.0
    }

    /// Record a successful message send.
    pub fn record_send(&mut self, bytes: u64) {
        self.messages_sent += 1;
        self.bytes_sent += bytes;
    }

    /// Record a received message.
    pub fn record_recv(&mut self, bytes: u64) {
        self.messages_received += 1;
        self.bytes_received += bytes;
    }

    /// Record an error.
    pub fn record_error(&mut self) {
        self.errors += 1;
    }

    /// Update RTT measurement using exponential moving average.
    pub fn update_rtt(&mut self, rtt_ms: f64) {
        self.rtt_ms = self.rtt_ms * 0.8 + rtt_ms * 0.2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_request_serde_roundtrip() {
        let req = PingRequest {
            from_node: "node-a".to_string(),
            sequence: 42,
            indirect: true,
            on_behalf_of: Some("node-b".to_string()),
            mesh_time_us: system_time_us(),
            clock_source: ClockSource::Ntp,
        };
        let json = serde_json::to_string(&req).unwrap();
        let restored: PingRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.from_node, "node-a");
        assert_eq!(restored.sequence, 42);
        assert!(restored.indirect);
        assert_eq!(restored.on_behalf_of, Some("node-b".to_string()));
    }

    #[test]
    fn ping_response_serde_roundtrip() {
        let resp = PingResponse {
            from_node: "node-b".to_string(),
            sequence: 42,
            process_count: 5,
            service_count: 3,
            mesh_time_us: system_time_us(),
            clock_source: ClockSource::Local,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let restored: PingResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.from_node, "node-b");
        assert_eq!(restored.sequence, 42);
        assert_eq!(restored.process_count, 5);
        assert_eq!(restored.service_count, 3);
    }

    #[test]
    fn heartbeat_config_defaults() {
        let config = HeartbeatConfig::default();
        assert_eq!(config.probe_interval, Duration::from_secs(1));
        assert_eq!(config.ping_timeout, Duration::from_millis(500));
        assert_eq!(config.indirect_timeout, Duration::from_secs(1));
        assert_eq!(config.indirect_witnesses, 3);
        assert_eq!(config.suspect_timeout, Duration::from_secs(5));
    }

    #[test]
    fn peer_heartbeat_alive_to_suspect() {
        let config = HeartbeatConfig::default();
        let mut peer = PeerHeartbeat::new();
        assert_eq!(peer.state, HeartbeatState::Alive);

        peer.record_miss(&config);
        assert_eq!(peer.state, HeartbeatState::Suspect);
        assert_eq!(peer.missed_count, 1);
        assert!(peer.suspect_since.is_some());
    }

    #[test]
    fn peer_heartbeat_suspect_to_dead() {
        // Use a zero-duration suspect timeout so the transition is immediate.
        let config = HeartbeatConfig {
            suspect_timeout: Duration::from_secs(0),
            ..HeartbeatConfig::default()
        };
        let mut peer = PeerHeartbeat::new();

        // First miss: Alive -> Suspect
        peer.record_miss(&config);
        assert_eq!(peer.state, HeartbeatState::Suspect);

        // Second miss with zero timeout: Suspect -> Dead
        peer.record_miss(&config);
        assert_eq!(peer.state, HeartbeatState::Dead);
    }

    #[test]
    fn record_alive_resets_state() {
        let config = HeartbeatConfig::default();
        let mut peer = PeerHeartbeat::new();

        peer.record_miss(&config);
        assert_eq!(peer.state, HeartbeatState::Suspect);

        peer.record_alive();
        assert_eq!(peer.state, HeartbeatState::Alive);
        assert_eq!(peer.missed_count, 0);
        assert!(peer.suspect_since.is_none());
    }

    #[test]
    fn tracker_add_remove_peers() {
        let mut tracker = HeartbeatTracker::new(HeartbeatConfig::default());
        tracker.add_peer("node-a".to_string());
        tracker.add_peer("node-b".to_string());
        assert_eq!(tracker.peer_count(), 2);

        tracker.remove_peer("node-a");
        assert_eq!(tracker.peer_count(), 1);
        assert!(tracker.peer_state("node-a").is_none());
        assert_eq!(tracker.peer_state("node-b"), Some(HeartbeatState::Alive));
    }

    #[test]
    fn tracker_suspect_and_dead_peers() {
        let config = HeartbeatConfig {
            suspect_timeout: Duration::from_secs(0),
            ..HeartbeatConfig::default()
        };
        let mut tracker = HeartbeatTracker::new(config);
        tracker.add_peer("node-a".to_string());
        tracker.add_peer("node-b".to_string());
        tracker.add_peer("node-c".to_string());

        // node-a: one miss -> suspect
        tracker.record_miss("node-a");
        // node-b: two misses with zero timeout -> dead
        tracker.record_miss("node-b");
        tracker.record_miss("node-b");
        // node-c: stays alive

        let suspects = tracker.suspect_peers();
        assert_eq!(suspects.len(), 1);
        assert!(suspects.contains(&"node-a"));

        let dead = tracker.dead_peers();
        assert_eq!(dead.len(), 1);
        assert!(dead.contains(&"node-b"));
    }

    // ── PeerMetrics tests ─────────────────────────────────────────

    #[test]
    fn peer_metrics_affinity_score_zero_errors() {
        let mut m = PeerMetrics::new("n1".into());
        m.rtt_ms = 10.0;
        m.messages_sent = 100;
        m.errors = 0;
        // score = rtt + 0 = 10.0
        assert!((m.affinity_score() - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn peer_metrics_affinity_score_with_errors() {
        let mut m = PeerMetrics::new("n1".into());
        m.rtt_ms = 5.0;
        m.messages_sent = 100;
        m.errors = 10;
        // error_rate = 10/100 = 0.1, score = 5.0 + 0.1*1000 = 105.0
        assert!((m.affinity_score() - 105.0).abs() < f64::EPSILON);
    }

    #[test]
    fn peer_metrics_affinity_no_messages() {
        let m = PeerMetrics::new("n1".into());
        // No messages sent: error_rate = 0, score = 0
        assert!((m.affinity_score() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn peer_metrics_record_send_recv() {
        let mut m = PeerMetrics::new("n1".into());
        m.record_send(100);
        m.record_send(200);
        assert_eq!(m.messages_sent, 2);
        assert_eq!(m.bytes_sent, 300);

        m.record_recv(50);
        assert_eq!(m.messages_received, 1);
        assert_eq!(m.bytes_received, 50);
    }

    #[test]
    fn peer_metrics_record_error() {
        let mut m = PeerMetrics::new("n1".into());
        m.record_error();
        m.record_error();
        assert_eq!(m.errors, 2);
    }

    #[test]
    fn peer_metrics_update_rtt_ema() {
        let mut m = PeerMetrics::new("n1".into());
        m.rtt_ms = 100.0;
        m.update_rtt(50.0);
        // EMA: 100*0.8 + 50*0.2 = 80 + 10 = 90
        assert!((m.rtt_ms - 90.0).abs() < f64::EPSILON);
    }

    #[test]
    fn peer_metrics_serde_roundtrip() {
        let mut m = PeerMetrics::new("node-x".into());
        m.rtt_ms = 15.5;
        m.messages_sent = 42;
        let json = serde_json::to_string(&m).unwrap();
        let restored: PeerMetrics = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.node_id, "node-x");
        assert!((restored.rtt_ms - 15.5).abs() < f64::EPSILON);
        assert_eq!(restored.messages_sent, 42);
    }

    #[test]
    fn next_sequence_increments() {
        let mut tracker = HeartbeatTracker::new(HeartbeatConfig::default());
        assert_eq!(tracker.next_sequence(), 1);
        assert_eq!(tracker.next_sequence(), 2);
        assert_eq!(tracker.next_sequence(), 3);
    }

    // ── Time sync tests ──────────────────────────────────────────

    #[test]
    fn clock_source_ordering() {
        assert!(ClockSource::Gps > ClockSource::Ntp);
        assert!(ClockSource::Ntp > ClockSource::Mesh);
        assert!(ClockSource::Mesh > ClockSource::Local);
        assert!(ClockSource::Tsf > ClockSource::Ntp);
    }

    #[test]
    fn mesh_clock_sync_initial_state() {
        let sync = MeshClockSync::new(ClockSource::Local);
        assert_eq!(sync.offset_us, 0);
        assert_eq!(sync.sync_count, 0);
        assert!(sync.authority_id.is_none());
        assert_eq!(sync.uncertainty_us, 1_000_000);
    }

    #[test]
    fn mesh_clock_sync_from_ntp_authority() {
        let mut sync = MeshClockSync::new(ClockSource::Local);
        let now = system_time_us();

        // Simulate NTP authority 5ms ahead.
        let authority_time = now + 5_000;
        sync.process_sync("authority-1", authority_time, ClockSource::Ntp, now);

        assert_eq!(sync.authority_id.as_deref(), Some("authority-1"));
        assert_eq!(sync.authority_source, ClockSource::Ntp);
        assert_eq!(sync.sync_count, 1);
        // First sample with alpha=0.5: offset ≈ 0.5 * 5000 = 2500µs.
        assert!((sync.offset_us - 2_500).unsigned_abs() < 1_000);
    }

    #[test]
    fn mesh_clock_sync_ignores_worse_source() {
        let mut sync = MeshClockSync::new(ClockSource::Ntp);
        let now = system_time_us();

        // We have NTP, peer only has Local — should be ignored after warmup.
        for i in 0..10 {
            sync.process_sync("peer", now + i * 100, ClockSource::Local, now + i * 100);
        }
        // After warmup samples, local source should be ignored.
        let offset_before = sync.offset_us;
        sync.process_sync("peer", now + 50_000, ClockSource::Local, now);
        assert_eq!(sync.offset_us, offset_before);
    }

    #[test]
    fn mesh_clock_sync_smooths_jitter() {
        let mut sync = MeshClockSync::new(ClockSource::Local);
        let now = system_time_us();

        // Send 20 samples with 1ms offset + random jitter.
        for i in 0..20u64 {
            let jitter = (i % 3) as i64 * 50 - 50; // -50, 0, +50µs
            let authority_time = (now as i64 + 1_000 + jitter) as u64;
            sync.process_sync("auth", authority_time, ClockSource::Ntp, now);
        }

        // Offset should be close to 1000µs (1ms).
        assert!((sync.offset_us - 1_000).unsigned_abs() < 500);
        // Uncertainty should have decreased.
        assert!(sync.uncertainty_us < 10_000);
    }

    #[test]
    fn system_time_us_returns_plausible_value() {
        let t = system_time_us();
        // Should be after 2020-01-01 in microseconds.
        assert!(t > 1_577_836_800_000_000);
        // Should be before 2050-01-01.
        assert!(t < 2_524_608_000_000_000);
    }
}
