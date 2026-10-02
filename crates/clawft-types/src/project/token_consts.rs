//! Lifetime and refresh of the project token the supervisor hands a child
//! (ADR-103 A6, Phase 2 package G). Kept apart from `spawn.rs`.

/// Lifetime of the project token the supervisor issues (and every refresh).
pub const PROJECT_TOKEN_TTL_SECS: u64 = 3_600;
/// A child refreshes its token when this little lifetime is left.
pub const PROJECT_TOKEN_REFRESH_SECS: u64 = 900;
/// RPC the child calls on the user daemon to renew its project token.
pub const TOKEN_REFRESH_METHOD: &str = "project.token.refresh";
