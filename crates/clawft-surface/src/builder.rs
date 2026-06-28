//! Fluent Rust builder for [`SurfaceTree`] (ADR-016 §3 sketch).
//!
//! Emits the same IR as the TOML parser. First-party Rust apps use
//! this path; the TOML parser is strictly lossless w.r.t. the
//! builder's output.

use crate::parse::expr::parse as parse_expr;
use crate::tree::{
    AffordanceDecl, AttrValue, Binding, IdentityIri, Input, Invocation, Mode, SurfaceNode,
    SurfaceTree,
};

/// Entry point for a new surface description.
pub struct Surface {
    tree: SurfaceTree,
}

impl Surface {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            tree: SurfaceTree::new(id, SurfaceNode::new(IdentityIri::Stack, "/root")),
        }
    }

    pub fn modes(mut self, modes: &[Mode]) -> Self {
        self.tree.modes = modes.to_vec();
        self
    }

    pub fn inputs(mut self, inputs: &[Input]) -> Self {
        self.tree.inputs = inputs.to_vec();
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.tree.title = Some(title.into());
        self
    }

    pub fn subscribe(mut self, path: impl Into<String>) -> Self {
        self.tree.subscriptions.push(path.into());
        self
    }

    pub fn root(mut self, node: NodeBuilder) -> Self {
        self.tree.root = node.node;
        self
    }

    pub fn build(self) -> SurfaceTree {
        self.tree
    }
}

/// Per-node fluent builder. Every convenience constructor
/// (`grid`, `stack`, `chip`, …) returns one of these.
pub struct NodeBuilder {
    node: SurfaceNode,
}

impl NodeBuilder {
    pub fn new(kind: IdentityIri, path: impl Into<String>) -> Self {
        Self {
            node: SurfaceNode::new(kind, path),
        }
    }

    pub fn attr(mut self, key: impl Into<String>, value: AttrValue) -> Self {
        self.node.attrs.insert(key.into(), value);
        self
    }

    /// Add a binding expression. The expression is parsed eagerly so
    /// malformed binding sources panic here, not at render time.
    pub fn bind(mut self, slot: impl Into<String>, expr_src: &str) -> Self {
        let expr = parse_expr(expr_src).expect("malformed binding expression");
        self.node.bindings.insert(slot.into(), Binding::Expr(expr));
        self
    }

    pub fn bind_literal(mut self, slot: impl Into<String>, value: AttrValue) -> Self {
        self.node
            .bindings
            .insert(slot.into(), Binding::Literal(value));
        self
    }

    pub fn when(mut self, expr_src: &str) -> Self {
        let expr = parse_expr(expr_src).expect("malformed `when` expression");
        self.node.when = Some(Binding::Expr(expr));
        self
    }

    pub fn child(mut self, child: NodeBuilder) -> Self {
        self.node.children.push(child.node);
        self
    }

    pub fn affordance(
        mut self,
        name: impl Into<String>,
        verb: impl Into<String>,
        invocations: &[Invocation],
    ) -> Self {
        self.node.affordances.push(AffordanceDecl {
            name: name.into(),
            verb: verb.into(),
            invocations: invocations.to_vec(),
            args_schema: None,
        });
        self
    }

    pub fn affordance_with_schema(
        mut self,
        name: impl Into<String>,
        verb: impl Into<String>,
        invocations: &[Invocation],
        args_schema: impl Into<String>,
    ) -> Self {
        self.node.affordances.push(AffordanceDecl {
            name: name.into(),
            verb: verb.into(),
            invocations: invocations.to_vec(),
            args_schema: Some(args_schema.into()),
        });
        self
    }

    pub fn into_node(self) -> SurfaceNode {
        self.node
    }
}

// ── Convenience constructors ────────────────────────────────────────

pub fn stack(path: impl Into<String>) -> NodeBuilder {
    NodeBuilder::new(IdentityIri::Stack, path)
}

pub fn strip(path: impl Into<String>) -> NodeBuilder {
    NodeBuilder::new(IdentityIri::Strip, path)
}

pub fn grid(path: impl Into<String>) -> NodeBuilder {
    NodeBuilder::new(IdentityIri::Grid, path)
}

pub fn chip(path: impl Into<String>) -> NodeBuilder {
    NodeBuilder::new(IdentityIri::Chip, path)
}

pub fn pressable(path: impl Into<String>) -> NodeBuilder {
    NodeBuilder::new(IdentityIri::Pressable, path)
}

pub fn gauge(path: impl Into<String>) -> NodeBuilder {
    NodeBuilder::new(IdentityIri::Gauge, path)
}

pub fn table(path: impl Into<String>) -> NodeBuilder {
    NodeBuilder::new(IdentityIri::Table, path)
}

pub fn stream_view(path: impl Into<String>) -> NodeBuilder {
    NodeBuilder::new(IdentityIri::StreamView, path)
}

pub fn modal(path: impl Into<String>) -> NodeBuilder {
    NodeBuilder::new(IdentityIri::Modal, path)
}
