//! ObservationPack: archive large tool results and project a head/tail handle.
//!
//! Prompt-facing bodies stay under [`MAX_PROMPT_BYTES`]. The original bytes
//! live next to the session JSONL so later turns can page them via
//! `obs_recall`. Fail-open: archive errors fall through to
//! [`crate::security::truncate_result`]. Native-only on disk; WASM keeps
//! today's truncate path.
//!
//! Spec: `docs/design/observation-pack.md`. ECC: [`IMPULSE_ARCHIVED`] /
//! [`IMPULSE_RECALLED`] (`ImpulseType::Custom`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tracing::warn;

use crate::security::truncate_result;

/// Prompt-budget cap (CRIT-02). Same value as `MAX_TOOL_RESULT_BYTES`.
pub const MAX_PROMPT_BYTES: usize = 65_536;

/// V2 head budget in bytes (then floored to a UTF-8 boundary).
pub const HEAD_BYTES: usize = 2048;

/// V2 tail budget in bytes (then floored to a UTF-8 boundary).
pub const TAIL_BYTES: usize = 1536;

/// Combined head+tail; results at or below this are not archived.
pub const HEAD_TAIL_BUDGET: usize = HEAD_BYTES + TAIL_BYTES;

/// Default `obs_recall` page size.
pub const RECALL_DEFAULT_LIMIT: u32 = 8192;

/// Maximum `obs_recall` page size.
pub const RECALL_MAX_LIMIT: u32 = 32_768;

/// Full bodies returned for this many archived sends per tool+session.
pub const FULL_SENDS_BEFORE_PROJECT: u32 = 2;

/// `ImpulseType::Custom` — observation archived.
pub const IMPULSE_ARCHIVED: u8 = 0x70;

/// `ImpulseType::Custom` — observation recalled.
pub const IMPULSE_RECALLED: u8 = 0x71;

#[cfg(feature = "native")]
const RECALL_HINT: &str =
    "Use obs_recall with this id and offset/limit to page the archive. Do not re-run the tool to re-read this output.";



/// One `ledger.jsonl` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerRow {
    pub id: String,
    pub sha256: String,
    pub tool: String,
    pub tool_call_id: String,
    pub bytes: u64,
    pub created_ms: u64,
    pub full_sends: u32,
}

/// Session archive context for `obs_recall` (task-local, native).
#[derive(Debug, Clone)]
pub struct ObservationContext {
    pub sessions_dir: PathBuf,
    pub session_key: String,
}

#[cfg(feature = "native")]
tokio::task_local! {
    static OBSERVATION_CONTEXT: ObservationContext;
}

impl ObservationContext {
    /// Run `fut` with this session archive context installed.
    #[cfg(feature = "native")]
    pub async fn scope<F>(self, fut: F) -> F::Output
    where
        F: std::future::Future,
    {
        OBSERVATION_CONTEXT.scope(self, fut).await
    }

    /// Ambient archive context, or `None` outside a [`Self::scope`].
    #[cfg(feature = "native")]
    pub fn current() -> Option<Self> {
        OBSERVATION_CONTEXT.try_with(|c| c.clone()).ok()
    }

    #[cfg(not(feature = "native"))]
    pub fn current() -> Option<Self> {
        None
    }
}

/// `{sessions_dir}/{percent-encoded-key}.observations/`
pub fn observations_dir(sessions_dir: &Path, key: &str) -> PathBuf {
    let stem = crate::session::session_key_stem(key);
    sessions_dir.join(format!("{stem}.observations"))
}

#[cfg(feature = "native")]
fn ledger_path(dir: &Path) -> PathBuf {
    dir.join("ledger.jsonl")
}

#[cfg(feature = "native")]
fn blob_path(dir: &Path, sha256: &str) -> PathBuf {
    dir.join(format!("{sha256}.bin"))
}

/// Floor `max_bytes` to a char boundary (never split a codepoint).
pub fn utf8_prefix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Last `max_bytes` floored to a char boundary.
pub fn utf8_suffix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut start = s.len() - max_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

