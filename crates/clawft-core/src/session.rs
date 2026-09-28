//! Session management for conversation persistence.
//!
//! Provides [`SessionManager`] which caches active sessions in memory and
//! persists them to disk as JSONL files using the platform filesystem
//! abstraction. Each JSONL file has a metadata header line followed by
//! one line per conversation turn.
//!
//! Ported from Python `nanobot/session/manager.py`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::runtime::Mutex;
use chrono::Utc;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, percent_encode};
use tracing::{debug, warn};

use clawft_platform::Platform;
use clawft_types::error::ClawftError;
use clawft_types::session::Session;

/// Compute the on-disk `.jsonl` filename for a session `key`, joined under
/// `sessions_dir`.
///
/// Uses percent-encoding (`NON_ALPHANUMERIC`) so any valid session key maps to
/// a safe, collision-free filename that [`SessionManager::list_sessions`]
/// decodes back to the original key. This is the single source of truth for the
/// path scheme, shared with [`crate::agent::local_file_sink::LocalFileSink`]
/// (M3 store-collapse, design §D4) so the in-process sink reuses the **exact**
/// same files SessionManager writes today — zero migration.
pub(crate) fn session_key_stem(key: &str) -> String {
    percent_encode(key.as_bytes(), NON_ALPHANUMERIC).to_string()
}

pub(crate) fn session_file_path(sessions_dir: &Path, key: &str) -> PathBuf {
    sessions_dir.join(format!("{}.jsonl", session_key_stem(key)))
}

/// Legacy on-disk basename for a session key (pre-percent-encoding).
///
/// Older builds replaced only `:` with `_` (e.g. `telegram:123` →
/// `telegram_123.jsonl`). Used by migration and [`gc_migrated_session_files`].
pub(crate) fn legacy_underscore_session_filename(key: &str) -> String {
    format!("{}.jsonl", key.replace(':', "_"))
}

/// Report from a session GC pass that removes orphaned legacy underscore-
/// encoded session files after a successful migrate to percent-encoded names.
///
/// See [`gc_migrated_session_files`] and `weft sessions gc` (WEFT-87).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionGcReport {
    /// Legacy basenames removed (or that *would* be removed when `dry_run`).
    pub removed: Vec<String>,
    /// Legacy basenames skipped because content did not match the migrated copy.
    pub skipped_mismatch: Vec<String>,
    /// When true, no files were deleted — `removed` lists candidates only.
    pub dry_run: bool,
    /// Observation archive dirs removed because their sibling `.jsonl` is gone.
    pub removed_observations: Vec<String>,
}

impl SessionGcReport {
    /// Number of legacy files removed (or that would be removed in dry-run).
    pub fn count(&self) -> usize {
        self.removed.len()
    }
}

/// Remove legacy underscore-encoded session files that already have a verified
/// percent-encoded twin (WEFT-87).
///
/// Scans `sessions_dir` for `.jsonl` files whose percent-decoded stem is a
/// session key containing `:`. For each such key, if
/// `{key with :→_}.jsonl` also exists and its bytes match the percent-encoded
/// file, the legacy file is deleted (or reported when `dry_run` is true).
///
/// Orphaned legacy-only files (never loaded/migrated) are left alone so we
/// never discard the only copy. Content mismatches are skipped with a warn.
///
/// # Errors
///
/// Returns [`ClawftError::Io`] if listing the directory or reading/removing a
/// file fails fatally. Individual remove failures are logged and do not abort
/// the pass when other candidates remain.
pub async fn gc_migrated_session_files<P: Platform>(
    platform: &P,
    sessions_dir: &Path,
    dry_run: bool,
) -> clawft_types::Result<SessionGcReport> {
    let entries = platform
        .fs()
        .list_dir(sessions_dir)
        .await
        .map_err(ClawftError::Io)?;

    let mut report = SessionGcReport {
        dry_run,
        ..Default::default()
    };

    for entry in entries {
        let Some(name) = entry.file_name() else {
            continue;
        };
        let name = name.to_string_lossy();
        let Some(stem) = name.strip_suffix(".jsonl") else {
            continue;
        };

        // Only consider percent-encoded (new-format) files: decode stem → key.
        let key = match percent_decode_str(stem).decode_utf8() {
            Ok(decoded) => decoded.into_owned(),
            Err(e) => {
                warn!(filename = %name, error = %e, "gc: skipping undecodable session filename");
                continue;
            }
        };

        // Legacy encoding only rewrote `:`; keys without colons share no
        // distinct legacy twin under that scheme.
        if !key.contains(':') {
            continue;
        }

        let new_path = session_file_path(sessions_dir, &key);
        // Require the entry we listed to be the canonical new path (guards
        // against treating a raw underscore file as "new" when decode is a no-op).
        if entry != new_path {
            continue;
        }

        let old_name = legacy_underscore_session_filename(&key);
        let old_path = sessions_dir.join(&old_name);
        if old_path == new_path {
            continue;
        }
        if !platform.fs().exists(&old_path).await {
            continue;
        }

        let new_content = match platform.fs().read_to_string(&new_path).await {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    path = %new_path.display(),
                    error = %e,
                    "gc: failed to read migrated session file; skipping twin"
                );
                continue;
            }
        };
        let old_content = match platform.fs().read_to_string(&old_path).await {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    path = %old_path.display(),
                    error = %e,
                    "gc: failed to read legacy session file; skipping"
                );
                continue;
            }
        };

        if new_content != old_content {
            warn!(
                key = %key,
                old = %old_path.display(),
                new = %new_path.display(),
                "gc: legacy and migrated session content differ; leaving legacy file"
            );
            report.skipped_mismatch.push(old_name);
            continue;
        }

        if dry_run {
            debug!(
                key = %key,
                old = %old_name,
                "gc dry-run: would remove legacy underscore session file"
            );
            report.removed.push(old_name);
            continue;
        }

        match platform.fs().remove_file(&old_path).await {
            Ok(()) => {
                debug!(
                    key = %key,
                    old = %old_name,
                    "gc: removed legacy underscore session file"
                );
                report.removed.push(old_name);
            }
            Err(e) => {
                warn!(
                    path = %old_path.display(),
                    error = %e,
                    "gc: failed to remove legacy session file"
                );
            }
        }
    }

    report.removed.sort();
    report.skipped_mismatch.sort();
    report.removed_observations =
        crate::observation_pack::gc_orphaned_observation_dirs(platform, sessions_dir, dry_run)
            .await;
    Ok(report)
}

