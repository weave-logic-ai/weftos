//! WEFT-244 + WEFT-245 + WEFT-421: composer surface IR dispatch for
//! the additional canon IRIs (`ui://field`, plus toggle / select /
//! slider / sheet / modal / dock / tabs / tree / plot / media /
//! canvas).
//!
//! Each test builds a tiny surface tree with one of the IRIs as a
//! child of a stack, runs the headless composer, and asserts that
//! the corresponding `CanonResponse` shows up in the output stream.
//! That means the renderer was reached (rather than falling through
//! to `render_todo`).

#![cfg(not(target_arch = "wasm32"))]

use clawft_gui_egui::surface_host::render_headless;
use clawft_surface::builder::{NodeBuilder, Surface, stack};
use clawft_surface::substrate::OntologySnapshot;
use clawft_surface::tree::{AttrValue, IdentityIri};
use serde_json::json;

/// Build a surface with `node` as the sole child of a stack.
fn one_node_surface(node: NodeBuilder) -> clawft_surface::tree::SurfaceTree {
    Surface::new("compose-iri-test")
        .root(stack("/root").child(node))
        .build()
}

#[test]
fn field_iri_renders() {
    // ui://field — text input. With no binding, the rendered string
    // should be empty but the response must show up.
    let n = NodeBuilder::new(IdentityIri::Field, "/root/field")
        .attr("kind", AttrValue::Str("text".into()));
    let tree = one_node_surface(n);
    let snap = OntologySnapshot::empty();
    let resps = render_headless(&tree, snap);
    assert!(
        resps.iter().any(|r| r.identity == "ui://field"),
        "expected a ui://field response; got {:?}",
        resps.iter().map(|r| &r.identity).collect::<Vec<_>>()
    );
}

#[test]
fn toggle_iri_renders_with_bound_value() {
    let n = NodeBuilder::new(IdentityIri::Toggle, "/root/toggle")
        .bind("value", "$flag")
        .bind_literal("label", AttrValue::Str("On".into()));
    let tree = one_node_surface(n);
    let mut snap = OntologySnapshot::empty();
    snap.put("flag", json!(true));
    let resps = render_headless(&tree, snap);
    assert!(resps.iter().any(|r| r.identity == "ui://toggle"));
}

#[test]
fn select_iri_renders_with_options() {
    let n = NodeBuilder::new(IdentityIri::Select, "/root/select")
        .attr(
            "options",
            AttrValue::Array(vec![
                AttrValue::Str("alpha".into()),
                AttrValue::Str("beta".into()),
                AttrValue::Str("gamma".into()),
            ]),
        )
        .bind_literal("value", AttrValue::Str("alpha".into()));
    let tree = one_node_surface(n);
    let snap = OntologySnapshot::empty();
    let resps = render_headless(&tree, snap);
    assert!(resps.iter().any(|r| r.identity == "ui://select"));
}

#[test]
fn slider_iri_renders_within_bounds() {
    let n = NodeBuilder::new(IdentityIri::Slider, "/root/slider")
        .attr("min", AttrValue::Number(0.0))
        .attr("max", AttrValue::Number(100.0))
        .bind_literal("value", AttrValue::Number(42.0));
    let tree = one_node_surface(n);
    let snap = OntologySnapshot::empty();
    let resps = render_headless(&tree, snap);
    assert!(resps.iter().any(|r| r.identity == "ui://slider"));
}

#[test]
fn sheet_iri_descends_into_children() {
    // Sheet wraps a chip; we should see one ui://sheet *and* one
    // ui://chip in the response stream.
    let n = NodeBuilder::new(IdentityIri::Sheet, "/root/sheet").child(
        NodeBuilder::new(IdentityIri::Chip, "/root/sheet/chip")
            .bind_literal("label", AttrValue::Str("inside".into())),
    );
    let tree = one_node_surface(n);
    let snap = OntologySnapshot::empty();
    let resps = render_headless(&tree, snap);
    let kinds: Vec<&str> = resps.iter().map(|r| r.identity.as_ref()).collect();
    assert!(kinds.contains(&"ui://sheet"), "no ui://sheet in {kinds:?}");
    assert!(kinds.contains(&"ui://chip"), "no ui://chip in {kinds:?}");
}

