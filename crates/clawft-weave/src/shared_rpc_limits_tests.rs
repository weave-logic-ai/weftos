//! `limits_for_blocking`: the per-call manifest check behind `shared.*`.
//!
//! The manifest store is a tempdir injected through `scope_gate::init`; the
//! real home's store is never read (`manifests_dir` returns `None` under
//! `cfg(test)` until `init` is called).

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use clawft_types::project::{ProjectManifest, ProjectState, manifest_path, write_manifest};

use super::*;

const A: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WA";
const B: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WB";
const C: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WC";
const D: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";

fn manifest(id: &str, state: ProjectState) -> ProjectManifest {
    let now = chrono::Utc::now();
    ProjectManifest {
        schema_version: 1,
        id: id.into(),
        name: "t".into(),
        root: std::env::temp_dir(),
        state,
        created: now,
        last_seen: now,
        project_toml: Default::default(),
        seed: None,
        legacy: None,
        serve: None,
        chain: None,
        binary: None,
        extra: Default::default(),
    }
}

/// Write the manifest for `id` with `[shared] max_calls_per_min = calls`
/// appended as raw text (unknown keys are kept in `extra` on read).
fn put(dir: &std::path::Path, id: &str, state: ProjectState, calls: u32) -> PathBuf {
    write_manifest(dir, &manifest(id, state)).unwrap();
    let path = manifest_path(dir, id).unwrap();
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&format!("\n[shared]\nmax_calls_per_min = {calls}\n"));
    std::fs::write(&path, text).unwrap();
    path
}

fn kind(r: Result<SharedLimits, Response>) -> String {
    r.expect_err("must refuse").error_kind.unwrap_or_default()
}

fn set_mtime(path: &std::path::Path, t: SystemTime) {
    std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

fn store() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    crate::scope_gate::init(Some(t.path().to_path_buf()), false);
    t
}

/// An unchanged (mtime, length) stamp serves the cached limits without
/// reading the manifest; a changed stamp reads it again; an edited limit
/// still waits for `shared.reload`.
#[tokio::test]
async fn an_unchanged_stamp_reuses_the_cache_and_edits_wait_for_a_reload() {
    let _serial = crate::scope_gate::TEST_BOUND_LOCK.lock().await;
    let dir = store();
    let path = put(dir.path(), A, ProjectState::Active, 11);
    assert_eq!(limits_for_blocking(A).unwrap().max_calls_per_min, 11);

    // Same length, same mtime, but not a manifest any more: not read.
    let meta = std::fs::metadata(&path).unwrap();
    let (mtime, len) = (meta.modified().unwrap(), meta.len());
    std::fs::write(&path, vec![b'#'; len as usize]).unwrap();
    set_mtime(&path, mtime);
    assert_eq!(limits_for_blocking(A).unwrap().max_calls_per_min, 11, "served from the cache");

    // The stamp moves: the manifest is read, and it is unreadable.
    set_mtime(&path, mtime + Duration::from_secs(5));
    assert_eq!(kind(limits_for_blocking(A)), "project_store_unavailable");

    // A valid manifest with a new limit: cached limits survive the edit...
    put(dir.path(), A, ProjectState::Active, 22);
    assert_eq!(limits_for_blocking(A).unwrap().max_calls_per_min, 11);
    // ...until the operator reloads.
    state::reload();
    assert_eq!(limits_for_blocking(A).unwrap().max_calls_per_min, 22);
}

/// Archiving or unregistering stops the service at once, and drops the cache.
#[tokio::test]
async fn an_archived_or_removed_project_is_refused_and_its_cache_dropped() {
    let _serial = crate::scope_gate::TEST_BOUND_LOCK.lock().await;
    let dir = store();
    let path = put(dir.path(), B, ProjectState::Active, 5);
    limits_for_blocking(B).unwrap();
    assert!(state::cached(B).is_some());

    put(dir.path(), B, ProjectState::Archived, 5);
    assert_eq!(kind(limits_for_blocking(B)), "project_unknown", "archived");
    assert!(state::cached(B).is_none(), "the archived project's limits are gone");

    // Active again: limits are re-derived from the manifest, not resurrected.
    put(dir.path(), B, ProjectState::Active, 9);
    assert_eq!(limits_for_blocking(B).unwrap().max_calls_per_min, 9);

    std::fs::remove_file(path).unwrap();
    assert_eq!(kind(limits_for_blocking(B)), "project_unknown", "unregistered");
    assert!(state::cached(B).is_none());
}

/// No store, a malformed id and an unreadable manifest are
/// `project_store_unavailable`, not "unknown project": the caller can tell a
/// broken store from a missing project.
#[tokio::test]
async fn a_missing_store_or_unreadable_manifest_is_project_store_unavailable() {
    let _serial = crate::scope_gate::TEST_BOUND_LOCK.lock().await;
    crate::scope_gate::init(None, false);
    assert_eq!(kind(limits_for_blocking(C)), "project_store_unavailable", "no manifest store");

    let dir = store();
    assert_eq!(kind(limits_for_blocking("../escape")), "project_store_unavailable", "bad id");
    let path = put(dir.path(), D, ProjectState::Active, 3);
    std::fs::write(&path, "this is = not [valid toml").unwrap();
    assert_eq!(kind(limits_for_blocking(D)), "project_store_unavailable", "unreadable manifest");
    assert!(state::cached(D).is_none());
}
