//! Errors with stable machine-readable codes.

/// Why a source operation was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// A sources file is malformed or violates a rule.
    #[error("cog sources config: {0}")]
    Config(String),
    /// A namespaced id names a source that is not configured.
    #[error("unknown cog source '{0}'")]
    UnknownSource(String),
    /// The named source exists but is disabled.
    #[error("cog source '{0}' is disabled")]
    SourceDisabled(String),
    /// No enabled source lists the cog.
    #[error("cog '{id}' not found{}", .source_name.as_ref().map(|s| format!(" in source '{s}'")).unwrap_or_default())]
    NotFound {
        /// Source searched, when the id was namespaced.
        source_name: Option<String>,
        /// Cog id.
        id: String,
    },
    /// Two or more enabled sources list the bare id at the same priority.
    #[error("cog '{id}' is ambiguous: listed by {} at equal priority; use {}", .sources.join(", "), .sources.iter().map(|s| format!("{s}:{id}")).collect::<Vec<_>>().join(" or "))]
    Ambiguous {
        /// Bare cog id.
        id: String,
        /// Tied sources, sorted.
        sources: Vec<String>,
    },
    /// A Cognitum cog was requested and the project holds no licence for it.
    #[error("cog_unlicensed: this project holds no licence entitlement for '{cog}' from source '{source_name}' (add one with `weaver cog licence add`)")]
    Unlicensed {
        /// Source name.
        source_name: String,
        /// Cog id.
        cog: String,
    },
    /// A licence covers the cog but has expired.
    #[error("licence_expired: the licence for '{cog}' from source '{source_name}' expired on {expires}")]
    LicenceExpired {
        /// Source name.
        source_name: String,
        /// Cog id.
        cog: String,
        /// Expiry as written.
        expires: String,
    },
    /// The cog has no artifact for the requested arch.
    #[error("cog '{id}' has no '{arch}' artifact (has: {})", .have.join(", "))]
    NoArtifact {
        /// Cog id.
        id: String,
        /// Requested arch.
        arch: String,
        /// Arches present.
        have: Vec<String>,
    },
    /// A signed-source artifact carries no signature.
    #[error("refusing {id}: artifact is unsigned and source '{source_name}' is signed-only")]
    Unsigned {
        /// Cog id.
        id: String,
        /// Source name.
        source_name: String,
    },
    /// Size, sha256 or signature verification failed.
    #[error("refusing {id} from '{source_name}': {reason}")]
    Verify {
        /// Cog id.
        id: String,
        /// Source name.
        source_name: String,
        /// What failed.
        reason: String,
    },
    /// The source cannot give a hash to check the binary against.
    #[error("refusing {id} from '{source_name}': the registry lists no usable sha256 to check the binary against")]
    NoPinnedHash {
        /// Cog id.
        id: String,
        /// Source name.
        source_name: String,
    },
    /// `--enable` on a bare id that resolved to a project-defined source.
    #[error("'{reference}' resolved to {namespaced} from this project's own source list; use the namespaced id ({namespaced}) or pass --confirm-project-source to start it")]
    NeedsNamespacedId {
        /// Reference as typed.
        reference: String,
        /// The namespaced form.
        namespaced: String,
    },
    /// A Cognitum binary location is not https.
    #[error("refusing {id} from '{source_name}': binary location {location} is not https:// (Cognitum binaries are trusted by sha256 alone)")]
    Insecure {
        /// Cog id.
        id: String,
        /// Source name.
        source_name: String,
        /// The offending location.
        location: String,
    },
    /// Reading a registry or binary failed.
    #[error("fetch {location}: {msg}")]
    Fetch {
        /// Path or URL.
        location: String,
        /// Error text.
        msg: String,
    },
    /// A registry or catalog input did not parse.
    #[error("parse {what}: {msg}")]
    Parse {
        /// What was parsed.
        what: String,
        /// Error text.
        msg: String,
    },
    /// Local I/O failure.
    #[error("io {path}: {msg}")]
    Io {
        /// Path involved.
        path: String,
        /// Error text.
        msg: String,
    },
}

impl SourceError {
    /// Stable code for scripts, JSON output and chain payloads.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Config(_) => "config_invalid",
            Self::UnknownSource(_) => "unknown_source",
            Self::SourceDisabled(_) => "source_disabled",
            Self::NotFound { .. } => "cog_not_found",
            Self::Ambiguous { .. } => "ambiguous",
            Self::Unlicensed { .. } => "cog_unlicensed",
            Self::LicenceExpired { .. } => "licence_expired",
            Self::NoArtifact { .. } => "no_artifact",
            Self::Unsigned { .. } => "unsigned",
            Self::Verify { .. } => "verify_failed",
            Self::NoPinnedHash { .. } => "no_pinned_hash",
            Self::NeedsNamespacedId { .. } => "needs_namespaced_id",
            Self::Insecure { .. } => "insecure_transport",
            Self::Fetch { .. } => "fetch_failed",
            Self::Parse { .. } => "parse_failed",
            Self::Io { .. } => "io_error",
        }
    }
}

/// Result alias.
pub type Result<T> = std::result::Result<T, SourceError>;
