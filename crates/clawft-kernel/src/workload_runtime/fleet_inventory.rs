//! Read-only inventory of Cognitum cloud-registered Seeds (card
//! mesh-placement-23; `fleet-compat.md` sections 1b, 2(b2)).
//!
//! Calls the Cognitum cloud MCP's `fleet_status` tool and lists the Seeds as
//! *candidates* for node binding ([`super::seed_bind`]). It never places,
//! configures or registers anything through the cloud: every call passes
//! [`ReadOnlyFleet`], which refuses any tool outside [`READ_TOOLS`] before
//! it reaches the transport. The OAuth token lives in the operator secret
//! store behind [`super::fleet_mcp::OAuthTokens`]; it is only ever placed
//! in an `Authorization` header and never appears in a chain event, error
//! or log.
//!
//! The parser accepts the output of `fleet_status` in the shapes its schema
//! is expected to take (see `fixtures/cognitum_fleet_status.json`, an
//! assumed shape until the owner captures the real one) and skips, never
//! guesses at, an entry without a device id.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::seed::SeedApiRuntime;
use crate::chain::{ChainManager, EVENT_KIND_WORKLOAD_REFUSE};

/// Chain source of inventory events.
pub const INVENTORY_CHAIN_SOURCE: &str = "workload.inventory";
/// Chain kind of an inventory read.
pub const EVENT_KIND_FLEET_INVENTORY: &str = "workload.fleet.inventory";
/// The only MCP tools this adapter may call: reads.
pub const READ_TOOLS: &[&str] = &["fleet_status"];
/// Most devices accepted from one reply.
pub const MAX_DEVICES: usize = 1000;

/// Why an inventory call failed.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FleetError {
    /// A tool outside [`READ_TOOLS`] was asked for (never sent).
    #[error("MCP tool {0:?} is not a read tool; the inventory adapter is read-only")]
    NonReadTool(String),
    /// The cloud could not be reached or answered with an error.
    #[error("fleet transport: {0}")]
    Transport(String),
    /// The reply is not a recognisable `fleet_status` output.
    #[error("fleet_status output not understood: {0}")]
    Schema(String),
    /// The cloud rejected the credential.
    #[error("the Cognitum cloud refused the credential (HTTP {0})")]
    Unauthorized(u16),
}

/// One MCP tool call transport.
#[async_trait]
pub trait FleetMcp: Send + Sync {
    /// Call tool `name` with `args`; returns the tool's output as JSON.
    async fn call_tool(&self, name: &str, args: Value) -> Result<Value, FleetError>;
}

/// Wraps a transport so only [`READ_TOOLS`] can ever be called.
pub struct ReadOnlyFleet {
    inner: Arc<dyn FleetMcp>,
}

impl ReadOnlyFleet {
    /// Guard `inner`.
    pub fn new(inner: Arc<dyn FleetMcp>) -> Self {
        Self { inner }
    }

    /// Call a read tool; any other name is refused without a request.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value, FleetError> {
        if !READ_TOOLS.contains(&name) {
            return Err(FleetError::NonReadTool(name.to_string()));
        }
        self.inner.call_tool(name, args).await
    }
}

/// A cloud-registered Seed, proposed as a node-binding candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetCandidate {
    /// Device id.
    pub device_id: String,
    /// Firmware version, if reported.
    pub firmware: Option<String>,
    /// Online state, if reported.
    pub online: Option<bool>,
    /// Device public key as the cloud reports it, if reported.
    pub device_pubkey: Option<String>,
}

fn text(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .map(str::to_string)
}

fn online_of(v: &Value) -> Option<bool> {
    if let Some(b) = v.get("online").and_then(Value::as_bool) {
        return Some(b);
    }
    let s = text(v, &["status", "state"])?.to_ascii_lowercase();
    match s.as_str() {
        "online" | "active" | "connected" | "up" => Some(true),
        "offline" | "inactive" | "disconnected" | "down" => Some(false),
        _ => None,
    }
}

