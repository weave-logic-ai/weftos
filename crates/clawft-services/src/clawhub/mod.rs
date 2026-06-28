//! ClawHub skill registry module.
//!
//! Implements the ClawHub skill registry with:
//! - REST API contract stubs (Contract #20)
//! - Vector search integration (with keyword fallback)
//! - Star/comment system
//! - Versioning
//! - Skill signing enforcement

pub mod community;
pub mod registry;
pub mod search;

pub use registry::{
    ApiResponse, ClawHubClient, ClawHubConfig, ClawHubError, PublishRequest, SkillEntry,
    SkillInstallResult,
};
pub use search::SkillSearchResult;
