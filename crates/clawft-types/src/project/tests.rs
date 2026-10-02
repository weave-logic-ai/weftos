use std::path::{Path, PathBuf};

use super::*;
use crate::workspace::{WorkspaceEntry, WorkspaceRegistry};

pub(super) const GOLDEN_TOML: &str = r#"schema = 1
id = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD"
name = "example-project"
created = "2026-10-01T09:30:00Z"
"#;

pub(super) const GOLDEN_MANIFEST: &str = r#"schema = 1
id = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD"
name = "example-project"
root = "/Users/x/src/example-project"
state = "active"
created = "2026-10-01T09:30:00Z"
last_seen = "2026-10-01T09:30:00Z"
project_toml = "present"

[seed]
source = "workspaces.json"
legacy_name = "example-project"

[legacy]
runtime_dir = "/Users/x/src/example-project/.weftos/runtime"

[serve]
via = "user-daemon"

[binary]
path = ""
sha = ""
version = ""
"#;

pub(super) const ID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";

fn ws(name: &str, path: &Path) -> WorkspaceEntry {
    WorkspaceEntry {
        name: name.into(),
        path: path.to_path_buf(),
        last_accessed: None,
        created_at: None,
    }
}

fn registry(entries: Vec<WorkspaceEntry>) -> WorkspaceRegistry {
    WorkspaceRegistry {
        workspaces: entries,
    }
}

pub(super) fn mkdir(base: &Path, name: &str) -> PathBuf {
    let p = base.join(name);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap()
}

#[test]
fn golden_project_toml_round_trips() {
    let pt: ProjectToml = toml::from_str(GOLDEN_TOML).unwrap();
    assert_eq!(pt.id, ID);
    assert_eq!(pt.schema_version, 1);
    assert!(!pt.is_weave_master());
    assert_eq!(toml::to_string_pretty(&pt).unwrap(), GOLDEN_TOML);
}

#[test]
fn golden_manifest_round_trips() {
    let m: ProjectManifest = toml::from_str(GOLDEN_MANIFEST).unwrap();
    assert_eq!(m.state, ProjectState::Active);
    assert_eq!(m.project_toml, ProjectTomlPresence::Present);
    assert_eq!(m.serve.as_ref().unwrap().via, ServeVia::UserDaemon);
    assert_eq!(
        m.chain_dir(),
        PathBuf::from("/Users/x/src/example-project/.weftos/chain")
    );
    assert_eq!(toml::to_string_pretty(&m).unwrap(), GOLDEN_MANIFEST);
}

#[test]
fn weave_master_and_governance_parse() {
    let text = format!("{GOLDEN_TOML}governance = \"gov.toml\"\n\n[weave]\nmaster = true\n");
    let pt: ProjectToml = toml::from_str(&text).unwrap();
    assert!(pt.is_weave_master());
    assert_eq!(pt.governance.as_deref(), Some("gov.toml"));
    let back: ProjectToml = toml::from_str(&toml::to_string_pretty(&pt).unwrap()).unwrap();
    assert_eq!(back, pt);
}

#[test]
fn unknown_keys_preserved_on_rewrite() {
    let tmp = tempfile::tempdir().unwrap();
    let text = format!("{GOLDEN_TOML}future_flag = true\n\n[future_table]\nk = \"v\"\n");
    std::fs::create_dir_all(tmp.path().join(".weftos")).unwrap();
    std::fs::write(project_toml_path(tmp.path()), text).unwrap();
    let pt = read_project_toml(tmp.path()).unwrap().unwrap();
    write_project_toml(tmp.path(), &pt).unwrap();
    let raw = std::fs::read_to_string(project_toml_path(tmp.path())).unwrap();
    assert!(raw.contains("future_flag = true"), "{raw}");
    assert!(raw.contains("[future_table]"), "{raw}");

    let mut m: ProjectManifest = toml::from_str(GOLDEN_MANIFEST).unwrap();
    m.extra.insert("x".into(), toml::Value::Integer(7));
    write_manifest(tmp.path(), &m).unwrap();
    assert_eq!(
        read_manifest(tmp.path(), ID).unwrap().unwrap().extra["x"].as_integer(),
        Some(7)
    );
}