/// Resolve the sessions directory the same way [`SessionManager::new`] does:
///
/// 1. the project workspace's `.clawft/sessions/` when a workspace is found
///    (`$CLAWFT_WORKSPACE`, else a `.clawft/` directory walking up from the
///    current directory — the same discovery memory and skills use), unless
///    that workspace is the home directory itself;
/// 2. else `~/.clawft/workspace/sessions/` if it exists,
/// 3. else `~/.nanobot/workspace/sessions/` (legacy fallback) if it exists,
/// 4. else create and return `~/.clawft/workspace/sessions/`.
///
/// Shared with [`crate::agent::local_file_sink::LocalFileSink`] so both the
/// legacy manager and the M3 in-process sink agree on the directory.
pub(crate) async fn discover_sessions_dir<P: Platform>(
    platform: &P,
) -> clawft_types::Result<PathBuf> {
    discover_sessions_dir_in(platform, crate::workspace::discover_workspace()).await
}

/// [`discover_sessions_dir`] with the workspace root passed in (testable).
pub(crate) async fn discover_sessions_dir_in<P: Platform>(
    platform: &P,
    workspace_root: Option<PathBuf>,
) -> clawft_types::Result<PathBuf> {
    let home = platform
        .fs()
        .home_dir()
        .ok_or_else(|| ClawftError::ConfigInvalid {
            reason: "cannot determine home directory".into(),
        })?;

    // A project workspace keeps its own conversations. `~/.clawft` found by
    // walking up from somewhere under $HOME is the global workspace, which
    // keeps the legacy `~/.clawft/workspace/sessions` layout below.
    if let Some(root) = workspace_root {
        let dot_clawft = root.join(".clawft");
        if root != home && platform.fs().exists(&dot_clawft).await {
            let dir = dot_clawft.join("sessions");
            platform
                .fs()
                .create_dir_all(&dir)
                .await
                .map_err(ClawftError::Io)?;
            debug!(path = %dir.display(), "using workspace sessions dir");
            return Ok(dir);
        }
    }

    let clawft_dir = home.join(".clawft").join("workspace").join("sessions");
    let nanobot_dir = home.join(".nanobot").join("workspace").join("sessions");

    if platform.fs().exists(&clawft_dir).await {
        debug!(path = %clawft_dir.display(), "using clawft sessions dir");
        Ok(clawft_dir)
    } else if platform.fs().exists(&nanobot_dir).await {
        debug!(path = %nanobot_dir.display(), "using nanobot sessions dir (fallback)");
        Ok(nanobot_dir)
    } else {
        debug!(
            path = %clawft_dir.display(),
            "sessions dir does not exist, creating"
        );
        platform
            .fs()
            .create_dir_all(&clawft_dir)
            .await
            .map_err(ClawftError::Io)?;
        Ok(clawft_dir)
    }
}

/// Parse a session `.jsonl` document (metadata header on line 1, one message
/// object per line thereafter) into a [`Session`].
///
/// This is the single decoder for the on-disk session format, shared by
/// [`SessionManager::load_session`] and
/// [`crate::agent::local_file_sink::LocalFileSink::load_session`] (M3
/// store-collapse, design §D4/§P5) so the CLI's `sessions` reader and the
/// retained bridge reader agree byte-for-byte on how a file decodes. Malformed
/// message lines are dropped with a warn (they never fail the whole load);
/// missing header fields fall back to sensible defaults.
///
/// # Errors
///
/// Returns [`ClawftError::ConfigInvalid`] if `content` is empty (no header
/// line), or [`ClawftError::Json`] if the header line is not valid JSON.
pub(crate) fn session_from_jsonl(
    key: &str,
    content: &str,
    path: &Path,
) -> clawft_types::Result<Session> {
    let mut lines = content.lines();

    // Parse metadata line.
    let meta_line = lines.next().ok_or_else(|| ClawftError::ConfigInvalid {
        reason: format!("session file is empty: {}", path.display()),
    })?;

    let meta: serde_json::Value = serde_json::from_str(meta_line)?;

    let created_at = meta
        .get("created_at")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(Utc::now);

    let updated_at = meta
        .get("updated_at")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(Utc::now);

    let metadata: HashMap<String, serde_json::Value> = meta
        .get("metadata")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let last_consolidated = meta
        .get("last_consolidated")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    // Parse message lines.
    let mut messages = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(msg) => messages.push(msg),
            Err(e) => {
                warn!(
                    key = key,
                    error = %e,
                    "skipping malformed message line in session"
                );
            }
        }
    }

    debug!(
        key = key,
        messages = messages.len(),
        "loaded session from disk"
    );

    Ok(Session {
        key: key.to_string(),
        messages,
        created_at,
        updated_at,
        metadata,
        last_consolidated,
    })
}

