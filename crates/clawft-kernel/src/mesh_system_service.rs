//! Mesh as a [`SystemService`] — start/stop/health_check lifecycle (WEFT-119).
//!
//! Phase 5d historically spawned the mesh transport without registering a
//! service in the kernel [`ServiceRegistry`]. Operators could not inspect or
//! restart mesh through the standard service surface. [`MeshService`] wraps
//! [`MeshRuntime`] and participates in `start_all` / `stop_all` /
//! `health_all`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tracing::info;

use crate::health::HealthStatus;
use crate::mesh_runtime::MeshRuntime;
use crate::service::{ServiceType, SystemService};

/// SystemService adapter for the K6 mesh transport.
///
/// The accept/connect loops are spawned by kernel boot (phase 5d) and handed
/// to this service ([`MeshService::adopt_tasks`]), which stops them on `stop`;
/// it owns the **lifecycle surface**: marking the mesh live,
/// disconnecting peers on stop, and aggregating peer-health into
/// [`HealthStatus`] for the weaver service registry.
pub struct MeshService {
    runtime: Arc<MeshRuntime>,
    started: AtomicBool,
    /// Configured listen address (for diagnostics / health messages).
    listen_addr: Option<String>,
    /// Configured transport name (`tcp`, `ws`, …).
    transport: Option<String>,
    /// Accept loop and seed dial loops spawned by boot; aborted on `stop`
    /// and on drop so a stopped mesh neither accepts nor redials.
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Drop for MeshService {
    fn drop(&mut self) {
        self.abort_tasks();
    }
}

impl MeshService {
    /// Take ownership of the mesh loops so `stop` can end them. `stop` is
    /// terminal for these loops: `start` after `stop` does not respawn them.
    pub fn adopt_tasks(&self, handles: Vec<tokio::task::JoinHandle<()>>) {
        self.tasks.lock().unwrap().extend(handles);
    }

    fn abort_tasks(&self) {
        for h in self.tasks.lock().unwrap().drain(..) {
            h.abort();
        }
    }

    /// Wrap an existing mesh runtime (listen endpoint unknown).
    pub fn new(runtime: Arc<MeshRuntime>) -> Self {
        Self {
            runtime,
            started: AtomicBool::new(false),
            listen_addr: None,
            transport: None,
            tasks: Default::default(),
        }
    }

    /// Wrap a runtime with endpoint metadata from kernel mesh config.
    pub fn with_endpoints(
        runtime: Arc<MeshRuntime>,
        listen_addr: impl Into<String>,
        transport: impl Into<String>,
    ) -> Self {
        Self {
            runtime,
            started: AtomicBool::new(false),
            listen_addr: Some(listen_addr.into()),
            transport: Some(transport.into()),
            tasks: Default::default(),
        }
    }

    /// Shared handle to the underlying mesh runtime.
    pub fn runtime(&self) -> &Arc<MeshRuntime> {
        &self.runtime
    }

    /// Whether [`SystemService::start`] has been called more recently
    /// than [`SystemService::stop`].
    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// Configured listen address, if provided at construction.
    pub fn listen_addr(&self) -> Option<&str> {
        self.listen_addr.as_deref()
    }

    /// Configured transport name, if provided at construction.
    pub fn transport_name(&self) -> Option<&str> {
        self.transport.as_deref()
    }
}

#[async_trait]
impl SystemService for MeshService {
    fn name(&self) -> &str {
        "mesh"
    }

    fn service_type(&self) -> ServiceType {
        ServiceType::Core
    }

    async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Idempotent: boot may call start_all after the listener is already
        // running; operator restart also re-enters start after stop.
        let was = self.started.swap(true, Ordering::SeqCst);
        if was {
            info!(
                node = %self.runtime.node_id(),
                peers = self.runtime.peer_count(),
                "mesh service start (already running)"
            );
            return Ok(());
        }
        info!(
            node = %self.runtime.node_id(),
            peers = self.runtime.peer_count(),
            listen = self.listen_addr.as_deref().unwrap_or("-"),
            transport = self.transport.as_deref().unwrap_or("-"),
            "mesh service started"
        );
        Ok(())
    }