#[test]
fn id_validation() {
    assert!(validate_id(ID).is_ok());
    assert!(validate_id(&new_id()).is_ok());
    for bad in [
        "",
        "../etc/passwd",
        "01JB8Z3Q0V6X9KQ4M2N7T5R1W",
        "01JB8Z3Q0V6X9KQ4M2N7T5R1WDD",
        "01JB8Z3Q0V6X9KQ4M2N7T5R1WU",
        "01jb8z3q0v6x9kq4m2n7t5r1wd",
        "81JB8Z3Q0V6X9KQ4M2N7T5R1WD",
        "01JB8Z3Q0V6X9KQ4M2N7T5R1/D",
        "..............................",
    ] {
        assert!(
            matches!(validate_id(bad), Err(ProjectError::InvalidId(_))),
            "{bad}"
        );
    }
}

#[test]
fn bad_id_rejected_everywhere_it_becomes_a_path() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(manifest_path(tmp.path(), "../x").is_err());
    assert!(read_manifest(tmp.path(), "../x").is_err());
    let mut m: ProjectManifest = toml::from_str(GOLDEN_MANIFEST).unwrap();
    m.id = "../evil".into();
    assert!(write_manifest(tmp.path(), &m).is_err());
    std::fs::create_dir_all(tmp.path().join(".weftos")).unwrap();
    std::fs::write(
        project_toml_path(tmp.path()),
        GOLDEN_TOML.replace(ID, "not-a-ulid"),
    )
    .unwrap();
    assert!(matches!(
        read_project_toml(tmp.path()),
        Err(ProjectError::InvalidId(_))
    ));
}

#[test]
fn newer_schema_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".weftos")).unwrap();
    std::fs::write(
        project_toml_path(tmp.path()),
        GOLDEN_TOML.replace("schema = 1", "schema = 9"),
    )
    .unwrap();
    assert!(matches!(
        read_project_toml(tmp.path()),
        Err(ProjectError::UnsupportedSchema { found: 9, .. })
    ));
}

#[cfg(unix)]
#[test]
fn file_modes() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = mkdir(tmp.path(), "proj");
    let dir = tmp.path().join("manifests");
    let m = adopt_or_init(&root, &dir, None).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&manifest_path(&dir, &m.id).unwrap()), 0o600);
    assert_eq!(mode(&project_toml_path(&root)), 0o644);
    // No temp files left behind.
    let leftovers = std::fs::read_dir(&dir).unwrap().filter(|e| {
        e.as_ref()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp.")
    });
    assert_eq!(leftovers.count(), 0);
}

#[test]
fn adopt_or_init_is_idempotent_and_never_overwrites() {
    let tmp = tempfile::tempdir().unwrap();
    let root = mkdir(tmp.path(), "proj");
    let dir = tmp.path().join("manifests");
    let a = adopt_or_init(&root, &dir, Some("Alpha")).unwrap();
    assert_eq!(a.name, "Alpha");
    let toml_before = std::fs::read_to_string(project_toml_path(&root)).unwrap();
    let b = adopt_or_init(&root, &dir, Some("Other")).unwrap();
    assert_eq!(a, b);
    assert_eq!(
        toml_before,
        std::fs::read_to_string(project_toml_path(&root)).unwrap()
    );
    assert_eq!(list_manifests(&dir).unwrap().manifests.len(), 1);
    assert_eq!(read_project_toml(&root).unwrap().unwrap().id, a.id);
}

#[test]
fn adopt_reuses_existing_project_toml_and_registers_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = mkdir(tmp.path(), "proj");
    std::fs::create_dir_all(root.join(".weftos")).unwrap();
    std::fs::write(project_toml_path(&root), GOLDEN_TOML).unwrap();
    let dir = tmp.path().join("manifests");
    let m = adopt_or_init(&root, &dir, None).unwrap();
    assert_eq!(m.id, ID);
    assert_eq!(m.name, "example-project");
    assert_eq!(find_by_id(&dir, ID).unwrap().unwrap().root, root);
    assert_eq!(find_by_root(&dir, &root).unwrap().unwrap().id, ID);
}

