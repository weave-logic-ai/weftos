//! The steward's [`LicenceClient`] for whatever Seed is bound right now
//! (ADR-106 sections 3 and 4, phase 3).
//!
//! A binding can arrive, change steward or be withdrawn while the daemon
//! runs, so the client reads the binding in effect on every call: it signs
//! for that binding's `device_id` (the audience) as that binding's
//! `steward_node_id`, and refuses (`seed_not_bound`, `not_steward`) when no
//! binding is in effect or when it does not name this node and this key.
//! Nothing is sent in those cases.

use std::sync::Arc;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;

use super::client::{
    CheckoutWire, ClockMs, GrantsPage, LicenceClient, LicenceClientError, LicenceResponse, LicenceTransport,
    SignedLicenceClient,
};
use super::request::LicenceRequest;
use super::{BindingRecord, CheckoutGrantStore, SignedGrant};
use crate::workload_pkg::codec::hex_encode;

/// A shared transport, so one link serves every per-call client.
#[derive(Clone)]
pub struct SharedTransport(pub Arc<dyn LicenceTransport>);

#[async_trait]
impl LicenceTransport for SharedTransport {
    async fn call(&self, req: LicenceRequest) -> Result<LicenceResponse, LicenceClientError> {
        self.0.call(req).await
    }
}

/// [`LicenceClient`] bound to the store's binding in effect.
pub struct StewardLicenceClient {
    store: Arc<CheckoutGrantStore>,
    key: SigningKey,
    node_id: String,
    transport: SharedTransport,
    clock: ClockMs,
}

fn refused(code: &str) -> LicenceClientError {
    LicenceClientError::Refused { status: 0, code: code.into() }
}

impl StewardLicenceClient {
    /// A client for this node (`node_id`, signing with `key`) over `transport`.
    pub fn new(
        store: Arc<CheckoutGrantStore>,
        key: SigningKey,
        node_id: impl Into<String>,
        transport: Arc<dyn LicenceTransport>,
        clock: ClockMs,
    ) -> Arc<Self> {
        Arc::new(Self { store, key, node_id: node_id.into(), transport: SharedTransport(transport), clock })
    }

    /// The binding in effect when it names this node and key as its steward.
    pub fn steward_binding(&self) -> Result<BindingRecord, LicenceClientError> {
        let b = self.store.active_binding().ok_or_else(|| refused("seed_not_bound"))?;
        let me = hex_encode(&self.key.verifying_key().to_bytes());
        if b.steward_node_id != self.node_id || b.steward_pubkey != me {
            return Err(refused("not_steward"));
        }
        Ok(b)
    }

    fn signed(&self) -> Result<Arc<SignedLicenceClient<SharedTransport>>, LicenceClientError> {
        let b = self.steward_binding()?;
        Ok(SignedLicenceClient::for_binding(self.key.clone(), &b, self.transport.clone(), self.clock.clone()))
    }
}

#[async_trait]
impl LicenceClient for StewardLicenceClient {
    async fn checkout(&self, req: &CheckoutWire) -> Result<SignedGrant, LicenceClientError> {
        self.signed()?.checkout(req).await
    }

    async fn artifact(&self, blake3_hex: &str, max_len: u64) -> Result<Vec<u8>, LicenceClientError> {
        self.signed()?.artifact(blake3_hex, max_len).await
    }

    async fn grants_since(&self, since: u64) -> Result<Vec<SignedGrant>, LicenceClientError> {
        self.signed()?.grants_since(since).await
    }

    async fn grants_page(&self, since: u64) -> Result<GrantsPage, LicenceClientError> {
        self.signed()?.grants_page(since).await
    }

    async fn renew(&self) -> Result<Vec<SignedGrant>, LicenceClientError> {
        self.signed()?.renew().await
    }
}
