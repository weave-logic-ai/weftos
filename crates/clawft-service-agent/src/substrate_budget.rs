//! Substrate-backed [`BudgetStore`] (WEFT-322 / agent-core-v1.1 M5-B).
//!
//! Persists per-conversation budget accumulators at
//! `substrate/_derived/chat/<conv_id>/budget.json` (one Replace per
//! mutation) so the per-conv circuit-state survives daemon restart.
//! Mirrors the path layout used by [`SubstrateConversationSink`] —
//! both write through the daemon's `chat` `DerivedWriteGrant`.
//!
//! ## Why a sibling module
//!
//! The trait surface is the in-core
//! [`clawft_core::agent::cost_budget::BudgetStore`]. The substrate impl
//! lives here so the kernel dependency stays scoped to
//! `clawft-service-agent` (the same architectural rule the
//! [`SubstrateConversationSink`] follows).
//!
//! Reuses the [`SubstrateClient`] test seam from
//! [`crate::substrate_sink`] so unit tests can swap in a `Mutex<HashMap>`
//! without spinning up a kernel.

use std::sync::Arc;

use clawft_core::agent::cost_budget::{BudgetStore, BudgetUsage};
use serde_json::Value;

use crate::substrate_sink::{KernelSubstrateClient, SubstrateClient};

/// [`BudgetStore`] backed by the daemon's substrate.
///
/// Each `save` issues a single `Replace` against
/// `substrate/_derived/chat/<conv_id>/budget.json`; each `load` reads
/// the same path and falls back to [`BudgetUsage::default`] when the
/// path is unset (first call on a conv).
pub struct SubstrateBudgetStore {
    client: Arc<dyn SubstrateClient>,
    /// Daemon node-id — caller for the gated publish (grant lookup
    /// keys on it). Same role as `node_id` on
    /// [`SubstrateConversationSink`].
    node_id: String,
}

impl SubstrateBudgetStore {
    /// Build against an arbitrary [`SubstrateClient`]. Tests pass a
    /// `Mutex<HashMap>` stub here.
    pub fn with_client(client: Arc<dyn SubstrateClient>, node_id: impl Into<String>) -> Self {
        Self {
            client,
            node_id: node_id.into(),
        }
    }

    /// Convenience: build against a real kernel pair. Mirrors
    /// [`SubstrateConversationSink::new`].
    pub fn new(
        substrate: clawft_kernel::SubstrateService,
        node_registry: clawft_kernel::NodeRegistry,
        node_id: impl Into<String>,
    ) -> Self {
        Self::with_client(
            Arc::new(KernelSubstrateClient::new(substrate, node_registry)),
            node_id,
        )
    }

    /// Substrate path for the per-conv budget JSON.
    fn budget_path(conv_id: &str) -> String {
        // `.json` suffix matches the wire shape called out in the
        // WEFT-322 spec item 4 (`derived/chat/<conv_id>/budget.json`).
        format!("substrate/_derived/chat/{conv_id}/budget.json")
    }
}

impl BudgetStore for SubstrateBudgetStore {
    fn load(&self, conv_id: &str) -> BudgetUsage {
        let path = Self::budget_path(conv_id);
        match self.client.read(&path) {
            Ok(Some(v)) => parse_usage(&v).unwrap_or_default(),
            Ok(None) => BudgetUsage::default(),
            Err(e) => {
                tracing::warn!(
                    conv_id,
                    error = %e,
                    "substrate budget load failed; treating as zero usage"
                );
                BudgetUsage::default()
            }
        }
    }

    fn save(&self, conv_id: &str, usage: &BudgetUsage) -> Result<(), String> {
        let path = Self::budget_path(conv_id);
        let body = serde_json::to_value(usage)
            .map_err(|e| format!("budget serialise failed: {e}"))?;
        self.client.publish(&self.node_id, &path, body).map(|_| ())
    }
}

/// Parse a substrate-resident budget record back into a [`BudgetUsage`].
/// Returns `None` on malformed payloads so callers can fall back to a
/// zero default rather than failing the whole load.
fn parse_usage(v: &Value) -> Option<BudgetUsage> {
    serde_json::from_value::<BudgetUsage>(v.clone()).ok()
}