/// Parse a `fleet_status` output into candidates; the second value counts
/// entries skipped for lacking a device id.
pub fn parse_fleet_status(out: &Value) -> Result<(Vec<FleetCandidate>, usize), FleetError> {
    let list = match out {
        Value::Array(a) => a,
        Value::Object(_) => ["devices", "seeds", "fleet", "items"]
            .iter()
            .find_map(|k| out.get(*k).and_then(Value::as_array))
            .ok_or_else(|| FleetError::Schema("no devices array".into()))?,
        _ => return Err(FleetError::Schema("not an object or array".into())),
    };
    if list.len() > MAX_DEVICES {
        return Err(FleetError::Schema(format!(
            "more than {MAX_DEVICES} devices"
        )));
    }
    let mut out = Vec::new();
    let mut skipped = 0;
    for d in list {
        let Some(device_id) = text(d, &["device_id", "deviceId", "id"]) else {
            skipped += 1;
            continue;
        };
        out.push(FleetCandidate {
            device_id,
            firmware: text(
                d,
                &["firmware", "firmware_version", "firmwareVersion", "version"],
            ),
            online: online_of(d),
            device_pubkey: text(d, &["public_key", "publicKey", "pubkey"]),
        });
    }
    Ok((out, skipped))
}

/// What the local-versus-cloud key comparison found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetCheck {
    /// The cloud lists this device with the same public key as local pairing.
    Verified,
    /// The cloud does not list the locally paired device.
    NotListed,
    /// The cloud lists the device but reports no key: nothing is verified.
    KeyNotReported,
}

/// The read-only inventory adapter.
pub struct FleetInventory {
    fleet: ReadOnlyFleet,
    chain: Arc<ChainManager>,
    label: String,
}

impl FleetInventory {
    /// Inventory over `transport`. `label` names the credential (never its
    /// value) in chain events.
    pub fn new(transport: Arc<dyn FleetMcp>, chain: Arc<ChainManager>, label: &str) -> Self {
        Self {
            fleet: ReadOnlyFleet::new(transport),
            chain,
            label: label.chars().take(64).collect(),
        }
    }

    /// Read the fleet (optionally one `region`) and chain the read.
    pub async fn candidates(
        &self,
        region: Option<&str>,
    ) -> Result<Vec<FleetCandidate>, FleetError> {
        let args = match region {
            Some(r) => json!({ "region": r }),
            None => json!({}),
        };
        let out = self.fleet.call("fleet_status", args).await?;
        let (list, skipped) = parse_fleet_status(&out)?;
        self.chain.append(
            INVENTORY_CHAIN_SOURCE,
            EVENT_KIND_FLEET_INVENTORY,
            Some(json!({
                "tool": "fleet_status", "credential": self.label, "region": region,
                "candidates": list.iter().map(|c| json!({
                    "device_id": c.device_id, "firmware": c.firmware, "online": c.online,
                })).collect::<Vec<_>>(),
                "skipped_without_device_id": skipped,
            })),
        );
        Ok(list)
    }

    /// Compare the locally paired Seed (`rt`'s own `GET /api/v1/identity`)
    /// with the cloud's listing. A different key for the same device id is
    /// refused and chained.
    pub async fn cross_check(
        &self,
        candidates: &[FleetCandidate],
        rt: &SeedApiRuntime,
    ) -> Result<FleetCheck, FleetError> {
        let (device_id, key) = rt
            .identity()
            .await
            .map_err(|e| FleetError::Transport(e.to_string()))?;
        let Some(c) = candidates.iter().find(|c| c.device_id == device_id) else {
            return Ok(FleetCheck::NotListed);
        };
        match &c.device_pubkey {
            None => Ok(FleetCheck::KeyNotReported),
            Some(k) if *k == key => Ok(FleetCheck::Verified),
            Some(_) => {
                let e = FleetError::Schema(format!(
                    "the cloud's public key for {device_id} differs from the key the Seed reports locally"
                ));
                self.chain.append(
                    INVENTORY_CHAIN_SOURCE,
                    EVENT_KIND_WORKLOAD_REFUSE,
                    Some(json!({
                        "phase": "fleet.inventory", "code": "key_mismatch",
                        "device_id": device_id, "node_id": rt.node_id(),
                        "credential": self.label,
                    })),
                );
                Err(e)
            }
        }
    }
}
