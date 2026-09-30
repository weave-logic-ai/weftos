//! How the control plane reaches a `workload-host`, and how a node serves
//! one: mesh TCP (optionally Noise XX, as the kernel mesh listener does)
//! and in-process (same wire, in-memory streams) for local and Seed-adapter
//! nodes and for hermetic tests.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::mesh::{MeshError, MeshStream, MeshTransport, TransportListener};
use crate::mesh_noise::{NoiseChannel, NoiseConfig};
use crate::mesh_tcp::TcpTransport;
use crate::mesh_test_support::connected_pair;

use super::host_service::WorkloadHostService;
use super::session::{NoiseStream, serve_connection};

/// Address scheme for in-process hosts.
pub const MEM_SCHEME: &str = "mem://";

/// Opens a stream to a target address.
#[async_trait]
pub trait CtlConnector: Send + Sync {
    /// Connect to `addr`.
    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError>;
}

fn noise_config() -> NoiseConfig {
    NoiseConfig {
        pattern: crate::mesh_noise::NoisePattern::XX,
        local_private_key: rand::random(),
        remote_static_key: None,
    }
}

/// Mesh TCP, with Noise XX when `noise` is set; `mem://` addresses go to
/// registered in-process hosts.
#[derive(Default)]
pub struct MeshConnector {
    noise: bool,
    local: RwLock<HashMap<String, Arc<WorkloadHostService>>>,
}

impl MeshConnector {
    /// Connector; `noise` wraps TCP streams in Noise XX.
    pub fn new(noise: bool) -> Self {
        Self {
            noise,
            local: RwLock::new(HashMap::new()),
        }
    }

    /// Register an in-process host at `mem://<name>` and return the address.
    /// Used for this node's own host and for Seed adapters on an
    /// operator-assigned node id.
    pub fn register_local(&self, name: &str, svc: Arc<WorkloadHostService>) -> String {
        let addr = format!("{MEM_SCHEME}{name}");
        if let Ok(mut m) = self.local.write() {
            m.insert(addr.clone(), svc);
        }
        addr
    }
}

#[async_trait]
impl CtlConnector for MeshConnector {
    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        if addr.starts_with(MEM_SCHEME) {
            let svc = self
                .local
                .read()
                .ok()
                .and_then(|m| m.get(addr).cloned())
                .ok_or_else(|| MeshError::PeerNotConnected(addr.to_string()))?;
            let (client, server) = connected_pair().await?;
            tokio::spawn(async move {
                if let Err(e) = serve_connection(Box::new(server), svc).await {
                    tracing::debug!(error = %e, "in-process workload-host session ended");
                }
            });
            return Ok(Box::new(client));
        }
        let stream = TcpTransport.connect(addr).await?;
        if !self.noise {
            return Ok(stream);
        }
        let ch = NoiseChannel::initiate(stream, &noise_config()).await?;
        Ok(Box::new(NoiseStream::new(Box::new(ch))))
    }
}

/// Accept connections on `listener` and serve each as a `workload-host`
/// session (Noise XX responder when `noise` is set). Runs until the
/// listener fails.
pub async fn serve_listener(
    mut listener: Box<dyn TransportListener>,
    svc: Arc<WorkloadHostService>,
    noise: bool,
) -> Result<(), MeshError> {
    loop {
        let (stream, peer) = listener.accept().await?;
        let svc = svc.clone();
        tokio::spawn(async move {
            let stream: Box<dyn MeshStream> = if noise {
                match NoiseChannel::respond(stream, &noise_config()).await {
                    Ok(ch) => Box::new(NoiseStream::new(Box::new(ch))),
                    Err(e) => {
                        tracing::warn!(%peer, error = %e, "workload-host noise handshake failed");
                        return;
                    }
                }
            } else {
                stream
            };
            if let Err(e) = serve_connection(stream, svc).await {
                tracing::debug!(%peer, error = %e, "workload-host session ended");
            }
        });
    }
}

/// Bind a TCP listener for [`serve_listener`].
pub async fn listen_tcp(addr: &str) -> Result<Box<dyn TransportListener>, MeshError> {
    TcpTransport.listen(addr).await
}