#[cfg(test)]
mod tests {
    //! Inline unit tests use a `Mutex<HashMap>` stub of
    //! [`SubstrateClient`] so we don't pull in the kernel. The
    //! substrate path layout + serde round-trip are the load-bearing
    //! invariants per the WEFT-322 spec (item 4).

    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct StubClient {
        store: StdMutex<HashMap<String, Value>>,
    }

    impl SubstrateClient for StubClient {
        fn publish(&self, _node_id: &str, path: &str, value: Value) -> Result<u64, String> {
            self.store
                .lock()
                .map_err(|_| "poisoned".to_string())?
                .insert(path.to_string(), value);
            Ok(1)
        }
        fn list(&self, _prefix: &str, _depth: u32) -> Result<Vec<String>, String> {
            Ok(Vec::new())
        }
        fn read(&self, path: &str) -> Result<Option<Value>, String> {
            Ok(self
                .store
                .lock()
                .map_err(|_| "poisoned".to_string())?
                .get(path)
                .cloned())
        }
    }

    #[test]
    fn budget_path_matches_spec() {
        assert_eq!(
            SubstrateBudgetStore::budget_path("conv-123"),
            "substrate/_derived/chat/conv-123/budget.json"
        );
    }

    #[test]
    fn empty_load_returns_default_without_error() {
        let store = SubstrateBudgetStore::with_client(Arc::new(StubClient::default()), "daemon");
        let u = store.load("never-seen");
        assert_eq!(u, BudgetUsage::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let store = SubstrateBudgetStore::with_client(Arc::new(StubClient::default()), "daemon");
        let mut u = BudgetUsage::default();
        u.add_call(100, 50, 0.25);
        u.add_call(60, 40, 0.50);
        u.circuit_open = true;
        u.tripped_dimension = Some("usd".into());
        store.save("conv-rt", &u).unwrap();

        let loaded = store.load("conv-rt");
        assert_eq!(loaded.input_tokens, 160);
        assert_eq!(loaded.output_tokens, 90);
        assert!((loaded.usd - 0.75).abs() < 1e-9);
        assert_eq!(loaded.iterations, 2);
        assert!(loaded.circuit_open);
        assert_eq!(loaded.tripped_dimension.as_deref(), Some("usd"));
    }

    #[test]
    fn malformed_payload_falls_back_to_default() {
        let stub = Arc::new(StubClient::default());
        // Plant a non-budget value at the budget path.
        stub.store
            .lock()
            .unwrap()
            .insert(
                SubstrateBudgetStore::budget_path("conv-bad"),
                serde_json::json!("not an object"),
            );
        let store = SubstrateBudgetStore::with_client(stub, "daemon");
        let u = store.load("conv-bad");
        assert_eq!(u, BudgetUsage::default());
    }

    #[test]
    fn distinct_convs_are_isolated() {
        let stub = Arc::new(StubClient::default());
        let store = SubstrateBudgetStore::with_client(stub, "daemon");
        let mut a = BudgetUsage::default();
        a.add_call(10, 0, 0.0);
        store.save("conv-a", &a).unwrap();
        let mut b = BudgetUsage::default();
        b.add_call(99, 0, 0.0);
        store.save("conv-b", &b).unwrap();

        assert_eq!(store.load("conv-a").input_tokens, 10);
        assert_eq!(store.load("conv-b").input_tokens, 99);
    }

    #[test]
    fn save_overwrites_prior_record() {
        let store = SubstrateBudgetStore::with_client(Arc::new(StubClient::default()), "daemon");
        let mut u = BudgetUsage::default();
        u.add_call(50, 50, 0.10);
        store.save("conv-ow", &u).unwrap();
        let mut u2 = BudgetUsage::default();
        u2.add_call(1, 1, 0.01);
        store.save("conv-ow", &u2).unwrap();

        let loaded = store.load("conv-ow");
        assert_eq!(loaded.input_tokens, 1);
        assert_eq!(loaded.output_tokens, 1);
    }
}
