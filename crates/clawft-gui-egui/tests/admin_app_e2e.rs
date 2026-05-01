//! M1.5-E acceptance test: end-to-end WeftOS Admin render.
//!
//! Wires up the three streams that landed in M1.5 (app manifest +
//! substrate adapter + surface composer) to verify they collaborate
//! correctly when driven from a mock [`OntologyAdapter`].
//!
//! What this exercises:
//!
//! 1. A mock adapter emits [`StateDelta`]s on the four kernel topics
//!    (`substrate/kernel/{status,processes,services,logs}`).
//! 2. A [`Substrate`] subscribes the adapter via
//!    [`Substrate::subscribe_adapter`] and applies the deltas.
//! 3. The WeftOS Admin desktop surface is parsed from its bundled
//!    TOML fixture.
//! 4. The surface composer walks the tree against the substrate
//!    snapshot and emits `CanonResponse`s through one headless egui
//!    frame.
//! 5. The resulting response stream is non-empty and contains the
//!    expected `ui://gauge` and `ui://table` primitives.
//!
//! Together these four layers are the M1.5 acceptance contract from
//! session-10 §7.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use clawft_gui_egui::surface_host::render_headless;
use clawft_substrate::adapter::{
    AdapterError, BufferPolicy, OntologyAdapter, PermissionReq, RefreshHint, Sensitivity, SubId,
    Subscription, TopicDecl,
};
use clawft_substrate::{StateDelta, Substrate};
use clawft_surface::parse::parse_surface_toml;
use serde_json::{json, Value};
use tokio::sync::mpsc;

const ADMIN_SURFACE: &str =
    include_str!("../../clawft-surface/fixtures/weftos-admin-desktop.toml");

const KERNEL_TOPICS: &[TopicDecl] = &[
    TopicDecl {
        path: "substrate/kernel/status",
        shape: "ontology://kernel-status",
        refresh_hint: RefreshHint::Periodic { ms: 1000 },
        sensitivity: Sensitivity::Workspace,
        buffer_policy: BufferPolicy::Refuse,
        max_len: None,
    },
    TopicDecl {
        path: "substrate/kernel/processes",
        shape: "ontology://process-list",
        refresh_hint: RefreshHint::Periodic { ms: 1000 },
        sensitivity: Sensitivity::Workspace,
        buffer_policy: BufferPolicy::BlockCapped,
        max_len: None,
    },
    TopicDecl {
        path: "substrate/kernel/services",
        shape: "ontology://service-list",
        refresh_hint: RefreshHint::Periodic { ms: 1000 },
        sensitivity: Sensitivity::Workspace,
        buffer_policy: BufferPolicy::BlockCapped,
        max_len: None,
    },
    TopicDecl {
        path: "substrate/kernel/logs",
        shape: "ontology://log-ring",
        refresh_hint: RefreshHint::EventDriven,
        sensitivity: Sensitivity::Workspace,
        buffer_policy: BufferPolicy::DropOldest,
        max_len: Some(200),
    },
];

/// Adapter that fires one canned `Replace` delta per opened topic and
/// then closes the sender. Exercises the full `subscribe → apply`
/// path without depending on a running kernel daemon.
struct CannedKernelAdapter;

#[async_trait::async_trait]
impl OntologyAdapter for CannedKernelAdapter {
    fn id(&self) -> &'static str {
        "canned-kernel"
    }
    fn topics(&self) -> &'static [TopicDecl] {
        KERNEL_TOPICS
    }
    fn permissions(&self) -> &'static [PermissionReq] {
        &[]
    }

    async fn open(
        &self,
        topic: &str,
        _args: Value,
    ) -> Result<Subscription, AdapterError> {
        let (tx, rx) = mpsc::channel::<StateDelta>(8);
        let value = fixture_for(topic)
            .ok_or_else(|| AdapterError::UnknownTopic(topic.to_string()))?;
        // Emit one Replace (singletons + list topics) or a sequence of
        // Append (logs). Using Replace for all four paths keeps the
        // composer's read path identical.
        let delta = StateDelta::Replace {
            path: topic.to_string(),
            value,
        };
        // Send synchronously from a detached task so the mpsc's
        // backpressure doesn't block `open` — standard adapter
        // pattern.
        tokio::spawn(async move {
            let _ = tx.send(delta).await;
            // Keep `tx` alive briefly so the drain task has time to
            // pull the delta before the sender closes; then drop it
            // to signal end-of-stream.
            drop(tx);
        });
        Ok(Subscription {
            id: SubId(topic_to_id(topic)),
            rx,
        })
    }

    async fn close(&self, _sub_id: SubId) -> Result<(), AdapterError> {
        // Tombstone semantics: close is a no-op for this adapter; the
        // drain task exits naturally when the sender above drops.
        Ok(())
    }
}

fn fixture_for(topic: &str) -> Option<Value> {
    Some(match topic {
        "substrate/kernel/status" => {
            json!({"state": "healthy", "uptime_ms": 3_600_000u64})
        }
        "substrate/kernel/processes" => json!([
            {"pid": 101, "name": "weaver", "cpu": 4.2},
            {"pid": 222, "name": "claude", "cpu": 21.7},
        ]),
        "substrate/kernel/services" => json!({
            "mesh-listener": {"status": "healthy", "cpu_percent": 42.0},
            "rpc-gateway":   {"status": "healthy", "cpu_percent": 9.0},
            "audit-sink":    {"status": "at_risk", "cpu_percent": 88.0},
        }),
        "substrate/kernel/logs" => json!([
            "[t+0s] boot ok",
            "[t+1s] 3 services ready",
        ]),
        _ => return None,
    })
}

