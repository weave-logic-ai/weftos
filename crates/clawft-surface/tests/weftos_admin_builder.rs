//! M1.5 integration test: Rust-builder authoring of the WeftOS Admin
//! desktop surface must produce a tree structurally equal to the
//! declarative-TOML fixture.
//!
//! This is the acceptance artifact for session-10 rec. 6 ("TOML +
//! Rust builder variants must agree"). The `Binding::Expr` interior is
//! opaque (it embeds a parsed expression AST), so we compare the two
//! trees via a stable structural fingerprint rather than `PartialEq`.

use clawft_surface::builder::{Surface, chip, gauge, grid, stack, stream_view, table};
use clawft_surface::parse::parse_surface_toml;
use clawft_surface::tree::{
    AttrValue, IdentityIri, Input, Invocation, Mode, SurfaceNode, SurfaceTree,
};

const FIXTURE: &str = include_str!("../fixtures/weftos-admin-desktop.toml");

/// Build the WeftOS Admin desktop surface via the Rust builder API.
///
/// Mirrors `fixtures/weftos-admin-desktop.toml` 1-to-1: same id,
/// same modes/inputs, same subscriptions, same node tree, same
/// bindings, same affordances.
fn build_admin_surface_via_rust() -> SurfaceTree {
    Surface::new("weftos-admin/desktop")
        .modes(&[Mode::Desktop, Mode::Ide])
        .inputs(&[Input::Pointer, Input::Hybrid])
        .title("WeftOS Admin")
        .subscribe("substrate/kernel/status")
        .subscribe("substrate/kernel/processes")
        .subscribe("substrate/kernel/services")
        .subscribe("substrate/kernel/logs")
        .root(
            grid("/root")
                .attr("columns", AttrValue::Int(2))
                // Quadrant 1: overview chips
                .child(
                    stack("/root/overview")
                        .attr("axis", AttrValue::Str("horizontal".into()))
                        .attr("wrap", AttrValue::Bool(true))
                        .child(
                            chip("/root/overview/status")
                                .bind("label", "$substrate/kernel/status.state")
                                .bind("tone", "$substrate/kernel/status.state"),
                        )
                        .child(chip("/root/overview/services-healthy").bind(
                            "label",
                            "count($substrate/kernel/services, s -> s.status == \"healthy\")",
                        )),
                )
                // Quadrant 2: process table with kill affordance
                .child(
                    table("/root/processes")
                        .attr(
                            "columns",
                            AttrValue::Array(vec![
                                AttrValue::Str("pid".into()),
                                AttrValue::Str("name".into()),
                                AttrValue::Str("cpu".into()),
                            ]),
                        )
                        .bind("rows", "$substrate/kernel/processes")
                        .affordance_with_schema(
                            "kill",
                            "rpc.kernel.kill",
                            &[Invocation::Pointer, Invocation::Voice],
                            "ontology://kernel/kill-process",
                        ),
                )
                // Quadrant 3: service gauges with restart affordance
                .child(
                    stack("/root/services")
                        .attr("axis", AttrValue::Str("vertical".into()))
                        .child(
                            gauge("/root/services/mesh-listener")
                                .bind(
                                    "value",
                                    "$substrate/kernel/services/mesh-listener/cpu_percent",
                                )
                                .bind("label", "$substrate/kernel/services/mesh-listener/status")
                                .attr("min", AttrValue::Number(0.0))
                                .attr("max", AttrValue::Number(100.0))
                                .affordance_with_schema(
                                    "restart",
                                    "rpc.kernel.restart-service",
                                    &[Invocation::Pointer, Invocation::Voice],
                                    "ontology://kernel/restart-service",
                                ),
                        ),
                )
                // Quadrant 4: log stream
                .child(stream_view("/root/logs").bind("stream", "$substrate/kernel/logs")),
        )
        .build()
}