#[cfg(feature = "native")]
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(feature = "native")]
fn new_obs_id() -> String {
    format!("obs_{}", uuid::Uuid::new_v4().simple())
}

/// `id` is `obs_` + `[A-Za-z0-9_]+`. Rejects path traversal by charset.
pub fn parse_obs_id(id: &str) -> Result<&str, RecallError> {
    let rest = id
        .strip_prefix("obs_")
        .ok_or(RecallError::InvalidId)?;
    if rest.is_empty()
        || !rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(RecallError::InvalidId);
    }
    Ok(id)
}

#[cfg(feature = "native")]
fn valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(feature = "native")]
fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Errors from [`recall`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecallError {
    #[error("invalid observation id")]
    InvalidId,
    #[error("observation not found in this session")]
    NotFound,
    #[error("archive unavailable")]
    Unavailable,
    #[error("archive read failed: {0}")]
    Io(String),
}

/// Archive `value` when it exceeds the tiny-result threshold; return either
/// the original, a projection object, or a truncated fail-open body.
#[cfg(feature = "native")]
pub async fn pack_tool_result(
    sessions_dir: &Path,
    session_key: &str,
    tool: &str,
    tool_call_id: &str,
    value: Value,
    max_prompt_bytes: usize,
) -> Value {
    if session_key.is_empty() {
        return truncate_result(value, max_prompt_bytes);
    }
    if crate::security::validate_session_id(session_key).is_err() {
        return truncate_result(value, max_prompt_bytes);
    }

    let serialized = match serde_json::to_string(&value) {
        Ok(s) => s,
        Err(_) => return truncate_result(value, max_prompt_bytes),
    };

    if serialized.len() <= max_prompt_bytes && serialized.len() <= HEAD_TAIL_BUDGET {
        return value;
    }

    match archive_serialized(
        sessions_dir,
        session_key,
        tool,
        tool_call_id,
        &serialized,
    )
    .await
    {
        Ok(row) => {
            emit_impulse(
                crate::chain_event::EVENT_KIND_OBSERVATION_ARCHIVED,
                IMPULSE_ARCHIVED,
                session_key,
                &row.id,
                row.bytes,
            );
            if row.full_sends <= FULL_SENDS_BEFORE_PROJECT
                && serialized.len() <= max_prompt_bytes
            {
                value
            } else {
                project_row(&row, &serialized, max_prompt_bytes)
            }
        }
        Err(e) => {
            warn!(
                error = %e,
                tool,
                session = %session_key,
                "observation pack: archive failed; using truncate_result"
            );
            truncate_result(value, max_prompt_bytes)
        }
    }
}

#[cfg(not(feature = "native"))]
pub async fn pack_tool_result(
    _sessions_dir: &Path,
    _session_key: &str,
    _tool: &str,
    _tool_call_id: &str,
    value: Value,
    max_prompt_bytes: usize,
) -> Value {
    truncate_result(value, max_prompt_bytes)
}

#[cfg(feature = "native")]
async fn archive_serialized(
    sessions_dir: &Path,
    session_key: &str,
    tool: &str,
    tool_call_id: &str,
    serialized: &str,
) -> Result<LedgerRow, String> {
    let dir = observations_dir(sessions_dir, session_key);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;

    let digest = sha256_hex(serialized.as_bytes());
    let blob = blob_path(&dir, &digest);
    if !blob.exists() {
        tokio::fs::write(&blob, serialized.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
    }

    let ledger = ledger_path(&dir);
    let prior = count_tool_sends(&ledger, tool).await;
    let full_sends = prior + 1;
    let row = LedgerRow {
        id: new_obs_id(),
        sha256: digest,
        tool: tool.to_string(),
        tool_call_id: tool_call_id.to_string(),
        bytes: serialized.len() as u64,
        created_ms: now_ms(),
        full_sends,
    };
    let mut line = serde_json::to_string(&row).map_err(|e| e.to_string())?;
    line.push('\n');
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger)
        .await
        .map_err(|e| e.to_string())?;
    use tokio::io::AsyncWriteExt;
    file.write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    file.flush().await.map_err(|e| e.to_string())?;
    Ok(row)
}

#[cfg(feature = "native")]
async fn count_tool_sends(ledger: &Path, tool: &str) -> u32 {
    let Ok(content) = tokio::fs::read_to_string(ledger).await else {
        return 0;
    };
    content
        .lines()
        .filter_map(|l| serde_json::from_str::<LedgerRow>(l).ok())
        .filter(|r| r.tool == tool)
        .count() as u32
}

#[cfg(feature = "native")]
fn project_row(row: &LedgerRow, serialized: &str, max_prompt_bytes: usize) -> Value {
    let mut head_budget = HEAD_BYTES;
    let mut tail_budget = TAIL_BYTES;
    loop {
        let head = utf8_prefix(serialized, head_budget);
        let tail = utf8_suffix(serialized, tail_budget);
        let proj = serde_json::json!({
            "observation_pack": {
                "id": row.id,
                "sha256": row.sha256,
                "bytes": row.bytes,
                "tool": row.tool,
                "head": head,
                "tail": tail,
                "recall": "obs_recall",
                "hint": RECALL_HINT,
            }
        });
        let ser = serde_json::to_string(&proj).unwrap_or_default();
        if ser.len() <= max_prompt_bytes {
            return proj;
        }
        if head_budget == 0 && tail_budget == 0 {
            return truncate_result(proj, max_prompt_bytes);
        }
        head_budget /= 2;
        tail_budget /= 2;
    }
}

#[cfg(feature = "native")]
fn emit_impulse(kind: &str, code: u8, session_key: &str, obs_id: &str, bytes: u64) {
    crate::chain_event!(
        "observation",
        kind,
        {
            impulse_code: code,
            session: session_key,
            id: obs_id,
            bytes: bytes
        }
    );
}

/// Page a SHA blob from **this session's** ledger only.
#[cfg(feature = "native")]
pub async fn recall(
    sessions_dir: &Path,
    session_key: &str,
    id: &str,
    offset: u64,
    limit: u32,
) -> Result<Value, RecallError> {
    let id = parse_obs_id(id)?;
    if crate::security::validate_session_id(session_key).is_err() {
        return Err(RecallError::InvalidId);
    }
    let limit = limit.min(RECALL_MAX_LIMIT);
    let dir = observations_dir(sessions_dir, session_key);
    let ledger = ledger_path(&dir);
    let content = tokio::fs::read_to_string(&ledger)
        .await
        .map_err(|_| RecallError::NotFound)?;
    let row = content
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<LedgerRow>(l).ok())
        .find(|r| r.id == id)
        .ok_or(RecallError::NotFound)?;
    if !valid_sha256_hex(&row.sha256) {
        return Err(RecallError::NotFound);
    }
    let blob = blob_path(&dir, &row.sha256);
    let total = tokio::fs::metadata(&blob)
        .await
        .map_err(|e| RecallError::Io(e.to_string()))?
        .len();
    if offset >= total {
        emit_impulse(
            crate::chain_event::EVENT_KIND_OBSERVATION_RECALLED,
            IMPULSE_RECALLED,
            session_key,
            id,
            total,
        );
        return Ok(serde_json::json!({
            "id": id,
            "offset": offset,
            "bytes": total,
            "eof": true,
            "chunk": "",
        }));
    }
    let to_read = (limit as u64).min(total - offset) as usize;
    let mut file = tokio::fs::File::open(&blob)
        .await
        .map_err(|e| RecallError::Io(e.to_string()))?;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| RecallError::Io(e.to_string()))?;
    let mut buf = vec![0u8; to_read];
    file.read_exact(&mut buf)
        .await
        .map_err(|e| RecallError::Io(e.to_string()))?;
    let chunk = utf8_window(&buf);
    let end = offset + to_read as u64;
    emit_impulse(
        crate::chain_event::EVENT_KIND_OBSERVATION_RECALLED,
        IMPULSE_RECALLED,
        session_key,
        id,
        total,
    );
    Ok(serde_json::json!({
        "id": id,
        "offset": offset,
        "bytes": total,
        "eof": end >= total,
        "chunk": chunk,
    }))
}