#[test]
fn modal_iri_renders_placeholder() {
    // Modal is the labelled-placeholder path for M4-C; it must at
    // least register a response so the IRI doesn't fall through
    // to render_todo.
    let n = NodeBuilder::new(IdentityIri::Modal, "/root/modal")
        .attr("title", AttrValue::Str("Confirm?".into()));
    let tree = one_node_surface(n);
    let resps = render_headless(&tree, OntologySnapshot::empty());
    assert!(resps.iter().any(|r| r.identity == "ui://modal"));
}

#[test]
fn dock_iri_renders_placeholder() {
    let n = NodeBuilder::new(IdentityIri::Dock, "/root/dock");
    let tree = one_node_surface(n);
    let resps = render_headless(&tree, OntologySnapshot::empty());
    assert!(resps.iter().any(|r| r.identity == "ui://dock"));
}

#[test]
fn tabs_iri_descends_into_selected_tab() {
    let tabs = NodeBuilder::new(IdentityIri::Tabs, "/root/tabs")
        .child(
            NodeBuilder::new(IdentityIri::Chip, "/root/tabs/a")
                .attr("label", AttrValue::Str("A".into()))
                .bind_literal("label", AttrValue::Str("inside-a".into())),
        )
        .child(
            NodeBuilder::new(IdentityIri::Chip, "/root/tabs/b")
                .attr("label", AttrValue::Str("B".into()))
                .bind_literal("label", AttrValue::Str("inside-b".into())),
        );
    let tree = one_node_surface(tabs);
    let resps = render_headless(&tree, OntologySnapshot::empty());
    let kinds: Vec<&str> = resps.iter().map(|r| r.identity.as_ref()).collect();
    assert!(kinds.contains(&"ui://tabs"), "no ui://tabs in {kinds:?}");
    // The selected tab's child must render too.
    assert!(
        kinds.iter().filter(|k| **k == "ui://chip").count() >= 1,
        "expected at least one ui://chip from selected tab; got {kinds:?}"
    );
}

#[test]
fn tree_iri_renders_with_children() {
    let t = NodeBuilder::new(IdentityIri::Tree, "/root/tree")
        .attr("label", AttrValue::Str("Items".into()))
        .child(
            NodeBuilder::new(IdentityIri::Chip, "/root/tree/leaf-1")
                .bind_literal("label", AttrValue::Str("first".into())),
        );
    let tree = one_node_surface(t);
    let resps = render_headless(&tree, OntologySnapshot::empty());
    assert!(resps.iter().any(|r| r.identity == "ui://tree"));
    // Children render inside the open header.
    assert!(resps.iter().any(|r| r.identity == "ui://chip"));
}

#[test]
fn plot_iri_renders_with_points() {
    let n = NodeBuilder::new(IdentityIri::Plot, "/root/plot").bind("points", "$series");
    let tree = one_node_surface(n);
    let mut snap = OntologySnapshot::empty();
    snap.put("series", json!([1.0, 4.0, 9.0, 16.0, 25.0]));
    let resps = render_headless(&tree, snap);
    assert!(resps.iter().any(|r| r.identity == "ui://plot"));
}

#[test]
fn media_iri_renders_with_uri_binding() {
    // ui://media — load an egui::Image from the bound `uri`. With a
    // real URI binding the renderer should emit a `ui://media`
    // response (not the muted no-uri placeholder, which is a plain
    // ui.label and produces no CanonResponse).
    let n = NodeBuilder::new(IdentityIri::Media, "/root/media").bind("uri", "$avatar_url");
    let tree = one_node_surface(n);
    let mut snap = OntologySnapshot::empty();
    snap.put("avatar_url", json!("https://example.test/avatar.png"));
    let resps = render_headless(&tree, snap);
    let kinds: Vec<&str> = resps.iter().map(|r| r.identity.as_ref()).collect();
    assert!(
        kinds.contains(&"ui://media"),
        "expected ui://media response when uri is bound; got {kinds:?}"
    );
}

#[test]
fn canvas_iri_renders_placeholder_painter() {
    // ui://canvas — placeholder painter (faint checkerboard). The
    // primitive must register a CanonResponse so it doesn't fall
    // through to render_todo.
    let n = NodeBuilder::new(IdentityIri::Canvas, "/root/canvas")
        .attr("size_w", AttrValue::Number(120.0))
        .attr("size_h", AttrValue::Number(80.0));
    let tree = one_node_surface(n);
    let resps = render_headless(&tree, OntologySnapshot::empty());
    let kinds: Vec<&str> = resps.iter().map(|r| r.identity.as_ref()).collect();
    assert!(
        kinds.contains(&"ui://canvas"),
        "expected ui://canvas response; got {kinds:?}"
    );
}
