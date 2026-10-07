//! ADR-108 workspace adoption: an existing identity registered at a second
//! machine's checkout, with no key, chain or serve section.

use super::tests::{ID, mkdir};
use super::*;

#[test]
fn adopt_writes_a_workspace_identity_and_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let root = mkdir(tmp.path(), "app");
    let m = adopt_workspace(&root, &dir, ID, Some("app"), &[]).unwrap();
    assert_eq!(m.id, ID);
    assert_eq!(m.name, "app");
    assert!(m.is_workspace());
    assert!(m.serve.is_none() && m.legacy.is_none() && m.chain.is_none());
    let pt = read_project_toml(&root).unwrap().unwrap();
    assert_eq!(pt.id, ID);
    assert!(pt.is_workspace());
    // Only project.toml is written: no key, certificate or chain.
    let entries: Vec<_> = std::fs::read_dir(root.join(PROJECT_DIR))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(entries, [PROJECT_TOML]);
    // The manifest round-trips through the index the reporter reads.
    let listed = list_manifests(&dir).unwrap().manifests;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].is_workspace());
    assert_eq!(listed[0].root, root.canonicalize().unwrap());
}

#[test]
fn adopt_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let root = mkdir(tmp.path(), "p");
    let a = adopt_workspace(&root, &dir, ID, None, &[]).unwrap();
    let b = adopt_workspace(&root, &dir, ID, None, &[]).unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(a.created, b.created);
    assert_eq!(list_manifests(&dir).unwrap().manifests.len(), 1);
}

#[test]
fn adopt_refuses_a_tree_with_another_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let root = mkdir(tmp.path(), "p");
    let own = adopt_or_init(&root, &dir, None).unwrap();
    assert_ne!(own.id, ID);
    let err = adopt_workspace(&root, &dir, ID, None, &[]).unwrap_err();
    assert!(matches!(err, ProjectError::AdoptRefused { .. }), "{err}");
    // The original identity is untouched.
    assert_eq!(read_project_toml(&root).unwrap().unwrap().id, own.id);
}

#[test]
fn adopt_refuses_a_primary_identity_with_the_same_id() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let root = mkdir(tmp.path(), "p");
    let own = adopt_or_init(&root, &dir, None).unwrap();
    let err = adopt_workspace(&root, &dir, &own.id, None, &[]).unwrap_err();
    assert!(err.to_string().contains("primary identity"), "{err}");
}

#[test]
fn adopt_refuses_the_same_id_at_a_second_root() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let a = mkdir(tmp.path(), "a");
    let b = mkdir(tmp.path(), "b");
    adopt_workspace(&a, &dir, ID, None, &[]).unwrap();
    let err = adopt_workspace(&b, &dir, ID, None, &[]).unwrap_err();
    assert!(matches!(err, ProjectError::RootConflict { .. }), "{err}");
}

#[test]
fn adopt_refuses_an_invalid_id() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let root = mkdir(tmp.path(), "p");
    assert!(matches!(
        adopt_workspace(&root, &dir, "../escape", None, &[]),
        Err(ProjectError::InvalidId(_))
    ));
    assert!(read_project_toml(&root).unwrap().is_none());
}

#[test]
fn an_ordinary_project_is_not_a_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let root = mkdir(tmp.path(), "p");
    let m = adopt_or_init(&root, &dir, None).unwrap();
    assert!(!m.is_workspace());
    assert!(!read_project_toml(&root).unwrap().unwrap().is_workspace());
}

#[test]
fn extra_repos_are_stored_merged_and_validated() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("manifests");
    let root = mkdir(tmp.path(), "app");
    let a = mkdir(tmp.path(), "brain").canonicalize().unwrap();
    let b = mkdir(tmp.path(), "docs").canonicalize().unwrap();
    let m = adopt_workspace(&root, &dir, ID, None, std::slice::from_ref(&a)).unwrap();
    assert_eq!(m.workspace_repos(), vec![a.clone()]);
    // A later call adds to the list without duplicating.
    let m = adopt_workspace(&root, &dir, ID, None, &[a.clone(), b.clone()]).unwrap();
    assert_eq!(m.workspace_repos(), vec![a.clone(), b.clone()]);
    let m = adopt_workspace(&root, &dir, ID, None, &[]).unwrap();
    assert_eq!(m.workspace_repos().len(), 2);
    // Relative paths and paths inside the root are refused.
    assert!(adopt_workspace(&root, &dir, ID, None, &[std::path::PathBuf::from("../x")]).is_err());
    let inside = root.canonicalize().unwrap().join("sub");
    assert!(adopt_workspace(&root, &dir, ID, None, &[inside]).is_err());
    // An ordinary project never reports extra repos.
    let other = mkdir(tmp.path(), "plain");
    assert!(adopt_or_init(&other, &dir, None).unwrap().workspace_repos().is_empty());
}
