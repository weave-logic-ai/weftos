//! `clawft-surface` — surface-description IR, TOML + Rust builder
//! parsers, binding expression evaluator, and composer runtime that
//! drives the canon primitives in `clawft-gui-egui`.
//!
//! This crate is the **M1.5 subset** of ADR-016 — sufficient for the
//! WeftOS Admin reference panel (session-10 §6.1). Out of scope for
//! M1.5: ternary, nested lambdas, user-defined compositions, real
//! governance intersection, variant-id stamping, hot-path
//! memoisation. Sibling milestones M1.6+ fill these in.
//!
//! # M1.5 scope reductions vs ADR-016 §5
//!
//! The binding expression language implemented here is a strict
//! subset of the grammar in ADR-016 §5. Concretely, the following
//! are *not* implemented and are expected to land in M1.6+:
//!
//! - `sort(list, key)` — no ordering combinator. Only `count`,
//!   `filter`, `len`, `first`, `last` are wired.
//! - `.first`/`.last` as field-access shorthand — both exist only as
//!   function calls (`first($xs)`, `last($xs)`); the dotted form is
//!   parsed as an ordinary field access and returns `Null`.
//! - Scientific-notation (`1e5`) and hexadecimal (`0xff`) number
//!   literals — only decimal integers and finite-decimal floats.
//! - Ternary `?:` operator — explicitly rejected with
//!   `ParseError::TernaryNotSupported`.
//! - Nested lambdas — a lambda body that itself contains `->` is
//!   rejected with `ParseError::NestedLambda`. Sibling lambdas
//!   (e.g. the `s -> …` inside a `count(…, s -> …)` that sits
//!   beside another `t -> …` at the same depth) are permitted.
//! - User-defined compositions (`[compositions.*]`) — the TOML
//!   parser does not read this table.
//!
//! See `.planning/symposiums/compositional-ui/adrs/adr-016-surface-description.md`
//! §5 for the full authoritative grammar.
//!
//! # Authoring paths
//!
//! ```no_run
//! use clawft_surface::builder::{grid, chip, stack, Surface};
//! use clawft_surface::tree::{Mode, Input};
//!
//! let tree = Surface::new("weftos-admin/desktop")
//!     .modes(&[Mode::Desktop, Mode::Ide])
//!     .inputs(&[Input::Pointer, Input::Hybrid])
//!     .title("WeftOS Admin")
//!     .subscribe("substrate/kernel/status")
//!     .root(
//!         grid("/root")
//!             .attr("columns", clawft_surface::tree::AttrValue::Int(2))
//!             .child(
//!                 stack("/root/overview")
//!                     .attr("axis", clawft_surface::tree::AttrValue::Str("horizontal".into()))
//!                     .child(
//!                         chip("/root/overview/status")
//!                             .bind("label", "$substrate/kernel/status.state")
//!                             .bind("tone", "$substrate/kernel/status.state"),
//!                     ),
//!             ),
//!     )
//!     .build();
//! ```
//!
//! See the integration tests (`tests/`) for the TOML authoring path
//! and a full admin-panel fixture.

pub mod builder;
pub mod eval;
pub mod parse;
pub mod tree;

/// Ontology snapshot re-export (ADR-017 §1; unified M1.5-D).
///
/// The composer runtime reads the substrate state tree through an
/// [`OntologySnapshot`]. The canonical definition lives in
/// `clawft-substrate` alongside the `Substrate` state tree; this
/// module simply re-exports it so downstream callers can keep using
/// `clawft_surface::substrate::OntologySnapshot`.
///
/// Prior to M1.5-D this crate held its own structurally-identical
/// copy to avoid a pre-merge dependency cycle. That cycle is now
/// resolved — the substrate crate owns the type, and the composer
/// builds on top of it.
///
/// **WEFT-428**: previously a 14-line `src/substrate.rs` shim; folded
/// inline here as a one-line re-export so the indirection has zero
/// surface area.
pub mod substrate {
    pub use clawft_substrate::OntologySnapshot;
}

pub use substrate::OntologySnapshot;
pub use tree::{
    AffordanceDecl, AttrValue, Binding, IdentityIri, Input, Invocation, Mode, SurfaceNode,
    SurfaceTree,
};

// NOTE on composer location (M1.5-D): the composer runtime
// (`compose(tree, snapshot, ui) -> Vec<CanonResponse>`) lives in
// `clawft-gui-egui::surface_host` because it talks directly to canon
// primitive types defined in that crate. Moving it here would
// introduce a cyclic crate dependency (surface → gui-egui → surface).
// A future milestone may extract the canon types into their own
// crate, at which point the composer can migrate back.