#[test]
fn adopt_takes_seeded_manifest_id_and_flips_pending() {
    let tmp = tempfile::tempdir().unwrap();
    let root = mkdir(tmp.path(), "proj");
    let dir = tmp.path().join("manifests");
    let report = seed_from_registry(&registry(vec![ws("legacy", &root)]), &dir).unwrap();
    let seeded_id = report.created[0].clone();
    assert!(
        !project_toml_path(&root).exists(),
        "seeding must not write into the tree"
    );
    let m = adopt_or_init(&root, &dir, None).unwrap();
    assert_eq!(m.id, seeded_id);
    assert_eq!(m.project_toml, ProjectTomlPresence::Present);
    assert_eq!(read_project_toml(&root).unwrap().unwrap().id, seeded_id);
    assert_eq!(list_manifests(&dir).unwrap().manifests.len(), 1);
}

#[test]
fn adopt_rejects_bad_root_and_conflicting_id() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    assert!(matches!(
        adopt_or_init(&tmp.path().join("nope"), &dir, None),
        Err(ProjectError::BadRoot(_))
    ));
    // A copied project.toml in a second live tree must not steal the id.
    let a = mkdir(tmp.path(), "a");
    let b = mkdir(tmp.path(), "b");
    adopt_or_init(&a, &dir, None).unwrap();
    std::fs::create_dir_all(b.join(".weftos")).unwrap();
    std::fs::copy(project_toml_path(&a), project_toml_path(&b)).unwrap();
    assert!(matches!(
        adopt_or_init(&b, &dir, None),
        Err(ProjectError::RootConflict { .. })
    ));
}

#[test]
fn seed_is_idempotent_and_leaves_source_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let a = mkdir(tmp.path(), "a");
    let b = mkdir(tmp.path(), "b");
    let json = tmp.path().join("workspaces.json");
    registry(vec![ws("a", &a), ws("b", &b)])
        .save(&json)
        .unwrap();
    let before = std::fs::read(&json).unwrap();
    let dir = tmp.path().join("manifests");

    let first = seed_from_workspaces(&json, &dir).unwrap();
    assert_eq!(first.created.len(), 2);
    let second = seed_from_workspaces(&json, &dir).unwrap();
    assert!(second.created.is_empty() && second.adopted.is_empty());
    assert_eq!(second.unchanged.len(), 2);
    assert_eq!(list_manifests(&dir).unwrap().manifests.len(), 2);
    assert_eq!(before, std::fs::read(&json).unwrap());

    let m = find_by_root(&dir, &a).unwrap().unwrap();
    assert_eq!(m.project_toml, ProjectTomlPresence::Pending);
    assert_eq!(m.seed.unwrap().legacy_name.as_deref(), Some("a"));
    assert!(!project_toml_path(&a).exists());
}

#[test]
fn seed_reuses_existing_project_toml_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = mkdir(tmp.path(), "proj");
    std::fs::create_dir_all(root.join(".weftos")).unwrap();
    std::fs::write(project_toml_path(&root), GOLDEN_TOML).unwrap();
    let dir = tmp.path().join("manifests");
    let r = seed_from_registry(&registry(vec![ws("proj", &root)]), &dir).unwrap();
    assert_eq!(r.adopted, vec![ID.to_string()]);
    let m = find_by_id(&dir, ID).unwrap().unwrap();
    assert_eq!(m.project_toml, ProjectTomlPresence::Present);
}

#[test]
fn seed_marks_vanished_path_missing_and_recovers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = mkdir(tmp.path(), "gone");
    let dir = tmp.path().join("manifests");
    let reg = registry(vec![ws("gone", &root)]);
    let id = seed_from_registry(&reg, &dir).unwrap().created[0].clone();

    std::fs::remove_dir_all(&root).unwrap();
    let r = seed_from_registry(&reg, &dir).unwrap();
    assert_eq!(r.missing, vec![id.clone()]);
    assert_eq!(
        find_by_id(&dir, &id).unwrap().unwrap().state,
        ProjectState::Missing
    );
    assert_eq!(
        list_manifests(&dir).unwrap().manifests.len(),
        1,
        "never deleted"
    );
    // Second run on a still-missing path is a no-op.
    assert_eq!(
        seed_from_registry(&reg, &dir).unwrap().unchanged,
        vec![id.clone()]
    );

    std::fs::create_dir_all(&root).unwrap();
    let r = seed_from_registry(&reg, &dir).unwrap();
    assert_eq!(r.adopted, vec![id.clone()]);
    assert_eq!(
        find_by_id(&dir, &id).unwrap().unwrap().state,
        ProjectState::Active
    );
}

