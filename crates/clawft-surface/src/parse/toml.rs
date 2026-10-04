//! TOML → [`SurfaceTree`] converter (ADR-016 §2).
//!
//! Accepts the structure produced by the arrays-of-tables syntax in
//! the ADR's worked example (§10). The parser is permissive on
//! attribute names but strict on structure: nodes must carry a
//! `type` and an `id`; containers may carry `children`; leaves may
//! not. Expressions in `bindings` and `when` are handed to
//! [`crate::parse::expr`].
//!
//! # Compositions (ADR-016 §7 / WEFT-425)
//!
//! Document-level `[compositions.Name]` tables declare author-local
//! macros. Each def carries `expands_to` (a canon `ui://…` IRI) and
//! optional default `attrs`. When a node sets `type = "Name"`, the
//! parser expands it at load time into the target primitive:
//!
//! - composition default attrs are applied first; instance attrs win
//! - children / bindings / when / affordances pass through
//! - if the merged attrs contain `submit_verb` (Form sugar), the last
//!   child is wrapped in a `ui://pressable` with that verb
//!
//! Names that collide with the 21-IRI short names or full `ui://…`
//! forms are rejected at load. The resulting [`SurfaceTree`] only
//! ever contains canon IRIs — the composer runtime never sees a
//! composition name.

use std::collections::BTreeMap;

use thiserror::Error;
use toml::Value as Toml;