    async fn stop(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let peer_count = self.runtime.peer_count();
        // Stop accepting and redialling first, so the disconnect below is
        // not undone by a seed redial.
        self.abort_tasks();
        self.runtime.disconnect_all_peers();
        self.started.store(false, Ordering::SeqCst);
        info!(
            node = %self.runtime.node_id(),
            peers_disconnected = peer_count,
            "mesh service stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> HealthStatus {
        if !self.started.load(Ordering::SeqCst) {
            return HealthStatus::Unhealthy("mesh service not started".into());
        }

        let unhealthy = self.runtime.check_peer_health();
        let peers = self.runtime.peer_count();
        if unhealthy.is_empty() {
            HealthStatus::Healthy
        } else if peers == 0 || unhealthy.len() >= peers {
            HealthStatus::Unhealthy(format!(
                "all tracked peers unhealthy: {}",
                unhealthy.join(", ")
            ))
        } else {
            HealthStatus::Degraded(format!(
                "{}/{} peers unhealthy: {}",
                unhealthy.len(),
                peers,
                unhealthy.join(", ")
            ))
        }
    }

    #[cfg(feature = "os-patterns")]
    async fn liveness_check(&self) -> crate::health::ProbeResult {
        if self.started.load(Ordering::SeqCst) {
            crate::health::ProbeResult::Live
        } else {
            crate::health::ProbeResult::NotLive {
                reason: "mesh not started".into(),
            }
        }
    }

    #[cfg(feature = "os-patterns")]
    async fn readiness_check(&self) -> crate::health::ProbeResult {
        if self.started.load(Ordering::SeqCst) {
            crate::health::ProbeResult::Ready
        } else {
            crate::health::ProbeResult::NotReady {
                reason: "mesh not started".into(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ServiceRegistry;

    #[tokio::test]
    async fn lifecycle_start_stop_health() {
        let rt = Arc::new(MeshRuntime::new("node-test".into()));
        let svc = MeshService::with_endpoints(Arc::clone(&rt), "127.0.0.1:9489", "tcp");

        assert!(!svc.is_started());
        assert!(matches!(
            svc.health_check().await,
            HealthStatus::Unhealthy(_)
        ));

        svc.start().await.expect("start");
        assert!(svc.is_started());
        assert_eq!(svc.health_check().await, HealthStatus::Healthy);

        // Idempotent start
        svc.start().await.expect("start again");
        assert!(svc.is_started());

        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        rt.add_peer("peer-a".into(), tx);
        assert_eq!(rt.peer_count(), 1);

        svc.stop().await.expect("stop");
        assert!(!svc.is_started());
        assert_eq!(rt.peer_count(), 0, "stop disconnects all peers");
        assert!(matches!(
            svc.health_check().await,
            HealthStatus::Unhealthy(_)
        ));
    }

    #[tokio::test]
    async fn registry_start_all_stop_all() {
        let rt = Arc::new(MeshRuntime::new("node-reg".into()));
        let svc = Arc::new(MeshService::new(Arc::clone(&rt)));
        let registry = ServiceRegistry::new();
        registry.register(svc.clone()).expect("register");

        assert!(registry.get("mesh").is_some());
        assert_eq!(svc.name(), "mesh");
        assert_eq!(svc.service_type(), ServiceType::Core);

        registry.start_all().await.expect("start_all");
        assert!(svc.is_started());
        let health = registry.health_all().await;
        let mesh_health = health
            .iter()
            .find(|(n, _)| n == "mesh")
            .map(|(_, h)| h.clone())
            .expect("mesh in health_all");
        assert_eq!(mesh_health, HealthStatus::Healthy);

        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        rt.add_peer("p1".into(), tx);
        assert_eq!(rt.peer_count(), 1);

        registry.stop_all().await.expect("stop_all");
        assert!(!svc.is_started());
        assert_eq!(rt.peer_count(), 0);
    }

    #[tokio::test]
    async fn health_unhealthy_when_all_discovery_peers_dead() {
        use crate::mesh_heartbeat::HeartbeatConfig;
        use std::time::Duration;

        let mut rt = MeshRuntime::with_discovery("node-d".into(), [1u8; 32]);
        {
            let disc = rt.discovery_mut().expect("discovery");
            // Replace tracker with zero suspect timeout for deterministic dead peers.
            *disc.heartbeat.lock().unwrap() =
                crate::mesh_heartbeat::HeartbeatTracker::new(HeartbeatConfig {
                    suspect_timeout: Duration::from_secs(0),
                    ..HeartbeatConfig::default()
                });
            let mut hb = disc.heartbeat.lock().unwrap();
            hb.add_peer("dying".into());
            // Two misses with zero timeout → Dead.
            hb.record_miss("dying");
            hb.record_miss("dying");
            assert_eq!(hb.dead_peers().len(), 1);
        }
        let rt = Arc::new(rt);
        // Connected peer map should also list the peer so Unhealthy (all peers) path hits.
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        rt.add_peer("dying".into(), tx);

        let svc = MeshService::new(Arc::clone(&rt));
        svc.start().await.unwrap();
        let status = svc.health_check().await;
        assert!(
            matches!(status, HealthStatus::Unhealthy(_)),
            "expected Unhealthy when sole peer is dead, got {status}"
        );
    }

    #[tokio::test]
    async fn stop_ends_the_dial_and_accept_loops() {
        let rt = Arc::new(MeshRuntime::new("n".into()));
        let svc = MeshService::new(Arc::clone(&rt));
        // A dial loop against a dead address would redial forever.
        let handles = crate::mesh_serve::connect_seeds(
            &rt,
            &["127.0.0.1:1".to_string()],
            "tcp",
            None,
            None,
        );
        let aborts: Vec<_> = handles.iter().map(|h| h.abort_handle()).collect();
        svc.adopt_tasks(handles);
        svc.start().await.unwrap();
        assert!(aborts.iter().all(|a| !a.is_finished()));
        svc.stop().await.unwrap();
        for _ in 0..2000 {
            if aborts.iter().all(|a| a.is_finished()) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("dial loops still running after stop");
    }

    #[test]
    fn endpoints_accessors() {
        let rt = Arc::new(MeshRuntime::new("n".into()));
        let svc = MeshService::with_endpoints(Arc::clone(&rt), "0.0.0.0:9489", "ws");
        assert_eq!(svc.listen_addr(), Some("0.0.0.0:9489"));
        assert_eq!(svc.transport_name(), Some("ws"));
        assert_eq!(svc.runtime().node_id(), "n");
    }
}
