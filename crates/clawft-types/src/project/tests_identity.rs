//! Fork, race, datetime and housekeeping tests for the project model.

use std::path::Path;

use super::tests::{GOLDEN_MANIFEST, GOLDEN_TOML, ID, mkdir};
use super::*;

#[test]
fn native_toml_datetimes_are_accepted() {
    let text = GOLDEN_TOML.replace("\"2026-10-01T09:30:00Z\"", "2026-10-01T09:30:00Z");
    let pt: ProjectToml = toml::from_str(&text).unwrap();
    assert_eq!(pt.created.to_rfc3339(), "2026-10-01T09:30:00+00:00");
    let m_text = GOLDEN_MANIFEST.replace("\"2026-10-01T09:30:00Z\"", "2026-10-01T09:30:00+02:00");
    let m: ProjectManifest = toml::from_str(&m_text).unwrap();
    assert_eq!(m.created.to_rfc3339(), "2026-10-01T07:30:00+00:00");
    // Still serialized as a quoted string.
    assert!(
        toml::to_string_pretty(&pt)
            .unwrap()
            .contains("created = \"2026-10-01T09:30:00Z\"")
    );
    // Through the file readers (flatten path) too.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".weftos")).unwrap();
    std::fs::write(project_toml_path(tmp.path()), text).unwrap();
    assert!(read_project_toml(tmp.path()).unwrap().is_some());
}

#[test]
fn conflict_message_names_both_roots_and_remedy() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let a = mkdir(tmp.path(), "orig");
    let b = mkdir(tmp.path(), "copy");
    adopt_or_init(&a, &dir, None).unwrap();
    std::fs::create_dir_all(b.join(".weftos")).unwrap();
    std::fs::copy(project_toml_path(&a), project_toml_path(&b)).unwrap();
    let msg = adopt_or_init(&b, &dir, None).unwrap_err().to_string();
    assert!(
        msg.contains(a.to_str().unwrap()) && msg.contains(b.to_str().unwrap()),
        "{msg}"
    );
    assert!(msg.contains("weft project init --fork"), "{msg}");
}

#[test]
fn reinit_fork_gives_a_copy_its_own_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let a = mkdir(tmp.path(), "orig");
    let b = mkdir(tmp.path(), "copy");
    let orig = adopt_or_init(&a, &dir, Some("orig")).unwrap();
    std::fs::create_dir_all(b.join(".weftos")).unwrap();
    std::fs::copy(project_toml_path(&a), project_toml_path(&b)).unwrap();
    let orig_manifest_before = std::fs::read(manifest_path(&dir, &orig.id).unwrap()).unwrap();

    let fork = reinit_fork(&b, &dir, Some("copy")).unwrap();
    assert_ne!(fork.id, orig.id);
    assert_eq!(fork.name, "copy");
    assert_eq!(fork.root, b);
    let pt = read_project_toml(&b).unwrap().unwrap();
    assert_eq!(pt.id, fork.id);
    assert_eq!(pt.parent.as_deref(), Some(orig.id.as_str()));
    // Original untouched, both registered, and adopt is now idempotent on the fork.
    assert_eq!(
        orig_manifest_before,
        std::fs::read(manifest_path(&dir, &orig.id).unwrap()).unwrap()
    );
    assert_eq!(read_project_toml(&a).unwrap().unwrap().id, orig.id);
    assert_eq!(adopt_or_init(&b, &dir, None).unwrap(), fork);
    assert_eq!(list_manifests(&dir).unwrap().manifests.len(), 2);
}

#[test]
fn reinit_fork_preserves_unknown_keys_and_inits_bare_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let b = mkdir(tmp.path(), "copy");
    std::fs::create_dir_all(b.join(".weftos")).unwrap();
    std::fs::write(project_toml_path(&b), format!("{GOLDEN_TOML}keep = 1\n")).unwrap();
    reinit_fork(&b, &dir, None).unwrap();
    assert!(
        std::fs::read_to_string(project_toml_path(&b))
            .unwrap()
            .contains("keep = 1")
    );
    let bare = mkdir(tmp.path(), "bare");
    let m = reinit_fork(&bare, &dir, None).unwrap();
    assert_eq!(read_project_toml(&bare).unwrap().unwrap().parent, None);
    assert_eq!(m.root, bare);
}