use super::expr::{ParseError as ExprError, parse as parse_expr};
use crate::tree::{
    AffordanceDecl, AttrValue, Binding, CompositionDef, IdentityIri, Input, InputExt, Invocation,
    Mode, ModeExt, SurfaceNode, SurfaceTree,
};

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid TOML: {0}")]
    Toml(#[from] ::toml::de::Error),
    #[error("document has no `[[surfaces]]` array")]
    NoSurfaces,
    #[error("surface variant #{index} is missing required field `{field}`")]
    MissingField { index: usize, field: &'static str },
    #[error("unknown primitive IRI `{0}`")]
    UnknownIri(String),
    #[error("`{iri}` is a leaf primitive but has `children`")]
    LeafWithChildren { iri: String },
    #[error("binding expression error in `{key}`: {source}")]
    BadExpr {
        key: String,
        #[source]
        source: ExprError,
    },
    #[error("unknown mode `{0}`")]
    UnknownMode(String),
    #[error("unknown input `{0}`")]
    UnknownInput(String),
    #[error("unknown invocation `{0}`")]
    UnknownInvocation(String),
    #[error("malformed affordance: {0}")]
    BadAffordance(&'static str),
    #[error("composition `{name}` shadows canon IRI `{iri}`")]
    CompositionShadowsCanon { name: String, iri: String },
    #[error("composition `{name}` is missing required field `{field}`")]
    CompositionMissingField {
        name: String,
        field: &'static str,
    },
    #[error("composition `{name}` expands_to `{iri}` is not a canon primitive")]
    CompositionBadTarget { name: String, iri: String },
    #[error("unknown composition `{0}`")]
    UnknownComposition(String),
}

/// Parse the first surface variant from a document. Convenience for
/// single-variant tests and for M1.5 admin-panel rendering.
pub fn parse_surface_toml(src: &str) -> Result<SurfaceTree, ParseError> {
    let mut vs = parse_all_surface_variants(src)?;
    if vs.is_empty() {
        return Err(ParseError::NoSurfaces);
    }
    Ok(vs.remove(0))
}

/// Parse every `[[surfaces]]` entry from a document.
///
/// Document-level `[compositions.*]` tables are parsed once and
/// applied to every surface variant during node expansion.
pub fn parse_all_surface_variants(src: &str) -> Result<Vec<SurfaceTree>, ParseError> {
    let root: Toml = src.parse()?;
    let table = root.as_table().ok_or(ParseError::NoSurfaces)?;

    let compositions = parse_compositions(table.get("compositions"))?;

    let surfaces = table.get("surfaces").ok_or(ParseError::NoSurfaces)?;

    let arr = match surfaces {
        Toml::Array(a) => a.clone(),
        Toml::Table(_) => vec![surfaces.clone()],
        _ => return Err(ParseError::NoSurfaces),
    };

    arr.into_iter()
        .enumerate()
        .map(|(i, s)| parse_variant(i, s, &compositions))
        .collect()
}

/// Parse the top-level `[compositions.*]` table (ADR-016 §7).
///
/// Returns an empty map when the key is absent. Validates that each
/// composition name does not shadow a canon IRI short name or full
/// `ui://…` form, and that `expands_to` is a known canon primitive.
pub fn parse_compositions(
    value: Option<&Toml>,
) -> Result<BTreeMap<String, CompositionDef>, ParseError> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let tbl = value.as_table().ok_or(ParseError::BadAffordance(
        "compositions must be a table of named defs",
    ))?;

    let mut out = BTreeMap::new();
    for (name, def_val) in tbl {
        let def = parse_composition_def(name, def_val)?;
        out.insert(name.clone(), def);
    }
    Ok(out)
}

fn parse_composition_def(name: &str, value: &Toml) -> Result<CompositionDef, ParseError> {
    if let Some(iri) = canon_collision(name) {
        return Err(ParseError::CompositionShadowsCanon {
            name: name.to_string(),
            iri,
        });
    }

    let tbl = value.as_table().ok_or(ParseError::CompositionMissingField {
        name: name.to_string(),
        field: "table body",
    })?;

    let expands_src = tbl
        .get("expands_to")
        .and_then(Toml::as_str)
        .ok_or(ParseError::CompositionMissingField {
            name: name.to_string(),
            field: "expands_to",
        })?;

    let expands_to = IdentityIri::parse(expands_src).ok_or_else(|| {
        ParseError::CompositionBadTarget {
            name: name.to_string(),
            iri: expands_src.to_string(),
        }
    })?;

    let mut attrs = BTreeMap::new();
    if let Some(Toml::Table(a)) = tbl.get("attrs") {
        for (k, v) in a {
            if let Some(attr) = toml_to_attr(v) {
                attrs.insert(k.clone(), attr);
            }
        }
    }

    Ok(CompositionDef {
        name: name.to_string(),
        expands_to,
        attrs,
    })
}

/// True when `name` collides with a canon primitive short name
/// (`stack`) or full IRI (`ui://stack`). Returns the colliding IRI
/// string for the error message.
fn canon_collision(name: &str) -> Option<String> {
    if let Some(iri) = IdentityIri::parse(name) {
        return Some(iri.as_iri().to_string());
    }
    let prefixed = format!("ui://{name}");
    if let Some(iri) = IdentityIri::parse(&prefixed) {
        return Some(iri.as_iri().to_string());
    }
    None
}

fn parse_variant(
    index: usize,
    t: Toml,
    compositions: &BTreeMap<String, CompositionDef>,
) -> Result<SurfaceTree, ParseError> {
    let tbl = t.as_table().ok_or(ParseError::MissingField {
        index,
        field: "surface table",
    })?;

    let id = tbl
        .get("id")
        .and_then(Toml::as_str)
        .ok_or(ParseError::MissingField { index, field: "id" })?
        .to_string();

    let modes = tbl
        .get("modes")
        .and_then(Toml::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Toml::as_str)
                .map(|s| Mode::parse(s).ok_or_else(|| ParseError::UnknownMode(s.into())))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();

    let inputs = tbl
        .get("inputs")
        .and_then(Toml::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Toml::as_str)
                .map(|s| Input::parse(s).ok_or_else(|| ParseError::UnknownInput(s.into())))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();

    let title = tbl.get("title").and_then(Toml::as_str).map(str::to_string);

    let subscriptions = tbl
        .get("subscriptions")
        .and_then(Toml::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Toml::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let root_tbl = tbl.get("root").ok_or(ParseError::MissingField {
        index,
        field: "root",
    })?;

    let root = parse_node(root_tbl.clone(), compositions)?;

    let mut tree = SurfaceTree::new(id, root);
    tree.modes = modes;
    tree.inputs = inputs;
    tree.title = title;
    tree.subscriptions = subscriptions;
    tree.compositions = compositions.clone();
    Ok(tree)
}

fn parse_node(
    t: Toml,
    compositions: &BTreeMap<String, CompositionDef>,
) -> Result<SurfaceNode, ParseError> {
    let tbl = t
        .as_table()
        .ok_or(ParseError::BadAffordance("node must be a table"))?;

    let type_src = tbl
        .get("type")
        .and_then(Toml::as_str)
        .ok_or(ParseError::MissingField {
            index: 0,
            field: "type",
        })?;

    let path = tbl
        .get("id")
        .and_then(Toml::as_str)
        .ok_or(ParseError::MissingField {
            index: 0,
            field: "id",
        })?
        .to_string();

    // Resolve type: canon IRI first, then composition name (WEFT-425).
    let (kind, composition_defaults) = resolve_type(type_src, compositions)?;

    let mut node = SurfaceNode::new(kind, path);

    // Composition default attrs first; instance attrs override.
    for (k, v) in composition_defaults {
        node.attrs.insert(k, v);
    }

    if let Some(Toml::Table(attrs)) = tbl.get("attrs") {
        for (k, v) in attrs {
            if let Some(a) = toml_to_attr(v) {
                node.attrs.insert(k.clone(), a);
            }
        }
    }

    if let Some(Toml::Table(bs)) = tbl.get("bindings") {
        for (k, v) in bs {
            let expr_src = match v {
                Toml::String(s) => s.clone(),
                other => other.to_string(),
            };
            let b = parse_binding(&expr_src, k)?;
            node.bindings.insert(k.clone(), b);
        }
    }

    if let Some(w) = tbl.get("when") {
        let src = match w {
            Toml::String(s) => s.clone(),
            other => other.to_string(),
        };
        node.when = Some(parse_binding(&src, "when")?);
    }

    if let Some(Toml::Array(aff)) = tbl.get("affordances") {
        for a in aff {
            node.affordances.push(parse_affordance(a.clone())?);
        }
    }

    if let Some(Toml::Array(cs)) = tbl.get("children") {
        if !kind.is_container() {
            return Err(ParseError::LeafWithChildren {
                iri: kind.as_iri().to_string(),
            });
        }
        for c in cs {
            node.children.push(parse_node(c.clone(), compositions)?);
        }
    }

    // Form sugar (ADR-016 §7): when `submit_verb` is present on the
    // expanded node, wrap the last child in a ui://pressable that
    // carries that verb. The attr is consumed so layout code never
    // sees it as a presentation attribute.
    apply_submit_verb_sugar(&mut node);

    Ok(node)
}

/// Resolve a `type = "…"` string to a canon kind, optionally with
/// composition default attrs. Bare composition names (`Card`) expand
/// here so the composer never observes a non-canon type.
fn resolve_type(
    type_src: &str,
    compositions: &BTreeMap<String, CompositionDef>,
) -> Result<(IdentityIri, BTreeMap<String, AttrValue>), ParseError> {
    if let Some(kind) = IdentityIri::parse(type_src) {
        return Ok((kind, BTreeMap::new()));
    }
    if let Some(def) = compositions.get(type_src) {
        return Ok((def.expands_to, def.attrs.clone()));
    }
    // Helpful error: unknown composition vs unknown IRI.
    if !type_src.starts_with("ui://") {
        return Err(ParseError::UnknownComposition(type_src.to_string()));
    }
    Err(ParseError::UnknownIri(type_src.to_string()))
}

/// ADR-016 §7 Form magic: `attrs.submit_verb = "…"` wraps the last
/// child in a pressable with that verb. No-op when the attr is
/// absent or there are no children.
fn apply_submit_verb_sugar(node: &mut SurfaceNode) {
    let verb = match node.attrs.remove("submit_verb") {
        Some(AttrValue::Str(s)) if !s.is_empty() => s,
        Some(_) | None => return,
    };
    let Some(last) = node.children.pop() else {
        return;
    };
    let submit_path = format!("{}/submit", node.path);
    let mut pressable = SurfaceNode::new(IdentityIri::Pressable, submit_path);
    pressable.affordances.push(AffordanceDecl {
        name: "submit".into(),
        verb,
        invocations: vec![Invocation::Pointer, Invocation::Voice],
        args_schema: None,
    });
    pressable.children.push(last);
    node.children.push(pressable);
}

fn parse_binding(src: &str, key: &str) -> Result<Binding, ParseError> {
    let expr = parse_expr(src).map_err(|source| ParseError::BadExpr {
        key: key.to_string(),
        source,
    })?;
    Ok(Binding::Expr(expr))
}

fn parse_affordance(t: Toml) -> Result<AffordanceDecl, ParseError> {
    let tbl = t
        .as_table()
        .ok_or(ParseError::BadAffordance("affordance must be a table"))?;
    let name = tbl
        .get("name")
        .and_then(Toml::as_str)
        .ok_or(ParseError::BadAffordance("missing name"))?
        .to_string();
    let verb = tbl
        .get("verb")
        .and_then(Toml::as_str)
        .ok_or(ParseError::BadAffordance("missing verb"))?
        .to_string();
    let invocations = tbl
        .get("invocations")
        .and_then(Toml::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Toml::as_str)
                .map(|s| {
                    Invocation::parse(s).ok_or_else(|| ParseError::UnknownInvocation(s.into()))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let args_schema = tbl
        .get("args_schema")
        .and_then(Toml::as_str)
        .map(str::to_string);
    Ok(AffordanceDecl {
        name,
        verb,
        invocations,
        args_schema,
    })
}

fn toml_to_attr(v: &Toml) -> Option<AttrValue> {
    Some(match v {
        Toml::Boolean(b) => AttrValue::Bool(*b),
        Toml::Integer(i) => AttrValue::Int(*i),
        Toml::Float(f) => AttrValue::Number(*f),
        Toml::String(s) => AttrValue::Str(s.clone()),
        Toml::Array(arr) => {
            let items: Vec<AttrValue> = arr.iter().filter_map(toml_to_attr).collect();
            AttrValue::Array(items)
        }
        // Nested tables inside attrs aren't supported in M1.5 — we
        // drop them silently rather than fail the parse, because
        // TOML tables-of-tables are used elsewhere for legitimate
        // structural nesting (affordances, bindings).
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::IdentityIri;

    const MULTI_COMP: &str = r#"
[compositions.Card]
expands_to = "ui://stack"
attrs      = { axis = "vertical", padding = 12, frame = "rounded" }

[compositions.Form]
expands_to = "ui://stack"
attrs      = { axis = "vertical", gap = 8 }

[[surfaces]]
id     = "demo/multi-comp"
modes  = ["desktop"]
inputs = ["pointer"]
title  = "Multi-composition"

[surfaces.root]
type  = "ui://stack"
id    = "/root"
attrs = { axis = "vertical" }

[[surfaces.root.children]]
type  = "Card"
id    = "/root/card"
attrs = { padding = 8 }

  [[surfaces.root.children.children]]
  type  = "ui://chip"
  id    = "/root/card/title"
  attrs = { label = "Hello" }

[[surfaces.root.children]]
type  = "Form"
id    = "/root/form"
attrs = { submit_verb = "rpc.form.submit" }

  [[surfaces.root.children.children]]
  type  = "ui://field"
  id    = "/root/form/name"
  attrs = { label = "Name" }

  [[surfaces.root.children.children]]
  type  = "ui://chip"
  id    = "/root/form/go"
  attrs = { label = "Send" }
"#;

    #[test]
    fn parses_and_expands_multi_composition_surface() {
        let tree = parse_surface_toml(MULTI_COMP).expect("parse");
        assert_eq!(tree.id, "demo/multi-comp");
        assert_eq!(tree.compositions.len(), 2);
        assert!(tree.compositions.contains_key("Card"));
        assert!(tree.compositions.contains_key("Form"));

        // Root is still a stack; two expanded children.
        assert_eq!(tree.root.kind, IdentityIri::Stack);
        assert_eq!(tree.root.children.len(), 2);

        // Card → stack with merged attrs (instance padding wins).
        let card = &tree.root.children[0];
        assert_eq!(card.kind, IdentityIri::Stack);
        assert_eq!(card.path, "/root/card");
        assert_eq!(card.attrs.get("axis"), Some(&AttrValue::Str("vertical".into())));
        assert_eq!(card.attrs.get("padding"), Some(&AttrValue::Int(8)));
        assert_eq!(card.attrs.get("frame"), Some(&AttrValue::Str("rounded".into())));
        assert_eq!(card.children.len(), 1);
        assert_eq!(card.children[0].kind, IdentityIri::Chip);

        // Form → stack; last child wrapped in pressable with submit verb.
        let form = &tree.root.children[1];
        assert_eq!(form.kind, IdentityIri::Stack);
        assert_eq!(form.path, "/root/form");
        assert_eq!(form.attrs.get("gap"), Some(&AttrValue::Int(8)));
        assert!(
            !form.attrs.contains_key("submit_verb"),
            "submit_verb consumed"
        );
        assert_eq!(form.children.len(), 2);
        assert_eq!(form.children[0].kind, IdentityIri::Field);
        let submit = &form.children[1];
        assert_eq!(submit.kind, IdentityIri::Pressable);
        assert_eq!(submit.path, "/root/form/submit");
        assert_eq!(submit.affordances.len(), 1);
        assert_eq!(submit.affordances[0].verb, "rpc.form.submit");
        assert_eq!(submit.affordances[0].name, "submit");
        assert_eq!(submit.children.len(), 1);
        assert_eq!(submit.children[0].kind, IdentityIri::Chip);
        assert_eq!(submit.children[0].path, "/root/form/go");

        // Wire never sees composition names as node kinds.
        assert_eq!(tree.count_of("ui://stack"), 3); // root + card + form
        assert_eq!(tree.count_of("ui://chip"), 2);
        assert_eq!(tree.count_of("ui://field"), 1);
        assert_eq!(tree.count_of("ui://pressable"), 1);
        assert!(tree.any_affordance_with_verb("rpc.form.submit"));
    }

    #[test]
    fn rejects_composition_name_shadowing_canon() {
        let src = r#"
[compositions.stack]
expands_to = "ui://grid"

[[surfaces]]
id = "x"
[surfaces.root]
type = "ui://stack"
id = "/root"
"#;
        let err = parse_surface_toml(src).expect_err("must reject shadow");
        match err {
            ParseError::CompositionShadowsCanon { name, iri } => {
                assert_eq!(name, "stack");
                assert_eq!(iri, "ui://stack");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rejects_unknown_composition_reference() {
        let src = r#"
[[surfaces]]
id = "x"
[surfaces.root]
type = "NotAThing"
id = "/root"
"#;
        let err = parse_surface_toml(src).expect_err("must reject unknown");
        match err {
            ParseError::UnknownComposition(name) => assert_eq!(name, "NotAThing"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rejects_composition_with_non_canon_target() {
        let src = r#"
[compositions.Card]
expands_to = "ui://not-a-real-primitive"

[[surfaces]]
id = "x"
[surfaces.root]
type = "ui://stack"
id = "/root"
"#;
        let err = parse_surface_toml(src).expect_err("must reject bad target");
        match err {
            ParseError::CompositionBadTarget { name, iri } => {
                assert_eq!(name, "Card");
                assert_eq!(iri, "ui://not-a-real-primitive");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn admin_fixture_still_parses_without_compositions() {
        let src = include_str!("../../fixtures/weftos-admin-desktop.toml");
        let tree = parse_surface_toml(src).expect("admin fixture");
        assert!(tree.compositions.is_empty());
        assert_eq!(tree.id, "weftos-admin/desktop");
    }
}
