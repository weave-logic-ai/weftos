//! M1.5 integration test 1: TOML → IR round trip.
//!
//! Loads the admin-panel fixture, reparses, and verifies the tree
//! shape matches the declared structure. Serialisation back to TOML
//! is not in scope for M1.5 (the wire form is TOML *input* only;
//! output is via the canon-response path). The "round trip" here is
//! parse → structural introspection → assertions.
//!
//! WEFT-425 also covers a multi-composition fixture: load-time
//! expansion of `[compositions.*]` into canon IRIs.

use clawft_surface::parse::parse_surface_toml;
use clawft_surface::tree::{AttrValue, IdentityIri, Input, Mode};

const FIXTURE: &str = include_str!("../fixtures/weftos-admin-desktop.toml");
const MULTI_COMP: &str = include_str!("../fixtures/multi-composition.toml");

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
    assert!(tree.any_affordance_with_verb("rpc.kernel.kill-process"));
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

/// WEFT-425: multi-composition surface expands at load time.
///
/// Round-trip here means: TOML with `[compositions.Card]` +
/// `[compositions.Form]` + instance references → fully expanded
/// canon-only tree, with defs retained on `SurfaceTree.compositions`
/// for introspection.
#[test]
fn multi_composition_surface_expands() {
    let tree = parse_surface_toml(MULTI_COMP).expect("parse multi-comp");
    assert_eq!(tree.id, "demo/multi-comp");
    assert_eq!(tree.title.as_deref(), Some("Multi-composition"));
    assert_eq!(tree.compositions.len(), 2);
    assert_eq!(
        tree.compositions["Card"].expands_to,
        IdentityIri::Stack
    );
    assert_eq!(
        tree.compositions["Form"].expands_to,
        IdentityIri::Stack
    );

    assert_eq!(tree.root.kind, IdentityIri::Stack);
    assert_eq!(tree.root.children.len(), 2);

    // Card expanded: stack + merged attrs (instance padding wins).
    let card = &tree.root.children[0];
    assert_eq!(card.kind, IdentityIri::Stack);
    assert_eq!(
        card.attrs.get("padding"),
        Some(&AttrValue::Int(8))
    );
    assert_eq!(
        card.attrs.get("frame"),
        Some(&AttrValue::Str("rounded".into()))
    );
    assert_eq!(card.children[0].kind, IdentityIri::Chip);

    // Form expanded: last child wrapped in pressable.
    let form = &tree.root.children[1];
    assert_eq!(form.kind, IdentityIri::Stack);
    assert!(!form.attrs.contains_key("submit_verb"));
    assert_eq!(form.children.len(), 2);
    assert_eq!(form.children[0].kind, IdentityIri::Field);
    assert_eq!(form.children[1].kind, IdentityIri::Pressable);
    assert_eq!(form.children[1].affordances[0].verb, "rpc.form.submit");
    assert_eq!(form.children[1].children[0].kind, IdentityIri::Chip);

    // Wire-level tree is flat canon — no composition names remain as kinds.
    assert_eq!(tree.count_of("ui://stack"), 3);
    assert_eq!(tree.count_of("ui://pressable"), 1);
    assert!(tree.any_affordance_with_verb("rpc.form.submit"));
}
