//! Local (RPC) checkouts on the steward: charged to the caller's own budget
//! and to a node-wide local budget, and gated as the caller's principal.

use std::sync::Arc;
use std::time::Duration;

use super::tests_common::*;
use super::tests_stub::*;
use super::*;

#[tokio::test]
async fn local_checkouts_are_bounded_per_principal_and_node_wide_and_gated_as_the_principal() {
    let net = Arc::new(Net::default());
    let clock = Arc::new(std::sync::atomic::AtomicU64::new(T0));
    let stub = StubLicence::new(clock.clone(), Duration::ZERO);
    let gate = TestGate::new(true);
    let limits = RelayLimits { per_local: 2, local_total: 3, ..RelayLimits::default() };
    let steward = add_member(&net, "a", |fx, ex| {
        let flood = Arc::new(NetFlood { net: net.clone(), from: "a".into(), floods: Default::default() });
        let r = CheckoutRelay::new(fx.store.clone(), ex.clone(), steward_client(&stub, &clock), gate.clone(), flood, None);
        Some(Arc::new(r.with_limits(limits.clone())))
    });
    let m = &steward.mesh;
    assert!(m.checkout_as("project:p1", &wire("aarch64")).await.is_ok());
    assert!(m.checkout_as("project:p1", &wire("aarch64")).await.is_ok());
    assert_eq!(m.checkout_as("project:p1", &wire("aarch64")).await.unwrap_err(), CheckoutRefusal::RateLimited, "per principal");
    assert!(m.checkout_as("operator", &wire("aarch64")).await.is_ok());
    assert_eq!(m.checkout_as("operator", &wire("aarch64")).await.unwrap_err(), CheckoutRefusal::RateLimited, "node-wide");
    // The kernel's own requests are not charged.
    assert!(m.checkout_local(&wire("aarch64")).await.is_ok());
    let asked = gate.asked.lock().unwrap().clone();
    assert!(asked.contains(&("local:project:p1".into(), GATE_ACTION.into())), "{asked:?}");
    assert!(asked.contains(&("local:operator".into(), GATE_ACTION.into())), "{asked:?}");
}