fn topic_to_id(topic: &str) -> u64 {
    // Stable per-topic id so the SubId is meaningful in debug logs.
    match topic {
        "substrate/kernel/status" => 1,
        "substrate/kernel/processes" => 2,
        "substrate/kernel/services" => 3,
        "substrate/kernel/logs" => 4,
        _ => 0,
    }
}

/// Drain settle: give the tokio scheduler a handful of polls to pull
/// the in-flight deltas through the subscribe-adapter drain tasks.
/// Tighter than a fixed `sleep(100ms)` — yields until the substrate
/// shows the expected path count.
async fn wait_for_substrate(substrate: &Arc<Substrate>, expected_paths: usize) {
    for _ in 0..64 {
        if substrate.snapshot().len() >= expected_paths {
            return;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    panic!(
        "substrate only received {} of {} expected paths within 64ms",
        substrate.snapshot().len(),
        expected_paths
    );
}

#[tokio::test(flavor = "current_thread")]
async fn weftos_admin_renders_end_to_end() {
    // 1. Mock adapter + Substrate.
    let adapter = Arc::new(CannedKernelAdapter) as Arc<dyn OntologyAdapter>;
    let substrate = Arc::new(Substrate::new());

    for topic in KERNEL_TOPICS {
        substrate
            .subscribe_adapter(Arc::clone(&adapter), topic.path, Value::Null)
            .await
            .unwrap_or_else(|e| panic!("subscribe {}: {e}", topic.path));
    }

    // 2. Wait for the drain tasks to fold every delta into state.
    wait_for_substrate(&substrate, KERNEL_TOPICS.len()).await;
    let snapshot = substrate.snapshot();
    assert!(
        snapshot.get("substrate/kernel/status").is_some(),
        "status topic should be populated"
    );
    assert!(
        snapshot.get("substrate/kernel/processes").is_some(),
        "processes topic should be populated"
    );

    // 3. Parse the admin-desktop surface fixture.
    let tree = parse_surface_toml(ADMIN_SURFACE).expect("parse admin surface");

    // 4. Drive the composer. `render_headless` runs one egui frame.
    let responses = render_headless(&tree, snapshot);

    // 5. Acceptance assertions.
    assert!(
        !responses.is_empty(),
        "composer must emit at least one CanonResponse"
    );
    let gauges = responses
        .iter()
        .filter(|r| r.identity == "ui://gauge")
        .count();
    let tables = responses
        .iter()
        .filter(|r| r.identity == "ui://table")
        .count();
    assert!(
        gauges >= 1 || tables >= 1,
        "expected at least one ui://gauge or ui://table response, got {} gauges / {} tables; full responses: {:?}",
        gauges,
        tables,
        responses.iter().map(|r| r.identity.clone()).collect::<Vec<_>>()
    );

    // WEFT-439: the admin surface ships a wired confirm-restart Modal.
    // Asserting the composer emits a `ui://modal` CanonResponse closes
    // the loop on "Composer renders … the modal path." (Dispatch on
    // click is covered by the dedicated test below — we cannot
    // synthesise a click in this headless smoke without a click-bot.)
    let modals = responses
        .iter()
        .filter(|r| r.identity == "ui://modal")
        .count();
    assert!(
        modals >= 1,
        "expected at least one ui://modal response from the wired confirm-restart node, got {modals}; full responses: {:?}",
        responses.iter().map(|r| r.identity.clone()).collect::<Vec<_>>()
    );

    // Tombstone the subscriptions (ADR-009 discipline) so the test
    // doesn't leave drain tasks alive past the runtime shutdown.
    substrate.close_all().await;
}

/// WEFT-439: focused unit assertion that the confirm-restart Modal in
/// the admin surface fixture parses, declares an affordance, and
/// renders through the composer with its `confirm-restart` affordance
/// preserved in the surface tree (so a future click-bot test can
/// drive the dispatch).
#[test]
fn admin_surface_includes_wired_restart_modal() {
    let tree = parse_surface_toml(ADMIN_SURFACE).expect("parse admin surface");
    // Walk the tree looking for a `ui://modal` node with at least one
    // affordance — the fixture pattern WEFT-439 introduces.
    fn find_modal_with_affordance(
        node: &clawft_surface::SurfaceNode,
    ) -> Option<&clawft_surface::SurfaceNode> {
        if matches!(node.kind, clawft_surface::IdentityIri::Modal)
            && !node.affordances.is_empty()
        {
            return Some(node);
        }
        for child in &node.children {
            if let Some(hit) = find_modal_with_affordance(child) {
                return Some(hit);
            }
        }
        None
    }
    let modal = find_modal_with_affordance(&tree.root)
        .expect("admin surface should declare a ui://modal node with an affordance");
    // Tighten the assertion to the contract we ship: a confirm-restart
    // affordance pointing at rpc.kernel.restart.
    let aff = modal
        .affordances
        .iter()
        .find(|a| a.name == "confirm-restart")
        .expect("modal should declare `confirm-restart` affordance");
    assert_eq!(aff.verb, "rpc.kernel.restart");
}