/// Manages conversation sessions with in-memory caching and JSONL persistence.
///
/// Sessions are identified by a string key (typically `"{channel}:{chat_id}"`).
/// The manager uses a write-through cache: reads check the in-memory cache
/// first, then fall back to loading from disk. Writes update both the cache
/// and the JSONL file.
///
/// # JSONL format
///
/// Each session file is named `{sanitized_key}.jsonl` and contains:
/// - Line 1: metadata object with `_type`, `created_at`, `updated_at`,
///   `metadata`, and `last_consolidated` fields.
/// - Lines 2+: message objects with `role`, `content`, and `timestamp` fields.
///
/// # Platform abstraction
///
/// All filesystem I/O goes through the [`Platform::fs()`] trait, making
/// SessionManager testable with mock filesystems and WASM-portable.
pub struct SessionManager<P: Platform> {
    /// Directory where JSONL session files are stored.
    sessions_dir: PathBuf,

    /// In-memory cache of active sessions.
    active_sessions: Arc<Mutex<HashMap<String, Session>>>,

    /// Platform providing filesystem access.
    platform: Arc<P>,

    /// Emit one `session.append` chain event per N successful appends
    /// (WEFT-85 sample-rate cap). `1` = every turn (default). Values
    /// below 1 are treated as 1. Counter is process-wide for this manager.
    append_event_every_n: u32,

    /// Monotonic counter of successful `append_turn` calls (for sampling).
    append_event_counter: AtomicU64,
}

impl<P: Platform> SessionManager<P> {
    /// Create a new session manager using the given platform.
    ///
    /// Discovers the sessions directory by checking:
    /// 1. `~/.clawft/workspace/sessions/`
    /// 2. `~/.nanobot/workspace/sessions/` (legacy fallback)
    ///
    /// If neither exists, defaults to `~/.clawft/workspace/sessions/` and
    /// creates it. Returns an error if the home directory cannot be determined.
    pub async fn new(platform: Arc<P>) -> clawft_types::Result<Self> {
        let sessions_dir = discover_sessions_dir(platform.as_ref()).await?;

        Ok(Self {
            sessions_dir,
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            platform,
            append_event_every_n: 1,
            append_event_counter: AtomicU64::new(0),
        })
    }

