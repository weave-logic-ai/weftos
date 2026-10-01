//! Error type for the project model.

use std::path::PathBuf;

/// Failure reading, writing or validating project identity files.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    /// Id is not a 26-char Crockford ULID (or contains path separators).
    #[error("invalid project id {0:?}: expected a 26-character ULID")]
    InvalidId(String),
    /// Filesystem error, with the path involved.
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// TOML (or workspaces.json) could not be parsed.
    #[error("{path}: {message}")]
    Parse { path: PathBuf, message: String },
    /// A value could not be serialized.
    #[error("serialize {path}: {message}")]
    Serialize { path: PathBuf, message: String },
    /// File declares a schema newer than this build understands.
    #[error("{path}: unsupported schema version {found} (this build supports up to {supported})")]
    UnsupportedSchema {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    /// A manifest file's name does not match the id inside it.
    #[error("{path}: file name does not match manifest id {id}")]
    IdMismatch { path: PathBuf, id: String },
    /// Root directory could not be canonicalised or is not a directory.
    #[error("{0}: project root is not an existing directory")]
    BadRoot(PathBuf),
    /// `reinit_fork` was asked to re-identify the registered home of an id.
    #[error(
        "{root} is the registered home of project {id}; fork the copy instead \
         (or pass force to archive this manifest and mint a new identity here)"
    )]
    RegisteredHome { id: String, root: PathBuf },
    /// The id is already registered for a different live root (a clone or
    /// copy of a project tree, or a stale copy left after a move).
    #[error(
        "project {id} at {root} is already registered for {existing}; \
         if {root} is a copy, run `weft project init --fork` there to give it its own identity"
    )]
    RootConflict {
        id: String,
        root: PathBuf,
        existing: PathBuf,
    },
}

impl ProjectError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
