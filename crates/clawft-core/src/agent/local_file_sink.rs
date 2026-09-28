//! In-process [`ConversationSink`] backed by the existing session JSONL files.
//!
//! [`LocalFileSink`] is the no-daemon backend for the M3 store-collapse
//! (design `.planning/hermes-loop/m3-store-collapse-design.md` §D4): when the
//! CLI runs the agent loop in-process there is no substrate to persist turns,
//! so today it uses the no-op [`InMemorySink`](super::sink::InMemorySink) and
//! all durability rides on the (soon-retired) `SessionManager`. This sink
//! replaces that no-op with real durability that **reuses the exact same
//! files** `SessionManager` writes:
//! `~/.clawft/workspace/sessions/{percent-encoded-key}.jsonl`. Same path, same
//! format — zero migration. The old session files simply *become* this sink's
//! backing store.
//!
//! ## File format (unchanged from `SessionManager`)
//!
//! - **Line 1** — a metadata header object: `_type`, `created_at`,
//!   `updated_at`, `metadata`, `last_consolidated`. The per-conversation
//!   metadata sidecar (design §D3, home of the hallucination score) lives in
//!   this header's `metadata` field — [`meta`](LocalFileSink::meta) /
//!   [`set_meta`](LocalFileSink::set_meta) read and write it.
//! - **Lines 2+** — one message object per turn: `role`, `content`,
//!   `timestamp`, plus optional `tool_calls` / `tool_call_id`. This is the
//!   **superset** (design §1): tool-loop intermediates are recorded here just
//!   like the substrate sink, so P3 hydration + the store-1 filter reproduce
//!   the assembled prompt identically regardless of which backend served it.
//!
//! ## Key scheme (design §D5)
//!
//! Keyed on the canonical `"{channel}:{chat_id}"`, which is exactly how the
//! existing files are already named — so legacy files need **no rename**.
//!
//! ## Concurrency
//!
//! [`lock_conversation`](LocalFileSink::lock_conversation) is a no-op: the
//! in-process CLI drives a single turn at a time. The real per-conv mutex lives
//! in `clawft-service-agent::AgentService` for the daemon path, mirroring
//! [`InMemorySink`](super::sink::InMemorySink).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use percent_encoding::percent_decode_str;
use tracing::warn;

use clawft_platform::Platform;
use clawft_types::error::ClawftError;
use clawft_types::session::Session;

use super::sink::{ConversationSink, Turn};
use crate::security::validate_session_id;
use crate::session::{discover_sessions_dir, session_file_path, session_from_jsonl};

/// `ConversationSink` persisting to `~/.clawft/workspace/sessions/*.jsonl`.
///
/// See the [module docs](self) for the file format and the M3 rationale.
pub struct LocalFileSink<P: Platform> {
    /// Directory holding the `{encoded-key}.jsonl` files.
    sessions_dir: PathBuf,
    /// Platform providing filesystem access (native / browser / mock).
    platform: Arc<P>,
}

impl<P: Platform> LocalFileSink<P> {
    /// Create a sink rooted at the discovered sessions directory.
    ///
    /// Resolves the directory exactly as `SessionManager::new` does
    /// (`~/.clawft/workspace/sessions/`, legacy `~/.nanobot/...` fallback,
    /// created if absent) so the two share one location.
    pub async fn new(platform: Arc<P>) -> clawft_types::Result<Self> {
        let sessions_dir = discover_sessions_dir(platform.as_ref()).await?;
        Ok(Self {
            sessions_dir,
            platform,
        })
    }