#[test]
fn seed_entry_never_seen_and_already_gone_becomes_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let r = seed_from_registry(&registry(vec![ws("x", &tmp.path().join("nope"))]), &dir).unwrap();
    assert_eq!(r.missing.len(), 1);
    assert_eq!(
        find_by_id(&dir, &r.missing[0]).unwrap().unwrap().state,
        ProjectState::Missing
    );
}

#[test]
fn malformed_inputs_are_skipped_not_fatal() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("garbage.toml"), "not = [valid").unwrap();
    std::fs::write(
        dir.join(format!("{ID}.toml")),
        GOLDEN_MANIFEST.replace(ID, "01JB8Z3Q0V6X9KQ4M2N7T5R1WE"),
    )
    .unwrap();
    let listing = list_manifests(&dir).unwrap();
    assert!(listing.manifests.is_empty());
    assert_eq!(listing.skipped.len(), 2);

    // A workspace whose project.toml is corrupt is skipped with a reason.
    let bad = mkdir(tmp.path(), "bad");
    std::fs::create_dir_all(bad.join(".weftos")).unwrap();
    std::fs::write(project_toml_path(&bad), "id = ").unwrap();
    let good = mkdir(tmp.path(), "good");
    let r = seed_from_registry(&registry(vec![ws("bad", &bad), ws("good", &good)]), &dir).unwrap();
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(r.created.len(), 1);
}

#[test]
fn seed_from_missing_or_corrupt_json() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let r = seed_from_workspaces(&tmp.path().join("absent.json"), &dir).unwrap();
    assert_eq!(r, SeedReport::default());
    let bad = tmp.path().join("bad.json");
    std::fs::write(&bad, "{nope").unwrap();
    assert!(matches!(
        seed_from_workspaces(&bad, &dir),
        Err(ProjectError::Parse { .. })
    ));
}

#[test]
fn seed_parses_legacy_registry_format() {
    let tmp = tempfile::tempdir().unwrap();
    let root = mkdir(tmp.path(), "proj");
    let json = tmp.path().join("workspaces.json");
    std::fs::write(
        &json,
        format!(
            r#"{{"workspaces":[{{"name":"proj","path":{:?},"last_accessed":"2026-01-02T03:04:05Z","created_at":"2025-12-01T00:00:00Z"}}]}}"#,
            root.to_str().unwrap()
        ),
    )
    .unwrap();
    let dir = tmp.path().join("manifests");
    let r = seed_from_workspaces(&json, &dir).unwrap();
    let m = find_by_id(&dir, &r.created[0]).unwrap().unwrap();
    assert_eq!(m.created.to_rfc3339(), "2025-12-01T00:00:00+00:00");
    assert_eq!(m.last_seen.to_rfc3339(), "2026-01-02T03:04:05+00:00");
}

#[test]
fn concurrent_seed_leaves_one_file_per_project() {
    let tmp = tempfile::tempdir().unwrap();
    let entries: Vec<_> = (0..4)
        .map(|i| ws(&format!("p{i}"), &mkdir(tmp.path(), &format!("p{i}"))))
        .collect();
    let reg = registry(entries);
    let dir = tmp.path().join("manifests");
    std::thread::scope(|s| {
        for _ in 0..2 {
            s.spawn(|| seed_from_registry(&reg, &dir).unwrap());
        }
    });
    let listing = list_manifests(&dir).unwrap();
    assert_eq!(listing.manifests.len(), 4);
    assert!(listing.skipped.is_empty());
}

#[test]
fn find_project_toml_walks_up_and_stops() {
    let tmp = tempfile::tempdir().unwrap();
    let root = mkdir(tmp.path(), "proj");
    let deep = mkdir(&root, "a/b/c");
    assert_eq!(find_project_toml(&deep, None), None);
    std::fs::create_dir_all(root.join(".weftos")).unwrap();
    std::fs::write(project_toml_path(&root), GOLDEN_TOML).unwrap();
    assert_eq!(find_project_toml(&deep, None), Some(root.clone()));
    // stop_at is exclusive: a stop at the project root hides it.
    assert_eq!(find_project_toml(&deep, Some(&root)), None);
    assert_eq!(find_project_toml(&deep, Some(tmp.path())), Some(root));
}
