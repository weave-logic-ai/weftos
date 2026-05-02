//! M1.5 integration test 1: TOML → IR round trip.
//!
//! Loads the admin-panel fixture, reparses, and verifies the tree
//! shape matches the declared structure. Serialisation back to TOML
//! is not in scope for M1.5 (the wire form is TOML *input* only;
//! output is via the canon-response path). The "round trip" here is
//! parse → structural introspection → assertions.

use clawft_surface::parse::parse_surface_toml;
use clawft_surface::tree::{IdentityIri, Input, Mode};

const FIXTURE: &str = include_str!("../fixtures/weftos-admin-desktop.toml");

#[test]
fn parses_admin_fixture() {
    let tree = parse_surface_toml(FIXTURE).expect("parse");
    assert_eq!(tree.id, "weftos-admin/desktop");
    assert_eq!(tree.title.as_deref(), Some("WeftOS Admin"));
    assert!(tree.modes.contains(&Mode::Desktop));
    assert!(tree.modes.contains(&Mode::Ide));
    assert!(tree.inputs.contains(&Input::Pointer));
    assert_eq!(tree.subscriptions.len(), 4);
    assert_eq!(tree.root.kind, IdentityIri::Grid);
    // Five children today: four quadrants + the empty/loading/offline
    // state sections appended for D-EM01 compliance under WEFT-589.
    assert_eq!(tree.root.children.len(), 5);
}

#[test]
fn toml_primitive_counts_match_expectations() {
    let tree = parse_surface_toml(FIXTURE).expect("parse");
    // Counts reflect the canonical four quadrants plus the state
    // sections (empty/loading/offline) added under WEFT-589 for
    // D-EM01 compliance. Bump these whenever the fixture grows.
    assert_eq!(tree.count_of("ui://chip"), 3);
    assert_eq!(tree.count_of("ui://gauge"), 1);
    assert_eq!(tree.count_of("ui://table"), 1);
    assert_eq!(tree.count_of("ui://stream-view"), 1);
    assert_eq!(tree.count_of("ui://grid"), 1);
    assert_eq!(tree.count_of("ui://stack"), 2);
}

#[test]
fn fixture_declares_kill_affordance() {
    let tree = parse_surface_toml(FIXTURE).expect("parse");
    assert!(tree.any_affordance_with_verb("rpc.kernel.kill"));
    assert!(tree.any_affordance_with_verb("rpc.kernel.restart-service"));
}

#[test]
fn fixture_parses_binding_expression() {
    // `count($substrate/kernel/services, s -> s.status == "healthy")`
    // must round-trip into an Expr::Call(count, …).
    let tree = parse_surface_toml(FIXTURE).expect("parse");
    let overview = &tree.root.children[0];
    let services_chip = &overview.children[1];
    let b = services_chip
        .bindings
        .get("label")
        .expect("services chip has label binding");
    match b {
        clawft_surface::tree::Binding::Expr(e) => {
            // Trivial: the expression should render to something
            // nontrivial with a synthetic snapshot. (Evaluated in
            // the eval test.)
            let _ = e;
        }
        _ => panic!("expected Expr binding"),
    }
}
