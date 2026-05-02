//! M1.5 integration test: headless composer render.
//!
//! Loads the admin-panel TOML + a synthetic kernel snapshot, runs
//! the composer through one egui pass, and asserts on the emitted
//! `CanonResponse` stream. Lives in `clawft-gui-egui` (not
//! `clawft-surface`) because the composer moved here in M1.5-D to
//! break the canon-types import cycle — see
//! `src/surface_host/mod.rs`.

#![cfg(not(target_arch = "wasm32"))]

use clawft_gui_egui::surface_host::render_headless;
use clawft_surface::parse::parse_surface_toml;
use clawft_surface::substrate::OntologySnapshot;
use serde_json::json;

const FIXTURE: &str = include_str!(
    "../../clawft-surface/fixtures/weftos-admin-desktop.toml"
);

fn healthy_snapshot() -> OntologySnapshot {
    let mut s = OntologySnapshot::empty();
    s.put(
        "substrate/kernel/status",
        json!({"state": "healthy", "uptime_ms": 3_600_000u64}),
    );
    s.put(
        "substrate/kernel/services",
        json!([
            {"name": "mesh-listener", "status": "healthy", "cpu_percent": 42.0},
            {"name": "rpc-gateway",   "status": "healthy", "cpu_percent": 9.0},
            {"name": "audit-sink",    "status": "at_risk", "cpu_percent": 88.0},
        ]),
    );
    s.put(
        "substrate/kernel/processes",
        json!([
            {"pid": 101, "name": "weaver", "cpu": 4.2},
            {"pid": 222, "name": "claude", "cpu": 21.7},
        ]),
    );
    s.put(
        "substrate/kernel/logs",
        json!(["[t+0s] boot ok", "[t+1s] 3 services ready"]),
    );
    s
}

#[test]
fn admin_panel_renders_without_panic() {
    let tree = parse_surface_toml(FIXTURE).expect("parse");
    let snap = healthy_snapshot();
    let responses = render_headless(&tree, snap);
    assert!(
        !responses.is_empty(),
        "composer must emit at least one response for a non-empty surface"
    );
}

#[test]
fn admin_panel_emits_expected_primitive_counts() {
    let tree = parse_surface_toml(FIXTURE).expect("parse");
    let snap = healthy_snapshot();
    let responses = render_headless(&tree, snap);

    let count = |iri: &str| responses.iter().filter(|r| r.identity == iri).count();
    assert_eq!(count("ui://grid"), 1, "one top-level grid");
    // Three chips: two overview chips + the chip declared in the
    // empty-state section appended for D-EM01 compliance under WEFT-589.
    assert_eq!(count("ui://chip"), 3);
    assert_eq!(count("ui://table"), 1, "one process table");
    assert_eq!(count("ui://gauge"), 1, "one mesh-listener gauge");
    assert_eq!(count("ui://stream-view"), 1, "one log stream");
    assert_eq!(count("ui://stack"), 2, "overview stack + services stack");
}

#[test]
fn affordance_kill_is_declared_on_process_table() {
    // Governance intersection is stubbed in M1.5, so every declared
    // affordance survives through the tree to the composer boundary.
    let tree = parse_surface_toml(FIXTURE).expect("parse");
    assert!(tree.any_affordance_with_verb("rpc.kernel.kill"));
}

/// Finding 5: a child with `when` evaluating to false must be skipped
/// by the composer (ADR-016 §6). The emitted `CanonResponse` stream
/// reflects the skip — the hidden chip produces zero responses, so
/// only the parent stack + the one visible chip show up.
#[test]
fn when_false_child_is_skipped_by_composer() {
    use clawft_surface::builder::{chip, stack, Surface};
    use clawft_surface::tree::{AttrValue, Mode};

    let tree = Surface::new("test/when-skip")
        .modes(&[Mode::Desktop])
        .root(
            stack("/root")
                .attr("axis", AttrValue::Str("horizontal".into()))
                .child(
                    chip("/root/visible")
                        .bind_literal("label", AttrValue::Str("visible".into())),
                )
                .child(
                    chip("/root/hidden")
                        .bind_literal("label", AttrValue::Str("hidden".into()))
                        // `$flag == true` against a snapshot where flag=false.
                        .when("$flag == true"),
                ),
        )
        .build();

    let mut snap = OntologySnapshot::empty();
    snap.put("flag", json!(false));

    let responses = render_headless(&tree, snap);

    let chips = responses.iter().filter(|r| r.identity == "ui://chip").count();
    let stacks = responses.iter().filter(|r| r.identity == "ui://stack").count();
    assert_eq!(
        chips, 1,
        "hidden chip must be skipped; expected 1 chip response, got {chips}"
    );
    assert_eq!(stacks, 1, "parent stack must still render");
}
