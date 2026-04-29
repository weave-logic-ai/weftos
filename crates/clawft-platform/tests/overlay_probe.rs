//! Workspace overlay (Layer 3) end-to-end test.
//!
//! Exercises [`load_config_raw`] against a real filesystem so the
//! cwd-relative `.clawft/config.json` discovery + deep-merge path
//! is hit exactly as it is in production. The original probe was
//! marked `#[ignore]` because it pointed at the developer's real
//! workspace at `/home/aepod/dev/clawft`; WEFT-82 rewrites it to
//! run hermetically inside a `tempfile::TempDir` so it is part of
//! the regular `cargo test -p clawft-platform` run.
//!
//! `set_current_dir` is a process-global side effect, so the test
//! serializes through a [`Mutex`] and restores the prior cwd on
//! exit. It does *not* depend on `~/.clawft/` and ignores the
//! `CLAWFT_CONFIG` env var if set.

use std::env;
use std::path::PathBuf;
use std::sync::Mutex;

use clawft_platform::config_loader::load_config_raw;
use serde_json::Value;

/// Process-global lock guarding the `set_current_dir` mutation
/// across the cwd-sensitive tests below. Cargo tests run in
/// parallel by default; without this lock concurrent overlay
/// tests would race on cwd.
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard that snapshots cwd on construction and restores it on
/// drop, so a test failure does not leave the test runner pointed
/// at a temp directory that is about to be deleted.
struct CwdGuard {
    original: PathBuf,
}

impl CwdGuard {
    fn switch_to(target: &std::path::Path) -> Self {
        let original = env::current_dir().expect("read cwd");
        env::set_current_dir(target).expect("switch cwd");
        Self { original }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = env::set_current_dir(&self.original);
    }
}

/// Minimal Environment that ignores the real process env so the
/// loader's Layer-2 discovery (`CLAWFT_CONFIG` → home-dir JSON)
/// is inert and we observe Layer 3 in isolation.
struct EmptyEnv;

impl clawft_platform::env::Environment for EmptyEnv {
    fn get_var(&self, _name: &str) -> Option<String> {
        None
    }
    fn set_var(&self, _name: &str, _value: &str) {}
    fn remove_var(&self, _name: &str) {}
}

#[tokio::test]
async fn workspace_overlay_applied_against_real_fs() {
    // Verify the cwd-relative `.clawft/config.json` Layer-3 overlay
    // surfaces in the merged JSON when load_config_raw runs against
    // a real filesystem rooted in a tempdir.
    let _guard = CWD_LOCK.lock().expect("cwd lock");
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let dot_clawft = tmp.path().join(".clawft");
    std::fs::create_dir_all(&dot_clawft).expect("mkdir .clawft");
    std::fs::write(
        dot_clawft.join("config.json"),
        r#"{
            "routing": {
                "permissions": {
                    "channels": { "agent.chat": { "level": 2 } }
                }
            }
        }"#,
    )
    .expect("write config");

    let _cwd = CwdGuard::switch_to(tmp.path());
    let fs = clawft_platform::fs::NativeFileSystem;
    let env = EmptyEnv;
    let raw = load_config_raw(&fs, &env).await.expect("load_config_raw");

    let level = raw
        .pointer("/routing/permissions/channels/agent.chat/level")
        .and_then(Value::as_u64);
    assert_eq!(
        level,
        Some(2),
        "workspace overlay must surface agent.chat.level=2; merged: {raw}"
    );
}

#[tokio::test]
async fn workspace_overlay_skipped_when_absent_against_real_fs() {
    // No `.clawft/` in the cwd → loader returns empty/defaults; the
    // overlay must not synthesize keys.
    let _guard = CWD_LOCK.lock().expect("cwd lock");
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let _cwd = CwdGuard::switch_to(tmp.path());

    let fs = clawft_platform::fs::NativeFileSystem;
    let env = EmptyEnv;
    let raw = load_config_raw(&fs, &env).await.expect("load_config_raw");

    // Without an overlay the loader's `Path::exists()` check on the
    // workspace path skips Layer 3 entirely. With Layer 2 inert
    // (EmptyEnv blocks CLAWFT_CONFIG and home discovery may still
    // fire on dev machines) we cannot guarantee the merged object is
    // empty, but we *can* guarantee that nothing under the overlay's
    // exclusive key path leaks through.
    assert!(
        raw.pointer("/routing/permissions/channels/agent.chat")
            .is_none(),
        "no overlay should mean no agent.chat key; merged: {raw}"
    );
}

#[tokio::test]
async fn workspace_overlay_invalid_json_is_ignored_against_real_fs() {
    // Malformed workspace JSON must not abort load_config_raw; the
    // overlay is best-effort and the loader logs + continues.
    let _guard = CWD_LOCK.lock().expect("cwd lock");
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let dot_clawft = tmp.path().join(".clawft");
    std::fs::create_dir_all(&dot_clawft).expect("mkdir .clawft");
    std::fs::write(dot_clawft.join("config.json"), "{ this is not json")
        .expect("write malformed config");

    let _cwd = CwdGuard::switch_to(tmp.path());
    let fs = clawft_platform::fs::NativeFileSystem;
    let env = EmptyEnv;
    let raw = load_config_raw(&fs, &env).await.expect("load_config_raw");
    assert!(
        raw.is_object(),
        "loader returns ok despite parse failure; merged: {raw}"
    );
    // Malformed JSON must not produce phantom keys.
    assert!(
        raw.pointer("/routing/permissions/channels/agent.chat")
            .is_none(),
        "malformed overlay must not synthesize keys; merged: {raw}"
    );
}

#[tokio::test]
async fn workspace_overlay_keys_normalize_to_snake_case_against_real_fs() {
    // Workspace JSON written in camelCase (matches the convention used
    // in checked-in `.clawft/config.json` files) must be normalized via
    // `normalize_keys` before merging.
    let _guard = CWD_LOCK.lock().expect("cwd lock");
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let dot_clawft = tmp.path().join(".clawft");
    std::fs::create_dir_all(&dot_clawft).expect("mkdir .clawft");
    std::fs::write(
        dot_clawft.join("config.json"),
        r#"{ "agentDefaults": { "maxTokens": 8192 } }"#,
    )
    .expect("write config");

    let _cwd = CwdGuard::switch_to(tmp.path());
    let fs = clawft_platform::fs::NativeFileSystem;
    let env = EmptyEnv;
    let raw = load_config_raw(&fs, &env).await.expect("load_config_raw");
    assert_eq!(
        raw.pointer("/agent_defaults/max_tokens")
            .and_then(Value::as_u64),
        Some(8192),
        "snake_case normalization missing; merged: {raw}"
    );
}
