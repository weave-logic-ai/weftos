//! `clawft-app` — WeftOS app manifest parser + local app registry.
//!
//! Implements the subset of [ADR-015][adr] required for milestone M1.5:
//! TOML manifest schema, structural validation rules 1–9, a JSON-backed
//! [`registry::AppRegistry`], and the lifecycle types the desktop
//! compositor will consume when launching apps.
//!
//! Out of scope (lands in sibling crates / later milestones):
//!
//! * Surface description parsing — [`manifest::SurfaceRef`] is just a
//!   string; ADR-016 / the `clawft-surface` crate owns the tree IR.
//! * Ontology adapter introspection — permission ↔ adapter consistency
//!   (ADR-015 rule 6) is TODO'd until the ADR-017 / `clawft-adapter`
//!   crate lands.
//! * Real governance — [`lifecycle::governance::NoopGate`] and
//!   [`lifecycle::governance::StrictGate`] are placeholders; ADR-012 /
//!   M1.6+ owns the real gate.
//!
//! [adr]: https://github.com/weave-logic-ai/weftos/blob/development-0.7.0/.planning/symposiums/compositional-ui/adrs/adr-015-app-manifest.md

pub mod lifecycle;
pub mod manifest;
pub mod registry;
pub mod validation;

pub use lifecycle::{
    AppLaunchRequest, AppLaunchResult, LaunchError, SessionConfig,
    governance::{Gate, NoopGate, StrictGate},
};
pub use manifest::{AppManifest, EntryPoint, Input, Mode, Permission, SurfaceRef};
pub use registry::{AppRegistry, InstalledApp, RegistryError};
pub use validation::{ValidationError, validate};