    /// Create a sink with an explicit sessions directory (tests / known path).
    pub fn with_dir(platform: Arc<P>, sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            platform,
        }
    }

    /// The sessions directory backing this sink.
    pub fn sessions_dir(&self) -> &PathBuf {
        &self.sessions_dir
    }

    /// Validate `conv_id` and resolve its on-disk `.jsonl` path.
    ///
    /// Rejects path-traversal / separator / control-char keys at the boundary
    /// (mirrors `SessionManager`); the percent-encode in `session_file_path`
    /// is a second line of defence.
    fn resolve(&self, conv_id: &str) -> Result<PathBuf, String> {
        validate_session_id(conv_id).map_err(|e| e.to_string())?;
        Ok(session_file_path(&self.sessions_dir, conv_id))
    }

    /// Read every message line for `conv_id` into [`Turn`]s (the superset).
    ///
    /// The first line is the metadata header and is skipped. Malformed message
    /// lines are dropped with a warn (matching `SessionManager::load_session`).
    /// A missing file is an empty conversation, not an error. `turn_id` is
    /// synthesised from the message position — the JSONL format carries no
    /// per-turn id and P3's filter keys off `role` / `tool_calls` /
    /// `tool_call_id`, never `turn_id`.
    async fn read_turns(&self, conv_id: &str) -> Result<Vec<Turn>, String> {
        let path = self.resolve(conv_id)?;
        if !self.platform.fs().exists(&path).await {
            return Ok(Vec::new());
        }
        let content = self
            .platform
            .fs()
            .read_to_string(&path)
            .await
            .map_err(|e| format!("read {}: {e}", path.display()))?;

        let mut turns = Vec::new();
        // Line 1 is the metadata header; messages start at line 2.
        for line in content.lines().skip(1) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(msg) => {
                    if let Some(turn) = message_to_turn(&msg, conv_id, turns.len()) {
                        turns.push(turn);
                    } else {
                        warn!(
                            conv = conv_id,
                            "skipping session message line without a role"
                        );
                    }
                }
                Err(e) => warn!(
                    conv = conv_id,
                    error = %e,
                    "skipping malformed session message line"
                ),
            }
        }
        Ok(turns)
    }

    /// Read the metadata header object, or `None` if absent/unreadable.
    async fn read_header(&self, path: &std::path::Path) -> Option<serde_json::Value> {
        if !self.platform.fs().exists(path).await {
            return None;
        }
        let content = self.platform.fs().read_to_string(path).await.ok()?;
        let first = content.lines().next()?;
        serde_json::from_str::<serde_json::Value>(first).ok()
    }

    // ── Session-management surface (the `weft sessions` CLI) ─────────────
    //
    // These inherent methods let the CLI list / inspect / delete the JSONL
    // files directly through this sink instead of `SessionManager`, which the
    // M3 store-collapse retires from the read path (design §P5). They read the
    // same directory and format `SessionManager` uses, via the shared helpers
    // (`session_file_path`, `session_from_jsonl`), so the CLI's output is
    // unchanged.

    /// List every conversation key backed by this sink.
    ///
    /// Enumerates `{encoded}.jsonl` files and percent-decodes each stem back
    /// to its `"{channel}:{chat_id}"` key — the inverse of
    /// [`session_file_path`]. Undecodable filenames are skipped with a warn.
    /// Keys are returned sorted for stable CLI output.
    pub async fn list_conversations(&self) -> clawft_types::Result<Vec<String>> {
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

    /// Load a full [`Session`] (header metadata + all message lines) for
    /// `conv_id`, decoded via the shared [`session_from_jsonl`] parser.
    ///
    /// Unlike [`history`](Self::history) (which yields the [`Turn`] superset
    /// for hydration), this returns the raw `Session` the CLI's `inspect`
    /// renders — key, timestamps, metadata, `last_consolidated`, and the
    /// message objects verbatim. Errors if the file is missing or its header
    /// line is unparseable.
    pub async fn load_session(&self, conv_id: &str) -> clawft_types::Result<Session> {
        validate_session_id(conv_id)?;
        let path = session_file_path(&self.sessions_dir, conv_id);
        let content = self.platform.fs().read_to_string(&path).await?;
        session_from_jsonl(conv_id, &content, &path)
    }

    /// Delete the on-disk `.jsonl` file for `conv_id`.
    ///
    /// A missing file is a no-op (idempotent delete). Unlike
    /// `SessionManager::delete_session` there is no in-memory cache to evict
    /// and no chain-event marker — the in-process CLI has no live chain writer
    /// to receive one.
    pub async fn delete(&self, conv_id: &str) -> clawft_types::Result<()> {
        validate_session_id(conv_id)?;
        let path = session_file_path(&self.sessions_dir, conv_id);
        if self.platform.fs().exists(&path).await {
            self.platform
                .fs()
                .remove_file(&path)
                .await
                .map_err(ClawftError::Io)?;
        }
        crate::observation_pack::remove_observations_dir(
            self.platform.as_ref(),
            &crate::observation_pack::observations_dir(&self.sessions_dir, conv_id),
        )
        .await;
        Ok(())
    }
}

