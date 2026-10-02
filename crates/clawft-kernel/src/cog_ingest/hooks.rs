//! The ingest bridge as the workload host sees it: issue a token with the
//! host contract, bind it to the placement, revoke it when the instance
//! stops, unloads or its package is revoked.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use super::bridge::{BridgeHandle, BridgeScope, IngestBridge};
use super::registry::{InstanceBinding, TokenRegistry};
use super::types::{INGEST_PATH, IngestError};
use crate::workload_runtime::HostContract;

/// A node's ingest wiring, shared by its workload hosts.
#[derive(Clone)]
pub struct IngestHooks {
    registry: Arc<TokenRegistry>,
    bridge: Arc<IngestBridge>,
    /// URL of the shared loopback bridge, handed to native cogs.
    url: String,
    /// Address to bind instance-scoped listeners on for container
    /// relays (the engine or VM gateway). `None`: containers get no
    /// upstream and rely on `network=host`.
    scoped_bind: Option<IpAddr>,
}

impl IngestHooks {
    /// Hooks over `registry` and `bridge`; native cogs are told `shared`
    /// (where the shared listener is bound).
    pub fn new(
        registry: Arc<TokenRegistry>,
        bridge: Arc<IngestBridge>,
        shared: SocketAddr,
        scoped_bind: Option<IpAddr>,
    ) -> Self {
        Self {
            registry,
            bridge,
            url: format!("http://{shared}{INGEST_PATH}"),
            scoped_bind,
        }
    }

    /// The token registry.
    pub fn registry(&self) -> &Arc<TokenRegistry> {
        &self.registry
    }

    /// Complete `contract` for an instance on `route`: native cogs get the
    /// shared bridge URL; other routes get their own token-scoped listener
    /// (when `scoped_bind` is set) as their `ingest_upstream`.
    pub async fn prepare(
        &self,
        route: &str,
        contract: HostContract,
    ) -> Result<(HostContract, Option<BridgeHandle>), IngestError> {
        if route == "native" {
            return Ok((contract.with_ingest_url(self.url.clone()), None));
        }
        let Some(ip) = self.scoped_bind else {
            return Ok((contract.with_ingest_url(self.url.clone()), None));
        };
        let h = *blake3::hash(contract.token.expose().as_bytes()).as_bytes();
        let handle = self
            .bridge
            .bind(SocketAddr::new(ip, 0), BridgeScope::Token(h))
            .await?;
        let addr = handle.addr();
        let contract = contract
            .with_ingest_upstream(addr)
            .with_ingest_url(format!("http://{addr}{INGEST_PATH}"));
        Ok((contract, Some(handle)))
    }

    /// Bind a loaded instance to its placement and accept its token.
    pub fn lease(
        &self,
        binding: InstanceBinding,
        contract: HostContract,
        listener: Option<BridgeHandle>,
    ) -> Result<IngestLease, IngestError> {
        self.registry.register(binding.clone(), &contract)?;
        Ok(IngestLease {
            binding,
            contract,
            _listener: listener,
        })
    }

    /// Accept the token again (after a stop and a later start).
    pub fn activate(&self, l: &IngestLease) -> Result<(), IngestError> {
        if self.registry.contains(&l.binding.instance_id) {
            return Ok(());
        }
        self.registry.register(l.binding.clone(), &l.contract)
    }

    /// Stop accepting the instance's token.
    pub fn deactivate(&self, l: &IngestLease) {
        self.registry.revoke(&l.binding.instance_id);
        self.bridge.forget(&l.binding.instance_id);
    }
}

/// One instance's ingest registration. Dropping it closes its scoped
/// listener; revoke the token with [`IngestHooks::deactivate`].
pub struct IngestLease {
    /// The placement binding.
    pub binding: InstanceBinding,
    contract: HostContract,
    _listener: Option<BridgeHandle>,
}
