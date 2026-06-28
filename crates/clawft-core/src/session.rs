//! Session management for conversation persistence.
//!
//! Provides [`SessionManager`] which caches active sessions in memory and
//! persists them to disk as JSONL files using the platform filesystem
//! abstraction. Each JSONL file has a metadata header line followed by
//! one line per conversation turn.
//!
//! Ported from Python `nanobot/session/manager.py`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::runtime::Mutex;
use chrono::Utc;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, percent_encode};
use tracing::{debug, warn};

use clawft_platform::Platform;
use clawft_types::error::ClawftError;
use clawft_types::session::Session;

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
        let home = platform
            .fs()
            .home_dir()
            .ok_or_else(|| ClawftError::ConfigInvalid {
                reason: "cannot determine home directory".into(),
            })?;

        let clawft_dir = home.join(".clawft").join("workspace").join("sessions");
        let nanobot_dir = home.join(".nanobot").join("workspace").join("sessions");

        let sessions_dir = if platform.fs().exists(&clawft_dir).await {
            debug!(path = %clawft_dir.display(), "using clawft sessions dir");
            clawft_dir
        } else if platform.fs().exists(&nanobot_dir).await {
            debug!(path = %nanobot_dir.display(), "using nanobot sessions dir (fallback)");
            nanobot_dir
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
            clawft_dir
        };

        Ok(Self {
            sessions_dir,
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            platform,
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
        }
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
    /// new filename (the old file is preserved for safety).
    pub async fn load_session(&self, key: &str) -> clawft_types::Result<Session> {
        crate::security::validate_session_id(key)?;
        let path = self.session_path(key);

        // Migration: try old-format filename if new-format doesn't exist
        if !self.platform.fs().exists(&path).await {
            let old_filename = format!("{}.jsonl", key.replace(':', "_"));
            let old_path = self.sessions_dir.join(&old_filename);
            if self.platform.fs().exists(&old_path).await {
                warn!(
                    key = key,
                    old = %old_path.display(),
                    new = %path.display(),
                    "migrating session file from old encoding format"
                );
                // Read from old, write to new, keep old for safety
                let content = self.platform.fs().read_to_string(&old_path).await?;
                self.platform.fs().write_string(&path, &content).await?;
            }
        }

        let content = self.platform.fs().read_to_string(&path).await?;

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
    pub async fn append_turn(
        &self,
        key: &str,
        role: &str,
        content: &str,
    ) -> clawft_types::Result<()> {
        crate::security::validate_session_id(key)?;
        let mut session = self.get_or_create(key).await?;
        session.add_message(role, content, None);

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

        Ok(())
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
        let encoded = percent_encode(key.as_bytes(), NON_ALPHANUMERIC).to_string();
        let filename = format!("{encoded}.jsonl");
        self.sessions_dir.join(filename)
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
        let loaded = mgr.load_session("telegram:user_123").await.unwrap();
        assert_eq!(loaded.key, "telegram:user_123");
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
    async fn list_sessions_empty_dir() {
        let platform = make_platform();
        let mgr = make_manager(platform);

        let keys = mgr.list_sessions().await.unwrap();
        assert!(keys.is_empty());
    }
}
