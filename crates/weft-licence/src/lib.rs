//! `weft-licence` (ADR-106 phase 2): the Seed-side licence proxy.
//!
//! It runs as its own binary, system user and unit beside `weft-cog-host`,
//! never inside it. It checks the declared licence, fetches the cog, verifies
//! the registry sha256, computes BLAKE3, and signs a `CheckoutGrant` (the wire
//! format is `weft-licence-wire`, shared with the kernel) for the one mesh the
//! Seed is bound to.
//!
//! Pending Cognitum (phase 4, questions C1, C2, C4): the licence check
//! ([`providers::LicenceProvider`]), the cog fetch ([`providers::CogFetcher`])
//! and device-key signing ([`providers::DeviceSigner`], a STUB) sit behind
//! traits.

pub mod artifact;
pub mod bind;
pub mod cache;
pub mod checkout;
pub mod config;
pub mod error;
pub mod fsio;
pub mod http;
pub mod keys;
pub mod limits;
pub mod providers;
#[cfg(feature = "registry")]
pub mod registry;
pub mod request;
pub mod service;
pub mod state;

pub use config::{CLOCK_FLOOR, Config, Limits};
pub use error::{ApiError, SvcError};
pub use service::{Body, Clock, Response, Service, system_clock};
