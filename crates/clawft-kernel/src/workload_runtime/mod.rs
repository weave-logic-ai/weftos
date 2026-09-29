//! Workload runtime adapters (mesh-placement-09; ADR-099 section 5,
//! COG-001 sections 3-5).
//!
//! - [`types`]: the [`WorkloadRuntime`] trait (id, provides, admit, load,
//!   start, stop, unload, status, control_mode, console) and its values.
//! - [`native`]: unprivileged native process under `[console]` /
//!   `[resources]` limits and rlimits ([`supervise`]), emitting
//!   [`RunEvidence`].
//! - [`container`]: Apple `container`, Docker / OrbStack and Podman,
//!   running a one-binary image built locally from the verified artifact
//!   over an operator-pinned base ([`container_cmd`]).
//! - [`seed`] / [`seed_ops`]: the `remote.api` adapter for a Cognitum Seed
//!   (its own HTTP API; pinned store cogs only; pairing; governed,
//!   backed-up firmware upgrade), over [`seed_http`].
//! - [`host_contract`]: `COG_CSI_BIND`, `COG_SENSOR_URL`,
//!   `COGNITUM_COG_TOKEN`, `COGNITUM_COG_DATA_DIR`.
//! - [`host`]: [`WorkloadHost`], which gates each transition and chains it
//!   as a `workload.*` event.
//!
//! WASM adapters, when built, use `clawft-wasm-host` (ADR-099 decision 4);
//! the kernel `wasm_runner` is not used for cogs.

pub mod cog_spec;
pub mod container;
pub mod container_cmd;
pub mod evidence;
pub mod host;
pub mod host_contract;
pub mod native;
pub mod seed;
pub mod seed_http;
pub mod seed_ops;
pub mod seed_types;
pub mod supervise;
pub mod types;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests_container;
#[cfg(test)]
mod tests_host;
#[cfg(test)]
mod tests_live;
#[cfg(test)]
mod tests_native;
#[cfg(test)]
mod tests_seed;

pub use cog_spec::{CogSpec, ConsoleLimits, Resources};
pub use container::{ContainerRuntime, ContainerRuntimeConfig};
pub use container_cmd::{CommandRunner, Engine, SystemRunner};
pub use evidence::RunEvidence;
pub use host::{RUNTIME_CHAIN_SOURCE, WorkloadHost};
pub use host_contract::HostContract;
pub use native::{NativeConfig, NativeRuntime};
pub use seed::{SeedApiRuntime, SeedConfig, SeedPin};
pub use seed_http::{ConfigServiceCredentials, HttpSeedTransport, SeedCredentials, SeedTransport};
pub use seed_ops::{SeedBackup, UpgradeOutcome};
pub use types::{
    Admission, ControlMode, InstanceHandle, InstanceState, InstanceStatus, RunMode, RuntimeError,
    VerifiedWorkload, WorkloadConfig, WorkloadRuntime, WorkloadSource,
};
