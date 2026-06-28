//! TOML → [`SurfaceTree`] converter (ADR-016 §2).
//!
//! Accepts the structure produced by the arrays-of-tables syntax in
//! the ADR's worked example (§10). The parser is permissive on
//! attribute names but strict on structure: nodes must carry a
//! `type` and an `id`; containers may carry `children`; leaves may
//! not. Expressions in `bindings` and `when` are handed to
//! [`crate::parse::expr`].
//!
//! This parser intentionally uses the stringly-typed
//! `toml::Value` variant rather than deriving `serde` structs — the
//! binding-expression values are strings at the document level and
//! get lifted into the expression AST at parse time.

use thiserror::Error;
use toml::Value as Toml;

use super::expr::{ParseError as ExprError, parse as parse_expr};
use crate::tree::{
    AffordanceDecl, AttrValue, Binding, IdentityIri, Input, InputExt, Invocation, Mode, ModeExt,
    SurfaceNode, SurfaceTree,
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
pub fn parse_all_surface_variants(src: &str) -> Result<Vec<SurfaceTree>, ParseError> {
    let root: Toml = src.parse()?;
    let surfaces = root
        .as_table()
        .and_then(|t| t.get("surfaces"))
        .ok_or(ParseError::NoSurfaces)?;

    let arr = match surfaces {
        Toml::Array(a) => a.clone(),
        Toml::Table(_) => vec![surfaces.clone()],
        _ => return Err(ParseError::NoSurfaces),
    };

    arr.into_iter()
        .enumerate()
        .map(|(i, s)| parse_variant(i, s))
        .collect()
}

fn parse_variant(index: usize, t: Toml) -> Result<SurfaceTree, ParseError> {
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

    let root = parse_node(root_tbl.clone())?;

    let mut tree = SurfaceTree::new(id, root);
    tree.modes = modes;
    tree.inputs = inputs;
    tree.title = title;
    tree.subscriptions = subscriptions;
    Ok(tree)
}

fn parse_node(t: Toml) -> Result<SurfaceNode, ParseError> {
    let tbl = t
        .as_table()
        .ok_or(ParseError::BadAffordance("node must be a table"))?;

    let iri = tbl
        .get("type")
        .and_then(Toml::as_str)
        .ok_or(ParseError::MissingField {
            index: 0,
            field: "type",
        })?;
    let kind = IdentityIri::parse(iri).ok_or_else(|| ParseError::UnknownIri(iri.to_string()))?;

    let path = tbl
        .get("id")
        .and_then(Toml::as_str)
        .ok_or(ParseError::MissingField {
            index: 0,
            field: "id",
        })?
        .to_string();

    let mut node = SurfaceNode::new(kind, path);

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
            node.children.push(parse_node(c.clone())?);
        }
    }

    Ok(node)
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