#[cfg(not(feature = "native"))]
pub async fn recall(
    _sessions_dir: &Path,
    _session_key: &str,
    _id: &str,
    _offset: u64,
    _limit: u32,
) -> Result<Value, RecallError> {
    Err(RecallError::Unavailable)
}

/// Drop incomplete leading/trailing UTF-8 sequences in a byte window.
#[cfg(feature = "native")]
fn utf8_window(buf: &[u8]) -> String {
    let mut start = 0;
    while start < buf.len() && (buf[start] & 0b1100_0000) == 0b1000_0000 {
        start += 1;
    }
    let mut end = buf.len();
    while end > start && (buf[end - 1] & 0b1100_0000) == 0b1000_0000 {
        end -= 1;
    }
    if end > start && (buf[end - 1] & 0b1000_0000) != 0 {
        // Last remaining byte starts a multi-byte sequence that was cut.
        let mut i = end - 1;
        while i > start && (buf[i] & 0b1100_0000) == 0b1000_0000 {
            i -= 1;
        }
        let need = match buf[i] {
            b if b & 0b1111_1000 == 0b1111_0000 => 4,
            b if b & 0b1111_0000 == 0b1110_0000 => 3,
            b if b & 0b1110_0000 == 0b1100_0000 => 2,
            _ => 1,
        };
        if i + need > end {
            end = i;
        }
    }
    String::from_utf8_lossy(&buf[start..end]).into_owned()
}

