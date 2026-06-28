//! Parse a surface description from TOML.
//!
//! Entry point: [`parse_surface_toml`]. Produces a
//! [`crate::tree::SurfaceTree`] matching the root surface found in
//! the TOML document. For multi-variant documents (ADR-016 §9) the
//! caller chooses a variant via [`parse_all_surface_variants`] which
//! returns every `[[surfaces]]` entry in declaration order.

pub mod expr;
pub mod toml;

pub use self::toml::{ParseError, parse_all_surface_variants, parse_surface_toml};
