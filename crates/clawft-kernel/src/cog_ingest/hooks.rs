//! The ingest bridge as the workload host sees it: authorise the placing
//! project, issue a token with the host contract, bind it to the placement,
//! and revoke it when the instance stops or unloads (a package revocation
//! that force-unloads an instance goes through the same unload; no such
//! path exists in the host yet).
//!
//! A cog instance registered by [`IngestHooks::lease`] lives in the host's
//! in-memory instance table. Anything that re-creates an instance record
//! without going through the host's `place` (a controller adopting an
//! in-flight placement, a host restart that re-adopts running instances)
//! must call `lease` again, or the cog's token is unknown and its posts are
//! refused.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use super::bridge::{BridgeHandle, BridgeScope, IngestBridge};
use super::registry::{InstanceBinding, TokenRegistry};
use super::types::{INGEST_PATH, IngestError};
use crate::workload_runtime::HostContract;

/// Which node a project's key is bound to, from the identity records.
pub trait ProjectDirectory: Send + Sync {
    /// Node id of the key currently bound to `project_id`; `None` when the
    /// project is unregistered or every key of it is revoked.
    fn bound_node(&self, project_id: &str) -> Option<String>;
}

/// A node's ingest wiring, shared by its workload hosts.
#[derive(Clone)]
pub struct IngestHooks {
    /// This node's id: its own requests may place for any project it routes.
    own_node: String,
    registry: Arc<TokenRegistry>,
    /// `None`: the bridge could not start (see `reason`).
    bridge: Option<Arc<IngestBridge>>,
    /// URL of the shared loopback bridge, handed to native cogs.
    url: String,
    /// Address to bind instance-scoped listeners on for container relays
    /// (the engine or VM gateway). `None`: containers get no upstream and
    /// rely on `network=host`.
    scoped_bind: Option<IpAddr>,
    reason: Option<String>,
    /// Per project: the controllers (node ids) besides this node that may
    /// place cogs for it.
    controllers: HashMap<String, HashSet<String>>,
    directory: Option<Arc<dyn ProjectDirectory>>,
}

impl IngestHooks {
    /// Hooks over `registry` and `bridge`; native cogs are told `shared`
    /// (where the shared listener is bound).
    pub fn new(
        own_node: impl Into<String>,
        registry: Arc<TokenRegistry>,
        bridge: Arc<IngestBridge>,
        shared: SocketAddr,
        scoped_bind: Option<IpAddr>,
    ) -> Self {
        Self {
            own_node: own_node.into(),
            registry,
            bridge: Some(bridge),
            url: format!("http://{shared}{INGEST_PATH}"),
            scoped_bind,
            reason: None,
            controllers: HashMap::new(),
            directory: None,
        }
    }

    /// Hooks for a node whose bridge could not start. Cogs are placed with
    /// no token and no URL, and every place result, status and
    /// advertisement says `ingest: disabled`.
    pub fn disabled(own_node: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            own_node: own_node.into(),
            registry: Arc::new(TokenRegistry::new()),
            bridge: None,
            url: String::new(),
            scoped_bind: None,
            reason: Some(reason.into()),
            controllers: HashMap::new(),
            directory: None,
        }
    }

    /// Controllers (node ids) besides this node allowed to place for each
    /// project.
    pub fn with_project_controllers(mut self, c: HashMap<String, HashSet<String>>) -> Self {
        self.controllers = c;
        self
    }

    /// Identity records: a project's bound key may place for it.
    pub fn with_project_directory(mut self, d: Arc<dyn ProjectDirectory>) -> Self {
        self.directory = Some(d);
        self
    }

    /// True when the bridge is running.
    pub fn is_enabled(&self) -> bool {
        self.bridge.is_some()
    }

    /// `enabled` or `disabled`.
    pub fn state(&self) -> &'static str {
        if self.is_enabled() { "enabled" } else { "disabled" }
    }

    /// Why the bridge is disabled.
    pub fn disabled_reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The token registry.
    pub fn registry(&self) -> &Arc<TokenRegistry> {
        &self.registry
    }

    /// May the controller in `b.controller_node` place a cog for
    /// `b.project_id`, and is there a store owner for the placement? The
    /// instance id is not known yet at place time and is ignored.
    ///
    /// A project needs one of: the controller is this node; the controller
    /// is listed for the project in the config; the controller is the node
    /// of the project's bound key. Whatever the project, a route to a store
    /// owner must exist, so a placement whose vectors could not be delivered
    /// is refused instead of placed.
    pub fn authorize(&self, b: &InstanceBinding) -> Result<(), IngestError> {
        let Some(bridge) = &self.bridge else {
            return Ok(());
        };
        if let Some(p) = &b.project_id {
            let me = b.controller_node == self.own_node;
            let listed = self
                .controllers
                .get(p)
                .is_some_and(|s| s.contains(&b.controller_node));
            let bound = self
                .directory
                .as_ref()
                .and_then(|d| d.bound_node(p))
                .is_some_and(|n| n == b.controller_node);
            if !(me || listed || bound) {
                return Err(IngestError::Forbidden);
            }
        }
        if !bridge.has_route(b) {
            return Err(IngestError::NotRouted(match &b.project_id {
                Some(p) => format!("project {p} is not routed here"),
                None => format!("no store route for controller {}", b.controller_node),
            }));
        }
        Ok(())
    }

    /// Complete `contract` for an instance on `route`: native cogs get the
    /// shared bridge URL; other routes get their own token-scoped listener
    /// (when `scoped_bind` is set) as their `ingest_upstream`.
    pub async fn prepare(
        &self,
        route: &str,
        contract: HostContract,
    ) -> Result<(HostContract, Option<BridgeHandle>), IngestError> {
        let Some(bridge) = &self.bridge else {
            return Ok((contract, None));
        };
        let Some(ip) = self.scoped_bind.filter(|_| route != "native") else {
            return Ok((contract.with_ingest_url(self.url.clone()), None));
        };
        let h = *blake3::hash(contract.token.expose().as_bytes()).as_bytes();
        let handle = bridge
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
        if let Some(b) = &self.bridge {
            b.forget(&l.binding.instance_id);
        }
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