/// Delete `{encoded}.observations/` (files via platform FS; dir via native).
pub async fn remove_observations_dir<P: clawft_platform::Platform>(
    platform: &P,
    dir: &Path,
) {
    if !platform.fs().exists(dir).await {
        return;
    }
    if let Ok(entries) = platform.fs().list_dir(dir).await {
        for entry in entries {
            if let Err(e) = platform.fs().remove_file(&entry).await {
                warn!(
                    path = %entry.display(),
                    error = %e,
                    "observation pack: failed to remove archive file"
                );
            }
        }
    }
    #[cfg(feature = "native")]
    if let Err(e) = tokio::fs::remove_dir_all(dir).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn!(
                path = %dir.display(),
                error = %e,
                "observation pack: failed to remove observations dir"
            );
        }
    }
}

/// Remove `.observations/` dirs whose sibling `.jsonl` is gone.
pub async fn gc_orphaned_observation_dirs<P: clawft_platform::Platform>(
    platform: &P,
    sessions_dir: &Path,
    dry_run: bool,
) -> Vec<String> {
    let Ok(entries) = platform.fs().list_dir(sessions_dir).await else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for entry in entries {
        let Some(name) = entry.file_name() else {
            continue;
        };
        let name = name.to_string_lossy();
        let Some(_stem) = name.strip_suffix(".observations") else {
            continue;
        };
        let jsonl = sessions_dir.join(format!(
            "{}.jsonl",
            name.trim_end_matches(".observations")
        ));
        if platform.fs().exists(&jsonl).await {
            continue;
        }
        if dry_run {
            removed.push(name.into_owned());
            continue;
        }
        remove_observations_dir(platform, &entry).await;
        removed.push(name.into_owned());
    }
    removed.sort();
    removed
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_sessions() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    #[test]
    fn utf8_prefix_does_not_split_codepoint() {
        let s = "é"; // 2 bytes
        assert_eq!(utf8_prefix(s, 1), "");
        assert_eq!(utf8_prefix(s, 2), "é");
    }

    #[test]
    fn utf8_suffix_does_not_split_codepoint() {
        let s = "aé";
        assert_eq!(utf8_suffix(s, 1), "");
        assert_eq!(utf8_suffix(s, 2), "é");
    }

    #[test]
    fn parse_obs_id_rejects_traversal() {
        assert!(parse_obs_id("../etc/passwd").is_err());
        assert!(parse_obs_id("obs_../x").is_err());
        assert!(parse_obs_id("obs_foo/bar").is_err());
        assert!(parse_obs_id("obs_foo.bar").is_err());
        assert!(parse_obs_id("obs_ok_id").is_ok());
        assert!(parse_obs_id("obs_").is_err());
    }

    #[tokio::test]
    async fn tiny_result_does_not_create_archive_dir() {
        let (_tmp, sessions) = temp_sessions();
        let val = json!({"ok": true, "msg": "short"});
        let out = pack_tool_result(&sessions, "t:1", "bash", "c1", val.clone(), MAX_PROMPT_BYTES)
            .await;
        assert_eq!(out, val);
        let obs = observations_dir(&sessions, "t:1");
        assert!(!obs.exists(), "tiny results must not create {obs:?}");
    }

    #[tokio::test]
    async fn ten_kb_archives_full_then_projects() {
        let (_tmp, sessions) = temp_sessions();
        let payload = "x".repeat(10_000);
        let val = json!({"data": payload});
        let ser_len = serde_json::to_string(&val).unwrap().len();
        assert!(ser_len > HEAD_TAIL_BUDGET);
        assert!(ser_len <= MAX_PROMPT_BYTES);

        let a = pack_tool_result(&sessions, "t:1", "bash", "c1", val.clone(), MAX_PROMPT_BYTES)
            .await;
        assert_eq!(a, val, "first send is full body");
        let b = pack_tool_result(&sessions, "t:1", "bash", "c2", val.clone(), MAX_PROMPT_BYTES)
            .await;
        assert_eq!(b, val, "second send is full body");
        let c = pack_tool_result(&sessions, "t:1", "bash", "c3", val.clone(), MAX_PROMPT_BYTES)
            .await;
        let pack = c
            .get("observation_pack")
            .expect("third send is a projection");
        assert_eq!(pack["tool"], "bash");
        assert_eq!(pack["recall"], "obs_recall");
        assert!(pack["id"].as_str().unwrap().starts_with("obs_"));
        assert_eq!(pack["bytes"], ser_len as u64);
        let head = pack["head"].as_str().unwrap();
        let tail = pack["tail"].as_str().unwrap();
        assert!(head.len() <= HEAD_BYTES);
        assert!(tail.len() <= TAIL_BYTES);
        let prompt = serde_json::to_string(&c).unwrap();
        assert!(prompt.len() <= MAX_PROMPT_BYTES);

        let obs = observations_dir(&sessions, "t:1");
        assert!(obs.join("ledger.jsonl").exists());
        let sha = pack["sha256"].as_str().unwrap();
        assert!(blob_path(&obs, sha).exists());
    }

    #[tokio::test]
    async fn recall_pages_and_sets_eof_on_last() {
        let (_tmp, sessions) = temp_sessions();
        let payload = "y".repeat(20_000);
        let val = json!({"data": payload});
        let packed =
            pack_tool_result(&sessions, "t:1", "bash", "c1", val, MAX_PROMPT_BYTES).await;
        // First send is full; still archived. Read id from ledger.
        let ledger = tokio::fs::read_to_string(ledger_path(&observations_dir(
            &sessions, "t:1",
        )))
        .await
        .unwrap();
        let row: LedgerRow = serde_json::from_str(ledger.lines().next().unwrap()).unwrap();
        let _ = packed;

        let page1 = recall(&sessions, "t:1", &row.id, 0, 8192).await.unwrap();
        assert_eq!(page1["eof"], false);
        assert_eq!(page1["chunk"].as_str().unwrap().len(), 8192);
        assert_eq!(page1["bytes"], row.bytes);

        let page2 = recall(&sessions, "t:1", &row.id, 8192, 8192).await.unwrap();
        assert_eq!(page2["eof"], false);

        let last_off = (row.bytes / 8192) * 8192;
        let last = recall(&sessions, "t:1", &row.id, last_off, 8192)
            .await
            .unwrap();
        assert_eq!(last["eof"], true);
        assert!(!last["chunk"].as_str().unwrap().is_empty() || last_off >= row.bytes);

        let past = recall(&sessions, "t:1", &row.id, row.bytes, 8192)
            .await
            .unwrap();
        assert_eq!(past["eof"], true);
        assert_eq!(past["chunk"], "");
    }

    #[tokio::test]
    async fn recall_denies_other_session_and_traversal() {
        let (_tmp, sessions) = temp_sessions();
        let val = json!({"data": "z".repeat(5000)});
        let _ = pack_tool_result(&sessions, "t:1", "bash", "c1", val, MAX_PROMPT_BYTES).await;
        let ledger = tokio::fs::read_to_string(ledger_path(&observations_dir(
            &sessions, "t:1",
        )))
        .await
        .unwrap();
        let row: LedgerRow = serde_json::from_str(ledger.lines().next().unwrap()).unwrap();

        let err = recall(&sessions, "t:2", &row.id, 0, 100).await.unwrap_err();
        assert_eq!(err, RecallError::NotFound);

        assert!(recall(&sessions, "t:1", "obs_../ledger", 0, 100)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn archive_write_failure_uses_truncate_result() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let sessions_dir = tmp.path(); // a file, not a directory
        let val = json!({"data": "q".repeat(5000)});
        let expected = truncate_result(val.clone(), MAX_PROMPT_BYTES);
        let out = pack_tool_result(
            sessions_dir,
            "t:1",
            "bash",
            "c1",
            val,
            MAX_PROMPT_BYTES,
        )
        .await;
        assert_eq!(out, expected);
        assert!(out.get("observation_pack").is_none());
    }

    #[tokio::test]
    async fn projection_of_huge_result_stays_under_prompt_cap() {
        let (_tmp, sessions) = temp_sessions();
        let val = json!({"data": "x".repeat(200_000)});
        let out =
            pack_tool_result(&sessions, "t:1", "bash", "c1", val, MAX_PROMPT_BYTES).await;
        let ser = serde_json::to_string(&out).unwrap();
        assert!(
            ser.len() <= MAX_PROMPT_BYTES,
            "projection {} bytes exceeds cap",
            ser.len()
        );
        assert!(out.get("observation_pack").is_some());
    }

    #[tokio::test]
    async fn session_gc_removes_observations_dir() {
        use clawft_platform::NativePlatform;
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let platform = Arc::new(NativePlatform::new());
        let mgr = crate::session::SessionManager::with_dir(
            platform.clone(),
            tmp.path().to_path_buf(),
        );
        mgr.get_or_create("t:1").await.unwrap();
        let val = json!({"data": "g".repeat(5000)});
        let _ = pack_tool_result(tmp.path(), "t:1", "bash", "c1", val, MAX_PROMPT_BYTES).await;
        let obs = observations_dir(tmp.path(), "t:1");
        assert!(obs.exists());

        mgr.delete_session("t:1").await.unwrap();
        assert!(!obs.exists(), "delete_session must remove {obs:?}");
    }

    #[tokio::test]
    async fn orphan_gc_removes_observations_without_jsonl() {
        use clawft_platform::NativePlatform;
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let platform = Arc::new(NativePlatform::new());
        let val = json!({"data": "o".repeat(5000)});
        let _ = pack_tool_result(tmp.path(), "orphan:1", "bash", "c1", val, MAX_PROMPT_BYTES)
            .await;
        let obs = observations_dir(tmp.path(), "orphan:1");
        assert!(obs.exists());

        let removed =
            gc_orphaned_observation_dirs(platform.as_ref(), tmp.path(), false).await;
        assert!(
            removed.iter().any(|n| n.ends_with(".observations")),
            "expected orphan dir in {removed:?}"
        );
        assert!(!obs.exists());
    }
}