/// Serialise a [`Turn`] to a session message line (`role` / `content` /
/// `timestamp`, plus `tool_calls` / `tool_call_id` when present).
///
/// Shape matches `SessionManager` / `Session::add_message` so the two writers
/// stay format-compatible.
fn turn_to_message_line(turn: &Turn) -> String {
    let ts = DateTime::<Utc>::from_timestamp_millis(turn.ts_ms as i64)
        .unwrap_or_else(Utc::now)
        .to_rfc3339();
    let mut msg = serde_json::json!({
        "role": turn.role,
        "content": turn.content,
        "timestamp": ts,
    });
    if let Some(obj) = msg.as_object_mut() {
        if let Some(tool_calls) = &turn.tool_calls {
            obj.insert("tool_calls".into(), serde_json::Value::Array(tool_calls.clone()));
        }
        if let Some(tool_call_id) = &turn.tool_call_id {
            obj.insert(
                "tool_call_id".into(),
                serde_json::Value::String(tool_call_id.clone()),
            );
        }
    }
    let mut line = serde_json::to_string(&msg).unwrap_or_default();
    line.push('\n');
    line
}

/// Parse a session message object into a [`Turn`], or `None` if it has no role
/// (e.g. a stray header line).
fn message_to_turn(msg: &serde_json::Value, conv_id: &str, index: usize) -> Option<Turn> {
    let role = msg.get("role").and_then(|v| v.as_str())?;
    let content = msg
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let ts_ms = msg
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp_millis().max(0) as u64)
        .unwrap_or(0);
    let tool_calls = msg
        .get("tool_calls")
        .filter(|v| !v.is_null())
        .and_then(|v| v.as_array())
        .map(|a| a.to_vec());
    let tool_call_id = msg
        .get("tool_call_id")
        .filter(|v| !v.is_null())
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some(Turn {
        turn_id: format!("{conv_id}#{index}"),
        role: role.to_string(),
        content,
        tool_calls,
        tool_call_id,
        ts_ms,
        voice_analysis: None,
            audio: None,
    })
}

/// Build a fresh metadata header line (design §D3 sidecar home).
fn fresh_header(metadata: &serde_json::Value) -> String {
    let now = Utc::now().to_rfc3339();
    let header = serde_json::json!({
        "_type": "metadata",
        "created_at": now,
        "updated_at": now,
        "metadata": metadata,
        "last_consolidated": 0,
    });
    serde_json::to_string(&header).unwrap_or_default()
}

