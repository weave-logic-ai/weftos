//! Multi-source cog repositories for a WeftOS project (ADR-105).
//!
//! A project gets cogs from a list of sources: the WeftOS (WeaveLogic) signed
//! registry, Cognitum's app registry (installable only under a licence
//! entitlement), and the project's own private signed registry. This crate is
//! the library behind `weaver cog ...`:
//!
//! - [`config`]: `[[cog_source]]` / `[[cog_licence]]` files, project + user merge;
//! - [`licence`]: the entitlement check (presence, coverage, expiry);
//! - [`resolve`]: loading sources, `source:cog-id` resolution, ambiguity;
//! - [`install`]: verified fetch (signature or sha256) and install into a cog-host root;
//! - [`catalog`]: the cog catalog across sources with run mode and placement policy.

pub mod catalog;
pub mod config;
pub mod error;
pub mod fetch;
pub mod install;
pub mod licence;
pub mod resolve;

#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests;

pub use config::{CogLicence, CogSource, EffectiveSources, LicensedCogs, SourceKind, SourcesFile};
pub use error::{Result, SourceError};
pub use fetch::{FsReader, Reader};
pub use install::{fetch_verified, install_into_host, FetchCtx, Fetched, Provenance};
pub use resolve::{load_all, load_source, parse_ref, resolve, CogRef, Listing, LoadedSource, Resolved, SourceCog};

/// The default cog-host root (`$WEFTOS_COG_ROOT`, else `~/.weftos/cogs`).
pub fn default_host_root() -> std::path::PathBuf {
    weftos_cog_host::default_root()
}