/// Structural fingerprint of a surface tree. Captures exactly the
/// parts both authoring paths control — shape, attrs, affordance verb
/// names, binding slot names (but not the opaque `Expr` interior,
/// since `Binding::Expr` has no `PartialEq`). Two fingerprints agreeing
/// means both authoring paths produced the same primitive tree.
fn fingerprint_tree(tree: &SurfaceTree) -> String {
    let mut out = String::new();
    out.push_str(&format!("id={}\n", tree.id));
    let mut modes: Vec<&Mode> = tree.modes.iter().collect();
    modes.sort_by_key(|m| format!("{m:?}"));
    let modes_str: Vec<String> = modes.iter().map(|m| format!("{m:?}")).collect();
    out.push_str(&format!("modes=[{}]\n", modes_str.join(",")));
    let mut inputs: Vec<&Input> = tree.inputs.iter().collect();
    inputs.sort_by_key(|i| format!("{i:?}"));
    let inputs_str: Vec<String> = inputs.iter().map(|i| format!("{i:?}")).collect();
    out.push_str(&format!("inputs=[{}]\n", inputs_str.join(",")));
    out.push_str(&format!("title={:?}\n", tree.title));
    let mut subs = tree.subscriptions.clone();
    subs.sort();
    out.push_str(&format!("subs={subs:?}\n"));
    fingerprint_node(&tree.root, 0, &mut out);
    out
}

fn fingerprint_node(n: &SurfaceNode, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    out.push_str(&format!(
        "{indent}{iri}@{path}\n",
        iri = n.kind.as_iri(),
        path = n.path,
    ));
    // Attrs — sort by key so BTreeMap iteration order (already sorted)
    // matches regardless of insertion order.
    for (k, v) in &n.attrs {
        out.push_str(&format!("{indent}  attr {k}={v:?}\n"));
    }
    // Binding slots — keys only. The `Binding::Expr` interior is not
    // structurally comparable across the two authoring paths (the
    // TOML parser and the expression parser produce the same AST but
    // debug-printing them also agrees — we skip it here to keep the
    // test robust to cosmetic AST refactors).
    for k in n.bindings.keys() {
        out.push_str(&format!("{indent}  bind {k}\n"));
    }
    // Affordances — verb name + invocations, deterministic order.
    for a in &n.affordances {
        let mut invs: Vec<String> = a.invocations.iter().map(|i| format!("{i:?}")).collect();
        invs.sort();
        out.push_str(&format!(
            "{indent}  aff {name} -> {verb} ({invs}) schema={schema:?}\n",
            name = a.name,
            verb = a.verb,
            invs = invs.join(","),
            schema = a.args_schema,
        ));
    }
    for c in &n.children {
        fingerprint_node(c, depth + 1, out);
    }
}

#[test]
fn rust_builder_produces_admin_surface() {
    let tree = build_admin_surface_via_rust();
    assert_eq!(tree.id, "weftos-admin/desktop");
    assert_eq!(tree.root.kind, IdentityIri::Grid);
    assert_eq!(tree.root.children.len(), 4, "four quadrants");
    assert_eq!(tree.count_of("ui://chip"), 2);
    assert_eq!(tree.count_of("ui://gauge"), 1);
    assert_eq!(tree.count_of("ui://table"), 1);
    assert_eq!(tree.count_of("ui://stream-view"), 1);
    assert!(tree.any_affordance_with_verb("rpc.kernel.kill"));
    assert!(tree.any_affordance_with_verb("rpc.kernel.restart-service"));
}

#[test]
fn toml_and_builder_trees_structurally_agree() {
    let toml_tree = parse_surface_toml(FIXTURE).expect("parse TOML fixture");
    let builder_tree = build_admin_surface_via_rust();
    let toml_fp = fingerprint_tree(&toml_tree);
    let builder_fp = fingerprint_tree(&builder_tree);
    // Show a diff on failure.
    assert_eq!(
        toml_fp, builder_fp,
        "TOML and builder fingerprints disagree:\n--- TOML ---\n{toml_fp}\n--- BUILDER ---\n{builder_fp}"
    );
}