// Browser platform FS futures are `!Send` (see `clawft_platform::FileSystem`);
// match `ConversationSink`'s browser `?Send` relaxation so LocalFileSink
// compiles under `wasm32-unknown-unknown` without weakening native Send.
#[cfg_attr(not(feature = "browser"), async_trait)]
#[cfg_attr(feature = "browser", async_trait(?Send))]
impl<P: Platform + 'static> ConversationSink for LocalFileSink<P> {
    async fn lock_conversation(&self, _conv_id: &str) {
        // In-process CLI runs one turn at a time; the real per-conv mutex is
        // in clawft-service-agent for the daemon path.
    }

    async fn append_turn(&self, conv_id: &str, turn: Turn) -> Result<(), String> {
        let path = self.resolve(conv_id)?;
        let line = turn_to_message_line(&turn);

        if self.platform.fs().exists(&path).await {
            // Existing conversation: append the message line, leaving the
            // metadata header untouched — same behaviour as
            // SessionManager::append_turn.
            self.platform
                .fs()
                .append_string(&path, &line)
                .await
                .map_err(|e| format!("append {}: {e}", path.display()))
        } else {
            // First turn: seed the file with a metadata header, then the line.
            let mut content = fresh_header(&serde_json::json!({}));
            content.push('\n');
            content.push_str(&line);
            self.platform
                .fs()
                .write_string(&path, &content)
                .await
                .map_err(|e| format!("write {}: {e}", path.display()))
        }
    }

    async fn publish_status(
        &self,
        _conv_id: &str,
        _status: &str,
        _payload: serde_json::Value,
    ) -> Result<(), String> {
        // No substrate topic in-process; status heartbeats are a daemon-only
        // concern. Drop on the floor (as InMemorySink does).
        Ok(())
    }

    async fn load_history(&self, conv_id: &str) -> Result<Vec<Turn>, String> {
        self.read_turns(conv_id).await
    }

    async fn history(&self, conv_id: &str, window: usize) -> Vec<Turn> {
        // Infallible by contract (design §6): degrade to empty on any error.
        let all = match self.read_turns(conv_id).await {
            Ok(turns) => turns,
            Err(e) => {
                warn!(conv = conv_id, error = %e, "history read failed; degrading to empty");
                return Vec::new();
            }
        };
        // Tail-truncate to the most recent `window` turns; `0` means all.
        // The window is applied to the *superset* — see the P1 contract on
        // `ConversationSink::history` (P3 must window post-filter or pass 0).
        if window == 0 || all.len() <= window {
            all
        } else {
            all[all.len() - window..].to_vec()
        }
    }

    async fn meta(&self, conv_id: &str) -> serde_json::Value {
        let path = match self.resolve(conv_id) {
            Ok(p) => p,
            Err(_) => return serde_json::Value::Null,
        };
        match self.read_header(&path).await {
            Some(header) => header
                .get("metadata")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            None => serde_json::Value::Null,
        }
    }

    async fn set_meta(&self, conv_id: &str, meta: serde_json::Value) {
        let path = match self.resolve(conv_id) {
            Ok(p) => p,
            Err(e) => {
                warn!(conv = conv_id, error = %e, "set_meta rejected invalid conv id");
                return;
            }
        };

        // Rebuild the header with the new metadata while preserving every
        // message line verbatim (no reserialisation drift).
        let existing = self.platform.fs().read_to_string(&path).await.ok();
        let (header_line, message_lines): (String, Vec<String>) = match existing {
            Some(content) => {
                let mut lines = content.lines();
                let header = match lines.next().and_then(|l| {
                    serde_json::from_str::<serde_json::Value>(l).ok()
                }) {
                    Some(mut h) => {
                        if let Some(obj) = h.as_object_mut() {
                            obj.insert("metadata".into(), meta.clone());
                            obj.insert(
                                "updated_at".into(),
                                serde_json::Value::String(Utc::now().to_rfc3339()),
                            );
                        }
                        serde_json::to_string(&h).unwrap_or_else(|_| fresh_header(&meta))
                    }
                    // File present but header unparseable: start a clean header
                    // and keep whatever message lines exist below it.
                    None => fresh_header(&meta),
                };
                let msgs = lines
                    .filter(|l| !l.trim().is_empty())
                    .map(str::to_string)
                    .collect();
                (header, msgs)
            }
            None => (fresh_header(&meta), Vec::new()),
        };

        let mut out = header_line;
        out.push('\n');
        for l in message_lines {
            out.push_str(&l);
            out.push('\n');
        }
        if let Err(e) = self.platform.fs().write_string(&path, &out).await {
            warn!(conv = conv_id, error = %e, "set_meta write failed; swallowed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_platform::NativePlatform;
    use serde_json::json;
    use tempfile::TempDir;

    /// A `LocalFileSink` over a real temp dir (native fs).
    fn temp_sink() -> (TempDir, LocalFileSink<NativePlatform>) {
        let dir = TempDir::new().unwrap();
        let sink = LocalFileSink::with_dir(
            Arc::new(NativePlatform::new()),
            dir.path().to_path_buf(),
        );
        (dir, sink)
    }

    fn turn(role: &str, content: &str) -> Turn {
        Turn {
            turn_id: format!("t-{role}"),
            role: role.into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            ts_ms: 1_700_000_000_000,
            voice_analysis: None,
            audio: None,
        }
    }

    #[tokio::test]
    async fn append_then_history_round_trips() {
        let (_dir, sink) = temp_sink();
        sink.append_turn("telegram:1", turn("user", "hello"))
            .await
            .unwrap();
        sink.append_turn("telegram:1", turn("assistant", "hi there"))
            .await
            .unwrap();

        let hist = sink.history("telegram:1", 0).await;
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].role, "user");
        assert_eq!(hist[0].content, "hello");
        assert_eq!(hist[1].role, "assistant");
        assert_eq!(hist[1].content, "hi there");
    }

    #[tokio::test]
    async fn history_of_unknown_conv_is_empty() {
        let (_dir, sink) = temp_sink();
        assert!(sink.history("nope:0", 0).await.is_empty());
        assert!(sink.load_history("nope:0").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn history_windows_to_the_tail_of_the_superset() {
        let (_dir, sink) = temp_sink();
        for i in 0..5 {
            sink.append_turn("c:1", turn("user", &i.to_string()))
                .await
                .unwrap();
        }
        let all = sink.history("c:1", 0).await;
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].content, "0");
        assert_eq!(all[4].content, "4");

        let tail = sink.history("c:1", 2).await;
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].content, "3");
        assert_eq!(tail[1].content, "4");

        // Window larger than history → all of it, no panic.
        assert_eq!(sink.history("c:1", 100).await.len(), 5);
    }

    #[tokio::test]
    async fn tool_intermediates_survive_the_superset_round_trip() {
        // The superset must retain tool-call + tool-result turns so P3's
        // hydration filter has the full record to reproduce store-1 from.
        let (_dir, sink) = temp_sink();
        sink.append_turn("c:1", turn("user", "q")).await.unwrap();
        let tool_call = Turn {
            tool_calls: Some(vec![json!({"id": "call-1", "name": "search"})]),
            ..turn("assistant", "calling")
        };
        sink.append_turn("c:1", tool_call).await.unwrap();
        let tool_result = Turn {
            tool_call_id: Some("call-1".into()),
            ..turn("tool", "result")
        };
        sink.append_turn("c:1", tool_result).await.unwrap();
        sink.append_turn("c:1", turn("assistant", "final"))
            .await
            .unwrap();

        let turns = sink.history("c:1", 0).await;
        assert_eq!(turns.len(), 4, "superset must keep tool intermediates");
        assert!(turns[1].tool_calls.is_some());
        assert_eq!(turns[1].tool_calls.as_ref().unwrap().len(), 1);
        assert_eq!(turns[2].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(turns[3].content, "final");
    }

    #[tokio::test]
    async fn meta_round_trips_and_defaults_to_null() {
        let (_dir, sink) = temp_sink();
        // Unknown conv → Null (caller defaults hallucination score to 0.0).
        assert_eq!(sink.meta("c:1").await, serde_json::Value::Null);

        sink.set_meta("c:1", json!({"hallucination_score": 0.42}))
            .await;
        assert_eq!(sink.meta("c:1").await["hallucination_score"], 0.42);

        // Overwrite in place.
        sink.set_meta("c:1", json!({"hallucination_score": 0.99}))
            .await;
        assert_eq!(sink.meta("c:1").await["hallucination_score"], 0.99);

        // Isolated per conversation.
        assert_eq!(sink.meta("other:1").await, serde_json::Value::Null);
    }

    #[tokio::test]
    async fn set_meta_preserves_existing_message_lines() {
        let (_dir, sink) = temp_sink();
        sink.append_turn("c:1", turn("user", "hello"))
            .await
            .unwrap();
        sink.append_turn("c:1", turn("assistant", "hi"))
            .await
            .unwrap();

        sink.set_meta("c:1", json!({"hallucination_score": 0.5}))
            .await;

        // Metadata landed…
        assert_eq!(sink.meta("c:1").await["hallucination_score"], 0.5);
        // …and the messages are still there, in order.
        let hist = sink.history("c:1", 0).await;
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].content, "hello");
        assert_eq!(hist[1].content, "hi");
    }

    #[tokio::test]
    async fn set_meta_before_any_turn_then_append() {
        // set_meta on a fresh conv creates the file with a header only; a later
        // append must still land as a message line and history must read both.
        let (_dir, sink) = temp_sink();
        sink.set_meta("c:1", json!({"hallucination_score": 0.1}))
            .await;
        assert!(sink.history("c:1", 0).await.is_empty());
        sink.append_turn("c:1", turn("user", "hi")).await.unwrap();

        assert_eq!(sink.meta("c:1").await["hallucination_score"], 0.1);
        let hist = sink.history("c:1", 0).await;
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].content, "hi");
    }

    #[tokio::test]
    async fn invalid_conv_id_is_rejected_or_degrades() {
        let (_dir, sink) = temp_sink();
        // Path traversal / separators rejected at the boundary.
        assert!(sink.append_turn("../evil", turn("user", "x")).await.is_err());
        assert!(sink.load_history("../evil").await.is_err());
        // Read-side helpers degrade rather than panic.
        assert!(sink.history("../evil", 0).await.is_empty());
        assert_eq!(sink.meta("../evil").await, serde_json::Value::Null);
        sink.set_meta("../evil", json!({"x": 1})).await; // no panic
    }

    #[tokio::test]
    async fn keys_with_same_chat_id_across_channels_are_distinct() {
        // Design §D5: canonical key {channel}:{chat_id} — no cross-channel
        // collision even when the chat_id matches.
        let (_dir, sink) = temp_sink();
        sink.append_turn("telegram:42", turn("user", "from telegram"))
            .await
            .unwrap();
        sink.append_turn("discord:42", turn("user", "from discord"))
            .await
            .unwrap();

        let tg = sink.history("telegram:42", 0).await;
        let dc = sink.history("discord:42", 0).await;
        assert_eq!(tg.len(), 1);
        assert_eq!(dc.len(), 1);
        assert_eq!(tg[0].content, "from telegram");
        assert_eq!(dc[0].content, "from discord");
    }

    /// Golden-fixture compatibility: a file written in the EXACT format
    /// `SessionManager::save_session` produces must read back cleanly.
    ///
    /// The header + message lines below are byte-for-byte the shape
    /// `session.rs` emits (metadata header, then `{role, content, timestamp}`
    /// message objects, one with `tool_calls`, one with `tool_call_id`).
    #[tokio::test]
    async fn reads_legacy_session_manager_format() {
        let (dir, sink) = temp_sink();
        // key "telegram:99" → percent-encoded filename, matching
        // SessionManager::session_path.
        let path = session_file_path(dir.path(), "telegram:99");

        let legacy = concat!(
            r#"{"_type":"metadata","created_at":"2026-01-01T00:00:00+00:00","updated_at":"2026-01-01T00:05:00+00:00","metadata":{"hallucination_score":0.25},"last_consolidated":0}"#, "\n",
            r#"{"role":"user","content":"what's the weather?","timestamp":"2026-01-01T00:00:01+00:00"}"#, "\n",
            r#"{"role":"assistant","content":"checking","timestamp":"2026-01-01T00:00:02+00:00","tool_calls":[{"id":"call-1","name":"weather"}]}"#, "\n",
            r#"{"role":"tool","content":"sunny, 22C","timestamp":"2026-01-01T00:00:03+00:00","tool_call_id":"call-1"}"#, "\n",
            r#"{"role":"assistant","content":"It's sunny and 22C.","timestamp":"2026-01-01T00:00:04+00:00"}"#, "\n",
        );
        std::fs::write(&path, legacy).unwrap();

        // Superset history: all four message turns, intermediates intact.
        let hist = sink.history("telegram:99", 0).await;
        assert_eq!(hist.len(), 4);
        assert_eq!(hist[0].role, "user");
        assert_eq!(hist[0].content, "what's the weather?");
        assert!(hist[1].tool_calls.is_some());
        assert_eq!(hist[2].role, "tool");
        assert_eq!(hist[2].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(hist[3].content, "It's sunny and 22C.");

        // Timestamps parsed from the RFC3339 strings.
        assert!(hist[0].ts_ms > 0);

        // Metadata sidecar read from the legacy header.
        assert_eq!(sink.meta("telegram:99").await["hallucination_score"], 0.25);
    }

    /// A file authored by `LocalFileSink` must itself be readable back by
    /// `SessionManager` (format is a two-way contract). We assert the on-disk
    /// bytes parse as the same header+message line shape SessionManager expects.
    #[tokio::test]
    async fn written_file_matches_session_manager_shape() {
        let (dir, sink) = temp_sink();
        sink.set_meta("c:1", json!({"hallucination_score": 0.7}))
            .await;
        sink.append_turn("c:1", turn("user", "hi")).await.unwrap();

        let path = session_file_path(dir.path(), "c:1");
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines = content.lines();

        // Line 1: metadata header with the expected fields.
        let header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(header["_type"], "metadata");
        assert!(header.get("created_at").is_some());
        assert!(header.get("updated_at").is_some());
        assert_eq!(header["metadata"]["hallucination_score"], 0.7);
        assert_eq!(header["last_consolidated"], 0);

        // Line 2: message with role/content/timestamp.
        let msg: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(msg["role"], "user");
        assert_eq!(msg["content"], "hi");
        assert!(msg.get("timestamp").is_some());

        // No stray trailing content.
        assert!(lines.next().is_none());
    }

    // ── CLI surface: list_conversations / load_session / delete ─────────

    #[tokio::test]
    async fn list_conversations_decodes_and_sorts_keys() {
        let (_dir, sink) = temp_sink();
        // Two channels sharing a chat id, plus a key with special chars — all
        // must round-trip through the percent-encoded filenames.
        sink.append_turn("telegram:42", turn("user", "a"))
            .await
            .unwrap();
        sink.append_turn("discord:42", turn("user", "b"))
            .await
            .unwrap();
        sink.append_turn("slack:channel:thread", turn("user", "c"))
            .await
            .unwrap();

        let keys = sink.list_conversations().await.unwrap();
        assert_eq!(
            keys,
            vec![
                "discord:42".to_string(),
                "slack:channel:thread".to_string(),
                "telegram:42".to_string(),
            ],
            "keys must be decoded and sorted"
        );
    }

    #[tokio::test]
    async fn list_conversations_empty_dir_is_empty() {
        let (_dir, sink) = temp_sink();
        assert!(sink.list_conversations().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn load_session_returns_header_and_messages() {
        let (_dir, sink) = temp_sink();
        sink.set_meta("telegram:7", json!({"hallucination_score": 0.3}))
            .await;
        sink.append_turn("telegram:7", turn("user", "hi"))
            .await
            .unwrap();
        sink.append_turn("telegram:7", turn("assistant", "hello"))
            .await
            .unwrap();

        let session = sink.load_session("telegram:7").await.unwrap();
        assert_eq!(session.key, "telegram:7");
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0]["role"], "user");
        assert_eq!(session.messages[0]["content"], "hi");
        assert_eq!(session.messages[1]["content"], "hello");
        assert_eq!(session.metadata["hallucination_score"], 0.3);
        assert_eq!(session.last_consolidated, 0);
    }

    #[tokio::test]
    async fn load_session_missing_is_error() {
        let (_dir, sink) = temp_sink();
        assert!(sink.load_session("nope:0").await.is_err());
    }

    #[tokio::test]
    async fn delete_removes_file_and_is_idempotent() {
        let (_dir, sink) = temp_sink();
        sink.append_turn("telegram:9", turn("user", "x"))
            .await
            .unwrap();
        assert_eq!(sink.list_conversations().await.unwrap().len(), 1);

        sink.delete("telegram:9").await.unwrap();
        assert!(sink.list_conversations().await.unwrap().is_empty());
        // Deleting an already-absent conversation is a no-op, not an error.
        sink.delete("telegram:9").await.unwrap();
    }

    #[tokio::test]
    async fn delete_rejects_invalid_conv_id() {
        let (_dir, sink) = temp_sink();
        assert!(sink.delete("../evil").await.is_err());
    }
}
