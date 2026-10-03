//! Errors: `SvcError` for local setup and state, `ApiError` for HTTP answers.

/// A local failure (init, load, persist, bind).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SvcError {
    /// `init` found a key already there; it is never overwritten.
    #[error("a grant key already exists; it is never overwritten")]
    KeyExists,
    /// No grant key (run `weft-licence init`).
    #[error("no grant key; run `weft-licence init` over USB")]
    NoKey,
    /// The key file is not a 64-hex seed.
    #[error("grant key file is malformed")]
    BadKey,
    /// The key or its directory is readable by another user.
    #[error("unsafe key permissions: {0}")]
    KeyPerms(String),
    /// Filesystem error.
    #[error("io: {0}")]
    Io(String),
    /// Persisting state failed; nothing was released.
    #[error("persist failed: {0}")]
    Persist(String),
    /// Invalid configuration.
    #[error("config: {0}")]
    Config(String),
    /// A binding record was refused.
    #[error("binding refused: {0}")]
    Bind(String),
    /// The Seed is bound to another mesh.
    #[error("seed_bound_elsewhere")]
    BoundElsewhere,
    /// A state file did not parse or re-verify; the service refuses to start.
    #[error("state file {0} is corrupt")]
    Corrupt(String),
}

/// An HTTP answer for a refused request: status, stable code, detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// HTTP status.
    pub status: u16,
    /// Stable machine-readable code (`cog_unlicensed`, `clock_not_set`, ...).
    pub code: &'static str,
    /// Human detail.
    pub detail: String,
}

impl ApiError {
    /// Build an error.
    pub fn new(status: u16, code: &'static str, detail: impl Into<String>) -> Self {
        Self { status, code, detail: detail.into() }
    }
}