#[test]
fn stale_copy_after_move_hits_the_conflict_path() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let old = mkdir(tmp.path(), "old");
    let m = adopt_or_init(&old, &dir, None).unwrap();
    let new = mkdir(tmp.path(), "new");
    std::fs::create_dir_all(new.join(".weftos")).unwrap();
    std::fs::copy(project_toml_path(&old), project_toml_path(&new)).unwrap();
    // Original still live: stale/new copy conflicts. Once it is really gone,
    // the moved tree re-homes the same id.
    assert!(matches!(
        adopt_or_init(&new, &dir, None),
        Err(ProjectError::RootConflict { .. })
    ));
    std::fs::remove_dir_all(&old).unwrap();
    assert_eq!(adopt_or_init(&new, &dir, None).unwrap().id, m.id);
}

#[test]
fn stale_tmp_files_are_reaped_fresh_ones_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    std::fs::create_dir_all(&dir).unwrap();
    let stale = dir.join(format!(".{ID}.toml.tmp.1.1"));
    let fresh = dir.join(format!(".{ID}.toml.tmp.2.2"));
    std::fs::write(&stale, "x").unwrap();
    std::fs::write(&fresh, "x").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    list_manifests(&dir).unwrap();
    assert!(!stale.exists());
    assert!(fresh.exists());
}

#[test]
fn find_by_root_matches_through_symlinked_spelling_after_removal() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let root = mkdir(tmp.path(), "p");
    let m = adopt_or_init(&root, &dir, None).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    // Un-canonical spelling of a now-missing path still finds the manifest.
    let spelled = tmp.path().join("x").join("..").join("p");
    std::fs::create_dir_all(tmp.path().join("x")).unwrap();
    assert_eq!(
        find_by_root(&dir, &spelled).unwrap().map(|m| m.id),
        Some(m.id)
    );
}

/// Child half of `adopt_races_across_processes`: inert unless driven by it.
#[test]
fn mp_adopt_child() {
    let (Ok(root), Ok(dir), Ok(go)) = (
        std::env::var("P1B_ROOT"),
        std::env::var("P1B_DIR"),
        std::env::var("P1B_GO"),
    ) else {
        return;
    };
    while !Path::new(&go).exists() {
        std::hint::spin_loop();
    }
    let m = adopt_or_init(Path::new(&root), Path::new(&dir), None).unwrap();
    println!("P1B_ID={}", m.id);
}

#[test]
fn adopt_races_across_processes() {
    let exe = std::env::current_exe().unwrap();
    for round in 0..20 {
        let tmp = tempfile::tempdir().unwrap();
        let root = mkdir(tmp.path(), "proj");
        let dir = tmp.path().join("manifests");
        let go = tmp.path().join("go");
        let children: Vec<_> = (0..6)
            .map(|_| {
                std::process::Command::new(&exe)
                    .args(["--exact", "project::tests_identity::mp_adopt_child", "--nocapture"])
                    .env("P1B_ROOT", &root)
                    .env("P1B_DIR", &dir)
                    .env("P1B_GO", &go)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        std::thread::sleep(std::time::Duration::from_millis(300));
        std::fs::write(&go, "").unwrap();
        let mut ids = std::collections::BTreeSet::new();
        for c in children {
            let out = c.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success(),
                "round {round}: {stdout}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let id = stdout
                .lines()
                .find_map(|l| l.strip_prefix("P1B_ID="))
                .expect(&stdout);
            ids.insert(id.to_string());
        }
        assert_eq!(ids.len(), 1, "round {round}: {ids:?}");
        let listing = list_manifests(&dir).unwrap();
        assert_eq!(listing.manifests.len(), 1, "round {round}");
        assert_eq!(listing.manifests[0].id, *ids.iter().next().unwrap());
        assert_eq!(
            read_project_toml(&root).unwrap().unwrap().id,
            listing.manifests[0].id
        );
    }
}