    /// Create a session manager with an explicit sessions directory.
    ///
    /// Useful for testing or when the directory is already known.
    pub fn with_dir(platform: Arc<P>, sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            platform,
            append_event_every_n: 1,
            append_event_counter: AtomicU64::new(0),
        }
    }

    /// Cap `session.append` chain-event volume: emit one event per `n`
    /// successful appends (WEFT-85). `n = 1` (default) emits every turn.
    ///
    /// Use when long-running sessions would flood the ExoChain; e.g.
    /// `with_append_event_every_n(10)` keeps ~10% of append markers.
    pub fn with_append_event_every_n(mut self, n: u32) -> Self {
        self.append_event_every_n = n.max(1);
        self
    }

    /// Current sample-rate cap (`1` = emit every append).
    pub fn append_event_every_n(&self) -> u32 {
        self.append_event_every_n.max(1)
    }

    /// Get an existing session or create a new one.
    ///
    /// Checks the in-memory cache first, then attempts to load from disk.
    /// If neither succeeds, creates a fresh empty session and caches it.
    ///
    /// # Errors
    ///
    /// Returns an error if `key` fails session-ID validation.
    pub async fn get_or_create(&self, key: &str) -> clawft_types::Result<Session> {
        crate::security::validate_session_id(key)?;

        // Check cache first.
        {
            let cache = self.active_sessions.lock().await;
            if let Some(session) = cache.get(key) {
                return Ok(session.clone());
            }
        }

        // Try loading from disk.
        if let Ok(session) = self.load_session(key).await {
            let mut cache = self.active_sessions.lock().await;
            cache.insert(key.to_string(), session.clone());
            return Ok(session);
        }

        // Create new session.
        let session = Session::new(key);
        let mut cache = self.active_sessions.lock().await;
        cache.insert(key.to_string(), session.clone());

        // Chain event marker for session creation.
        crate::chain_event!(
            "session",
            crate::chain_event::EVENT_KIND_SESSION_CREATE,
            { "key": key }
        );

        Ok(session)
    }

    /// Load a session from its JSONL file on disk.
    ///
    /// Parses the first line as metadata and remaining lines as messages.
    /// Returns an error if the file does not exist or contains invalid JSON.
    ///
    /// Includes a migration path: if the percent-encoded file does not exist
    /// but the old underscore-encoded file does, the content is copied to the
    /// new filename. After a successful write, the legacy file is removed
    /// (WEFT-87 self-cleanup). Bulk cleanup of already-migrated orphans is
    /// available via [`Self::gc_migrated`] / [`gc_migrated_session_files`].
    pub async fn load_session(&self, key: &str) -> clawft_types::Result<Session> {
        crate::security::validate_session_id(key)?;
        let path = self.session_path(key);

        // Migration: try old-format filename if new-format doesn't exist
        if !self.platform.fs().exists(&path).await {
            let old_filename = legacy_underscore_session_filename(key);
            let old_path = self.sessions_dir.join(&old_filename);
            if self.platform.fs().exists(&old_path).await {
                warn!(
                    key = key,
                    old = %old_path.display(),
                    new = %path.display(),
                    "migrating session file from old encoding format"
                );
                // Read from old, write to new, then remove old once verified.
                let content = self.platform.fs().read_to_string(&old_path).await?;
                self.platform.fs().write_string(&path, &content).await?;
                // Self-cleanup (WEFT-87): drop legacy twin only after the
                // percent-encoded copy is intact.
                match self.platform.fs().read_to_string(&path).await {
                    Ok(written) if written == content => {
                        if let Err(e) = self.platform.fs().remove_file(&old_path).await {
                            warn!(
                                key = key,
                                old = %old_path.display(),
                                error = %e,
                                "migrated session but failed to remove legacy underscore file"
                            );
                        } else {
                            debug!(
                                key = key,
                                old = %old_filename,
                                "removed legacy underscore session file after migration"
                            );
                        }
                    }
                    Ok(_) => {
                        warn!(
                            key = key,
                            old = %old_path.display(),
                            new = %path.display(),
                            "migrated session content mismatch; leaving legacy file"
                        );
                    }
                    Err(e) => {
                        warn!(
                            key = key,
                            new = %path.display(),
                            error = %e,
                            "could not verify migrated session; leaving legacy file"
                        );
                    }
                }
            }
        }

        let content = self.platform.fs().read_to_string(&path).await?;

        session_from_jsonl(key, &content, &path)
    }

    /// Garbage-collect legacy underscore-encoded session files that already
    /// have a verified percent-encoded twin (WEFT-87).
    ///
    /// When `dry_run` is true, reports candidates without deleting. See
    /// [`gc_migrated_session_files`].
    pub async fn gc_migrated(&self, dry_run: bool) -> clawft_types::Result<SessionGcReport> {
        gc_migrated_session_files(self.platform.as_ref(), &self.sessions_dir, dry_run).await
    }

    /// Save a session to its JSONL file on disk.
    ///
    /// Writes the full session: metadata line followed by all message lines.
    /// Also updates the in-memory cache.
    pub async fn save_session(&self, session: &Session) -> clawft_types::Result<()> {
        let path = self.session_path(&session.key);

        let meta = serde_json::json!({
            "_type": "metadata",
            "created_at": session.created_at.to_rfc3339(),
            "updated_at": session.updated_at.to_rfc3339(),
            "metadata": session.metadata,
            "last_consolidated": session.last_consolidated,
        });

        let mut content = serde_json::to_string(&meta).map_err(ClawftError::Json)?;
        content.push('\n');

        for msg in &session.messages {
            content.push_str(&serde_json::to_string(msg).map_err(ClawftError::Json)?);
            content.push('\n');
        }

        self.platform.fs().write_string(&path, &content).await?;

        // Update cache.
        let mut cache = self.active_sessions.lock().await;
        cache.insert(session.key.clone(), session.clone());

        debug!(key = %session.key, "saved session to disk");

        Ok(())
    }

    /// Append a single conversation turn to a session.
    ///
    /// Updates both the in-memory cache and appends to the JSONL file on disk.
    /// If the session does not exist yet, it is created via [`get_or_create`].
    ///
    /// Emits a `session.append` chain event on every successful append
    /// (WEFT-85 / MW-7), subject to [`Self::with_append_event_every_n`].
    /// Payload: `key`, `role`, `turn_count` (not message content).
    pub async fn append_turn(
        &self,
        key: &str,
        role: &str,
        content: &str,
    ) -> clawft_types::Result<()> {
        crate::security::validate_session_id(key)?;
        let mut session = self.get_or_create(key).await?;
        session.add_message(role, content, None);
        let turn_count = session.messages.len();

        // Append message line to file.
        let msg = serde_json::json!({
            "role": role,
            "content": content,
            "timestamp": Utc::now().to_rfc3339(),
        });
        let mut line = serde_json::to_string(&msg).map_err(ClawftError::Json)?;
        line.push('\n');

        let path = self.session_path(key);

        // If the file does not exist yet, write the full session (with metadata).
        if !self.platform.fs().exists(&path).await {
            self.save_session(&session).await?;
        } else {
            self.platform.fs().append_string(&path, &line).await?;
            // Update cache.
            let mut cache = self.active_sessions.lock().await;
            cache.insert(key.to_string(), session);
        }

        // Chain event for every appended turn (sample-rate capped).
        // Count only after a successful write so failed appends stay silent.
        self.maybe_emit_append_event(key, role, turn_count);

        Ok(())
    }

    /// Emit `session.append` when the sample-rate counter hits the cap.
    fn maybe_emit_append_event(&self, key: &str, role: &str, turn_count: usize) {
        let every_n = self.append_event_every_n() as u64;
        let seq = self.append_event_counter.fetch_add(1, Ordering::Relaxed) + 1;
        if seq % every_n != 0 {
            return;
        }
        // Bare identifiers (not "key") so stringify! yields clean JSON keys
        // (`key` not `"key"`). Same values as the create/destroy markers.
        crate::chain_event!(
            "session",
            crate::chain_event::EVENT_KIND_SESSION_APPEND,
            {
                key: key,
                role: role,
                turn_count: turn_count
            }
        );
    }

    /// List all session keys (derived from `.jsonl` filenames on disk).
    ///
    /// Decodes percent-encoded filenames back to the original session key.
    /// Files that cannot be decoded as valid UTF-8 are skipped with a warning.
    pub async fn list_sessions(&self) -> clawft_types::Result<Vec<String>> {
        let entries = self
            .platform
            .fs()
            .list_dir(&self.sessions_dir)
            .await
            .map_err(ClawftError::Io)?;

        let mut keys = Vec::new();
        for entry in entries {
            if let Some(name) = entry.file_name() {
                let name = name.to_string_lossy();
                if let Some(stem) = name.strip_suffix(".jsonl") {
                    match percent_decode_str(stem).decode_utf8() {
                        Ok(decoded) => keys.push(decoded.into_owned()),
                        Err(e) => {
                            warn!(filename = %name, error = %e, "skipping undecodable session filename");
                        }
                    }
                }
            }
        }

        keys.sort();
        Ok(keys)
    }

    /// Remove a session from the in-memory cache.
    ///
    /// The JSONL file on disk is not deleted; only the cached copy is
    /// evicted. The next [`get_or_create`] call will reload from disk.
    pub async fn invalidate(&self, key: &str) {
        let mut cache = self.active_sessions.lock().await;
        cache.remove(key);
        debug!(key = key, "invalidated session cache entry");
    }

    /// Delete a session file from disk and remove from cache.
    pub async fn delete_session(&self, key: &str) -> clawft_types::Result<()> {
        crate::security::validate_session_id(key)?;
        let path = self.session_path(key);
        if self.platform.fs().exists(&path).await {
            self.platform
                .fs()
                .remove_file(&path)
                .await
                .map_err(ClawftError::Io)?;
        }
        crate::observation_pack::remove_observations_dir(
            self.platform.as_ref(),
            &crate::observation_pack::observations_dir(&self.sessions_dir, key),
        )
        .await;
        self.invalidate(key).await;

        // Chain event marker for session destruction.
        crate::chain_event!(
            "session",
            crate::chain_event::EVENT_KIND_SESSION_DESTROY,
            { "key": key }
        );

        Ok(())
    }

    /// Get the sessions directory path.
    pub fn sessions_dir(&self) -> &PathBuf {
        &self.sessions_dir
    }

    /// Compute the filesystem path for a session key.
    ///
    /// Uses percent-encoding to safely represent any valid session key as
    /// a filename. This is reversible: `list_sessions()` decodes back to
    /// the original key.
    fn session_path(&self, key: &str) -> PathBuf {
        session_file_path(&self.sessions_dir, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use clawft_platform::fs::FileSystem;
    use std::sync::Mutex as StdMutex;

    // -- Mock platform for testing without real I/O --

    /// In-memory filesystem for test isolation.
    struct MockFs {
        files: StdMutex<HashMap<PathBuf, String>>,
        dirs: StdMutex<Vec<PathBuf>>,
    }

    impl MockFs {
        fn new() -> Self {
            Self {
                files: StdMutex::new(HashMap::new()),
                dirs: StdMutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl clawft_platform::fs::FileSystem for MockFs {
        async fn read_to_string(&self, path: &std::path::Path) -> std::io::Result<String> {
            let files = self.files.lock().unwrap();
            files.get(path).cloned().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("file not found: {}", path.display()),
                )
            })
        }

        async fn write_string(&self, path: &std::path::Path, content: &str) -> std::io::Result<()> {
            // Create parent dirs implicitly.
            if let Some(parent) = path.parent() {
                let mut dirs = self.dirs.lock().unwrap();
                if !dirs.contains(&parent.to_path_buf()) {
                    dirs.push(parent.to_path_buf());
                }
            }
            let mut files = self.files.lock().unwrap();
            files.insert(path.to_path_buf(), content.to_string());
            Ok(())
        }

        async fn append_string(
            &self,
            path: &std::path::Path,
            content: &str,
        ) -> std::io::Result<()> {
            let mut files = self.files.lock().unwrap();
            let entry = files.entry(path.to_path_buf()).or_default();
            entry.push_str(content);
            Ok(())
        }

        async fn exists(&self, path: &std::path::Path) -> bool {
            let files = self.files.lock().unwrap();
            if files.contains_key(path) {
                return true;
            }
            let dirs = self.dirs.lock().unwrap();
            dirs.contains(&path.to_path_buf())
        }

        async fn list_dir(&self, path: &std::path::Path) -> std::io::Result<Vec<PathBuf>> {
            let files = self.files.lock().unwrap();
            let mut entries = Vec::new();
            for file_path in files.keys() {
                if file_path.parent() == Some(path) {
                    entries.push(file_path.clone());
                }
            }
            Ok(entries)
        }

        async fn create_dir_all(&self, path: &std::path::Path) -> std::io::Result<()> {
            let mut dirs = self.dirs.lock().unwrap();
            if !dirs.contains(&path.to_path_buf()) {
                dirs.push(path.to_path_buf());
            }
            Ok(())
        }

        async fn remove_file(&self, path: &std::path::Path) -> std::io::Result<()> {
            let mut files = self.files.lock().unwrap();
            files.remove(path);
            Ok(())
        }

        fn home_dir(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/mock-home"))
        }

        async fn metadata(
            &self,
            path: &std::path::Path,
        ) -> std::io::Result<clawft_platform::fs::FsMetadata> {
            if let Some(content) = self.files.lock().unwrap().get(path) {
                return Ok(clawft_platform::fs::FsMetadata {
                    is_dir: false,
                    len: content.len() as u64,
                });
            }
            if self.dirs.lock().unwrap().contains(&path.to_path_buf()) {
                return Ok(clawft_platform::fs::FsMetadata {
                    is_dir: true,
                    len: 0,
                });
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not found",
            ))
        }
    }

    struct MockEnv;

    impl clawft_platform::env::Environment for MockEnv {
        fn get_var(&self, _name: &str) -> Option<String> {
            None
        }
        fn set_var(&self, _name: &str, _value: &str) {}
        fn remove_var(&self, _name: &str) {}
    }

    struct MockHttp;

    #[async_trait]
    impl clawft_platform::http::HttpClient for MockHttp {
        async fn request(
            &self,
            _method: &str,
            _url: &str,
            _headers: &HashMap<String, String>,
            _body: Option<&[u8]>,
        ) -> Result<clawft_platform::http::HttpResponse, Box<dyn std::error::Error + Send + Sync>>
        {
            Err(
                "MockHttp::request not implemented — use a real HTTP client for integration tests"
                    .into(),
            )
        }
    }

    struct MockPlatform {
        fs: MockFs,
        env: MockEnv,
        http: MockHttp,
    }

    impl MockPlatform {
        fn new() -> Self {
            Self {
                fs: MockFs::new(),
                env: MockEnv,
                http: MockHttp,
            }
        }
    }

    #[async_trait]
    impl Platform for MockPlatform {
        fn http(&self) -> &dyn clawft_platform::http::HttpClient {
            &self.http
        }

        fn fs(&self) -> &dyn clawft_platform::fs::FileSystem {
            &self.fs
        }

        fn env(&self) -> &dyn clawft_platform::env::Environment {
            &self.env
        }

        fn process(&self) -> Option<&dyn clawft_platform::process::ProcessSpawner> {
            None
        }
    }

    fn make_platform() -> Arc<MockPlatform> {
        Arc::new(MockPlatform::new())
    }

    fn make_manager(platform: Arc<MockPlatform>) -> SessionManager<MockPlatform> {
        let sessions_dir = PathBuf::from("/mock-home/.clawft/workspace/sessions");
        SessionManager::with_dir(platform, sessions_dir)
    }

    #[tokio::test]
    async fn get_or_create_new_session() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let session = mgr.get_or_create("telegram:123").await.unwrap();
        assert_eq!(session.key, "telegram:123");
        assert!(session.messages.is_empty());
    }

    #[tokio::test]
    async fn get_or_create_returns_cached() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let session1 = mgr.get_or_create("test:key").await.unwrap();
        let session2 = mgr.get_or_create("test:key").await.unwrap();
        // Both should have the same creation time (cached).
        assert_eq!(session1.created_at, session2.created_at);
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let mut session = Session::new("roundtrip:test");
        session.add_message("user", "hello world", None);
        session.add_message("assistant", "hi there", None);

        mgr.save_session(&session).await.unwrap();

        // Invalidate cache to force load from disk.
        mgr.invalidate("roundtrip:test").await;

        let loaded = mgr.load_session("roundtrip:test").await.unwrap();
        assert_eq!(loaded.key, "roundtrip:test");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0]["role"], "user");
        assert_eq!(loaded.messages[0]["content"], "hello world");
        assert_eq!(loaded.messages[1]["role"], "assistant");
        assert_eq!(loaded.messages[1]["content"], "hi there");
    }

    #[tokio::test]
    async fn jsonl_format_correctness() {
        let platform = make_platform();
        let mgr = make_manager(platform.clone());

        let mut session = Session::new("fmt:check");
        session.add_message("user", "test", None);

        mgr.save_session(&session).await.unwrap();

        let path = PathBuf::from("/mock-home/.clawft/workspace/sessions/fmt%3Acheck.jsonl");
        let content = platform.fs.read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();

        // First line is metadata.
        assert_eq!(lines.len(), 2);
        let meta: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(meta["_type"], "metadata");
        assert!(meta.get("created_at").is_some());
        assert!(meta.get("updated_at").is_some());
        assert_eq!(meta["last_consolidated"], 0);

        // Second line is the message.
        let msg: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(msg["role"], "user");
        assert_eq!(msg["content"], "test");
    }

    #[tokio::test]
    async fn append_turn_creates_session_if_needed() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        mgr.append_turn("new:session", "user", "first message")
            .await
            .unwrap();

        let session = mgr.get_or_create("new:session").await.unwrap();
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0]["content"], "first message");
    }

    #[tokio::test]
    async fn append_turn_adds_to_existing() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        // Create and save initial session.
        let mut session = Session::new("append:test");
        session.add_message("user", "first", None);
        mgr.save_session(&session).await.unwrap();

        // Append a second turn.
        mgr.append_turn("append:test", "assistant", "second")
            .await
            .unwrap();

        let loaded = mgr.get_or_create("append:test").await.unwrap();
        assert_eq!(loaded.messages.len(), 2);
    }

    #[tokio::test]
    async fn list_sessions_returns_keys() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let s1 = Session::new("telegram:100");
        let s2 = Session::new("slack:200");
        mgr.save_session(&s1).await.unwrap();
        mgr.save_session(&s2).await.unwrap();

        let keys = mgr.list_sessions().await.unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"slack:200".to_string()));
        assert!(keys.contains(&"telegram:100".to_string()));
    }

    #[tokio::test]
    async fn invalidate_removes_from_cache() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        mgr.get_or_create("cache:test").await.unwrap();

        // Verify it is in cache.
        {
            let cache = mgr.active_sessions.lock().await;
            assert!(cache.contains_key("cache:test"));
        }

        mgr.invalidate("cache:test").await;

        {
            let cache = mgr.active_sessions.lock().await;
            assert!(!cache.contains_key("cache:test"));
        }
    }

    #[tokio::test]
    async fn session_path_uses_percent_encoding() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let path = mgr.session_path("telegram:12345");
        assert_eq!(
            path,
            PathBuf::from("/mock-home/.clawft/workspace/sessions/telegram%3A12345.jsonl")
        );
    }

    #[tokio::test]
    async fn roundtrip_key_with_underscores() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        // Key containing underscores must survive round-trip without corruption.
        let key = "telegram:user_123";
        let session = Session::new(key);
        mgr.save_session(&session).await.unwrap();

        let keys = mgr.list_sessions().await.unwrap();
        assert!(
            keys.contains(&key.to_string()),
            "list_sessions should contain '{key}', got: {keys:?}"
        );

        mgr.invalidate(key).await;
        let loaded = mgr.load_session(key).await.unwrap();
        assert_eq!(loaded.key, key);
    }

    #[tokio::test]
    async fn roundtrip_key_with_multiple_colons() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let key = "slack:channel:thread";
        let session = Session::new(key);
        mgr.save_session(&session).await.unwrap();

        let keys = mgr.list_sessions().await.unwrap();
        assert!(keys.contains(&key.to_string()));

        mgr.invalidate(key).await;
        let loaded = mgr.load_session(key).await.unwrap();
        assert_eq!(loaded.key, key);
    }

    #[tokio::test]
    async fn roundtrip_key_with_special_chars() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let key = "discord:guild#channel+123";
        let session = Session::new(key);
        mgr.save_session(&session).await.unwrap();

        let keys = mgr.list_sessions().await.unwrap();
        assert!(keys.contains(&key.to_string()));

        mgr.invalidate(key).await;
        let loaded = mgr.load_session(key).await.unwrap();
        assert_eq!(loaded.key, key);
    }

    #[tokio::test]
    async fn migration_from_old_underscore_format() {
        let platform = make_platform();
        let mgr = make_manager(platform.clone());

        // Simulate an old-format file written by the previous implementation.
        let old_path =
            PathBuf::from("/mock-home/.clawft/workspace/sessions/telegram_user_123.jsonl");
        let meta = serde_json::json!({
            "_type": "metadata",
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z",
            "metadata": {},
            "last_consolidated": 0,
        });
        let content = format!("{}\n", serde_json::to_string(&meta).unwrap());
        platform.fs.write_string(&old_path, &content).await.unwrap();

        // load_session should find the old file and migrate it.
        let key = "telegram:user_123";
        let loaded = mgr.load_session(key).await.unwrap();
        assert_eq!(loaded.key, key);

        // WEFT-87: legacy underscore file removed after successful migrate.
        // New path percent-encodes `_` as `%5F` as well as `:` as `%3A`.
        let new_path = session_file_path(
            Path::new("/mock-home/.clawft/workspace/sessions"),
            key,
        );
        assert!(
            platform.fs.exists(&new_path).await,
            "percent-encoded migrated file must exist at {}",
            new_path.display()
        );
        assert!(
            !platform.fs.exists(&old_path).await,
            "legacy underscore file must be removed after verified migration"
        );
    }

    #[tokio::test]
    async fn gc_migrated_removes_legacy_when_twin_matches() {
        let platform = make_platform();
        let mgr = make_manager(platform.clone());

        let content = "{\"_type\":\"metadata\",\"created_at\":\"2025-01-01T00:00:00Z\",\"updated_at\":\"2025-01-01T00:00:00Z\",\"metadata\":{},\"last_consolidated\":0}\n";
        let new_path =
            PathBuf::from("/mock-home/.clawft/workspace/sessions/telegram%3A12345.jsonl");
        let old_path = PathBuf::from("/mock-home/.clawft/workspace/sessions/telegram_12345.jsonl");
        platform.fs.write_string(&new_path, content).await.unwrap();
        platform.fs.write_string(&old_path, content).await.unwrap();

        // Dry-run first: reports without deleting.
        let dry = mgr.gc_migrated(true).await.unwrap();
        assert!(dry.dry_run);
        assert_eq!(dry.removed, vec!["telegram_12345.jsonl".to_string()]);
        assert!(platform.fs.exists(&old_path).await);

        // Real GC: removes legacy twin.
        let report = mgr.gc_migrated(false).await.unwrap();
        assert!(!report.dry_run);
        assert_eq!(report.count(), 1);
        assert_eq!(report.removed, vec!["telegram_12345.jsonl".to_string()]);
        assert!(platform.fs.exists(&new_path).await);
        assert!(!platform.fs.exists(&old_path).await);
    }

    #[tokio::test]
    async fn gc_migrated_skips_content_mismatch() {
        let platform = make_platform();
        let mgr = make_manager(platform.clone());

        let new_path = PathBuf::from("/mock-home/.clawft/workspace/sessions/slack%3A9.jsonl");
        let old_path = PathBuf::from("/mock-home/.clawft/workspace/sessions/slack_9.jsonl");
        platform
            .fs
            .write_string(&new_path, "new-content\n")
            .await
            .unwrap();
        platform
            .fs
            .write_string(&old_path, "old-content\n")
            .await
            .unwrap();

        let report = mgr.gc_migrated(false).await.unwrap();
        assert!(report.removed.is_empty());
        assert_eq!(report.skipped_mismatch, vec!["slack_9.jsonl".to_string()]);
        assert!(platform.fs.exists(&old_path).await);
        assert!(platform.fs.exists(&new_path).await);
    }

    #[tokio::test]
    async fn gc_migrated_leaves_orphan_legacy_only() {
        let platform = make_platform();
        let mgr = make_manager(platform.clone());

        // Legacy-only: never migrated — must not delete the only copy.
        let old_path =
            PathBuf::from("/mock-home/.clawft/workspace/sessions/discord_channel.jsonl");
        platform
            .fs
            .write_string(&old_path, "legacy-only\n")
            .await
            .unwrap();

        let report = mgr.gc_migrated(false).await.unwrap();
        assert!(report.removed.is_empty());
        assert!(platform.fs.exists(&old_path).await);
    }

    #[tokio::test]
    async fn legacy_underscore_filename_helper() {
        assert_eq!(
            legacy_underscore_session_filename("telegram:user_123"),
            "telegram_user_123.jsonl"
        );
        assert_eq!(
            legacy_underscore_session_filename("slack:channel:thread"),
            "slack_channel_thread.jsonl"
        );
    }

    #[tokio::test]
    async fn load_nonexistent_session_returns_error() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let result = mgr.load_session("nonexistent:key").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn save_preserves_metadata() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let mut session = Session::new("meta:test");
        session
            .metadata
            .insert("agent".into(), serde_json::json!("test-agent"));
        session.last_consolidated = 5;

        mgr.save_session(&session).await.unwrap();
        mgr.invalidate("meta:test").await;

        let loaded = mgr.load_session("meta:test").await.unwrap();
        assert_eq!(loaded.last_consolidated, 5);
        assert_eq!(loaded.metadata["agent"], "test-agent");
    }

    #[tokio::test]
    async fn new_discovers_sessions_dir() {
        let platform = make_platform();
        // The mock home is /mock-home, and neither sessions dir exists,
        // so `new` should create ~/.clawft/workspace/sessions/.
        let mgr = SessionManager::new(platform).await.unwrap();
        assert_eq!(
            mgr.sessions_dir,
            PathBuf::from("/mock-home/.clawft/workspace/sessions")
        );
    }

    #[tokio::test]
    async fn sessions_dir_follows_the_project_workspace() {
        let platform = make_platform();
        platform
            .fs()
            .create_dir_all(Path::new("/proj/.clawft"))
            .await
            .unwrap();
        let dir = discover_sessions_dir_in(platform.as_ref(), Some(PathBuf::from("/proj")))
            .await
            .unwrap();
        assert_eq!(dir, PathBuf::from("/proj/.clawft/sessions"));
        assert!(platform.fs().exists(&dir).await);
    }

    #[tokio::test]
    async fn sessions_dir_keeps_global_layout_for_home_or_no_workspace() {
        let platform = make_platform();
        platform
            .fs()
            .create_dir_all(Path::new("/mock-home/.clawft"))
            .await
            .unwrap();
        let global = PathBuf::from("/mock-home/.clawft/workspace/sessions");
        // `~/.clawft` found by walking up is the global workspace.
        let home_ws = discover_sessions_dir_in(platform.as_ref(), Some(PathBuf::from("/mock-home")))
            .await
            .unwrap();
        assert_eq!(home_ws, global);
        // A workspace root without a `.clawft/` dir is ignored.
        let missing = discover_sessions_dir_in(platform.as_ref(), Some(PathBuf::from("/nope")))
            .await
            .unwrap();
        assert_eq!(missing, global);
        let none = discover_sessions_dir_in(platform.as_ref(), None).await.unwrap();
        assert_eq!(none, global);
    }

    #[tokio::test]
    async fn list_sessions_empty_dir() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let keys = mgr.list_sessions().await.unwrap();
        assert!(keys.is_empty());
    }

    /// WEFT-85: 100 appends produce 100 `session.append` chain events (default rate).
    #[tokio::test]
    async fn append_turn_emits_session_append_chain_event_every_turn() {
        let platform = make_platform();
        let mgr = make_manager(platform);
        let _ = crate::chain_event::drain_pending_chain_events();

        for i in 0..100 {
            mgr.append_turn("chain:100", "user", &format!("msg-{i}"))
                .await
                .unwrap();
        }

        let events = crate::chain_event::drain_pending_chain_events();
        let appends: Vec<_> = events
            .iter()
            .filter(|e| e.kind == crate::chain_event::EVENT_KIND_SESSION_APPEND)
            .collect();
        assert_eq!(
            appends.len(),
            100,
            "default sample rate must emit one session.append per turn; got {} appends in {} total events",
            appends.len(),
            events.len()
        );
        assert!(appends.iter().all(|e| e.source == "session"));
        // First append may also have produced session.create — do not require exclusivity.
        let last = appends.last().unwrap();
        let payload = last.payload.as_ref().expect("payload present");
        assert_eq!(payload["key"], "chain:100");
        assert_eq!(payload["role"], "user");
        // turn_count is stringified by chain_event! via format!("{}", ...)
        assert_eq!(payload["turn_count"], "100");
    }

    /// WEFT-85: sample-rate cap reduces event volume deterministically.
    #[tokio::test]
    async fn append_turn_sample_rate_caps_chain_events() {
        let platform = make_platform();
        let sessions_dir = PathBuf::from("/mock-home/.clawft/workspace/sessions");
        let mgr = SessionManager::with_dir(platform, sessions_dir).with_append_event_every_n(10);
        assert_eq!(mgr.append_event_every_n(), 10);
        let _ = crate::chain_event::drain_pending_chain_events();

        for i in 0..100 {
            mgr.append_turn("chain:sampled", "assistant", &format!("msg-{i}"))
                .await
                .unwrap();
        }

        let events = crate::chain_event::drain_pending_chain_events();
        let appends: Vec<_> = events
            .iter()
            .filter(|e| e.kind == crate::chain_event::EVENT_KIND_SESSION_APPEND)
            .collect();
        assert_eq!(
            appends.len(),
            10,
            "every_n=10 over 100 turns must emit 10 session.append events"
        );
    }

    /// WEFT-85: create + destroy still emit; append is independent.
    #[tokio::test]
    async fn session_lifecycle_chain_events_include_append() {
        let platform = make_platform();
        let mgr = make_manager(platform);
        let _ = crate::chain_event::drain_pending_chain_events();

        mgr.append_turn("life:1", "user", "hi").await.unwrap();
        mgr.append_turn("life:1", "assistant", "yo").await.unwrap();
        mgr.delete_session("life:1").await.unwrap();

        let events = crate::chain_event::drain_pending_chain_events();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert!(
            kinds.contains(&crate::chain_event::EVENT_KIND_SESSION_CREATE),
            "expected session.create in {kinds:?}"
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == crate::chain_event::EVENT_KIND_SESSION_APPEND)
                .count(),
            2
        );
        assert!(
            kinds.contains(&crate::chain_event::EVENT_KIND_SESSION_DESTROY),
            "expected session.destroy in {kinds:?}"
        );
    }
}
