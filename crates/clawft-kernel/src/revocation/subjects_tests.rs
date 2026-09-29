//! Tests for package / signer key / artifact hash revocation.

use super::*;

const KEY: &str = "abababababababababababababababababababababababababababababababab";
const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn list_in(dir: &tempfile::TempDir) -> (RevocationList, PathBuf) {
    let path = dir.path().join("revoked_hosts.json");
    (RevocationList::new(path.clone()), path)
}

#[test]
fn each_subject_kind_persists_and_is_queryable_after_reload() {
    let dir = tempfile::tempdir().unwrap();
    let (list, path) = list_in(&dir);
    assert!(list.revoke_subject(RevocationKind::Package, "cog/fall-detect", "cve").unwrap());
    assert!(list.revoke_subject(RevocationKind::SignerKey, KEY, "key leaked").unwrap());
    assert!(list.revoke_subject(RevocationKind::ArtifactHash, HASH, "bad build").unwrap());
    drop(list);

    let reloaded = RevocationList::load(path);
    assert!(reloaded.subjects_error().is_none());
    assert!(reloaded.is_subject_revoked(RevocationKind::Package, "cog/fall-detect"));
    assert!(reloaded.is_subject_revoked(RevocationKind::SignerKey, KEY));
    assert!(reloaded.is_subject_revoked(RevocationKind::ArtifactHash, HASH));
    // Kinds do not bleed into each other.
    assert!(!reloaded.is_subject_revoked(RevocationKind::ArtifactHash, KEY));
    assert!(!reloaded.is_subject_revoked(RevocationKind::Package, "cog/other"));

    assert_eq!(reloaded.list_subjects(None).len(), 3);
    let signers = reloaded.list_subjects(Some(RevocationKind::SignerKey));
    assert_eq!(signers.len(), 1);
    assert_eq!(signers[0].id, KEY);
    assert_eq!(signers[0].reason, "key leaked");
    assert_eq!(
        reloaded.find_subject(RevocationKind::ArtifactHash, HASH).unwrap().reason,
        "bad build"
    );
}

#[test]
fn hex_ids_are_case_normalised() {
    let dir = tempfile::tempdir().unwrap();
    let (list, _) = list_in(&dir);
    list.revoke_subject(RevocationKind::SignerKey, &KEY.to_uppercase(), "r").unwrap();
    assert!(list.is_subject_revoked(RevocationKind::SignerKey, KEY));
    assert!(!list.revoke_subject(RevocationKind::SignerKey, KEY, "again").unwrap());
    assert_eq!(list.list_subjects(None)[0].id, KEY);
}

#[test]
fn invalid_ids_are_rejected_and_never_match() {
    let dir = tempfile::tempdir().unwrap();
    let (list, _) = list_in(&dir);
    for (kind, id) in [
        (RevocationKind::SignerKey, "abcd"),
        (RevocationKind::SignerKey, &"zz".repeat(32)),
        (RevocationKind::ArtifactHash, ""),
        (RevocationKind::Package, ""),
        (RevocationKind::Package, "-leading-dash"),
        (RevocationKind::Package, "has space"),
        (RevocationKind::Package, &"a".repeat(MAX_PACKAGE_ID_LEN + 1)),
    ] {
        let err = list.revoke_subject(kind, id, "x").unwrap_err();
        assert!(matches!(err, RevocationError::InvalidId { .. }), "{kind} {id:?}");
        assert!(!list.is_subject_revoked(kind, id));
    }
    assert!(list.list_subjects(None).is_empty());
}

#[test]
fn unrevoke_subject_persists() {
    let dir = tempfile::tempdir().unwrap();
    let (list, path) = list_in(&dir);
    list.revoke_subject(RevocationKind::Package, "cog.a", "r").unwrap();
    assert!(list.unrevoke_subject(RevocationKind::Package, "cog.a").unwrap());
    assert!(!list.unrevoke_subject(RevocationKind::Package, "cog.a").unwrap());
    drop(list);
    assert!(RevocationList::load(path).list_subjects(None).is_empty());
}

#[test]
fn host_file_format_is_untouched_by_subjects() {
    let dir = tempfile::tempdir().unwrap();
    let (list, path) = list_in(&dir);
    list.revoke_host("host-1", "ban");
    list.revoke_subject(RevocationKind::Package, "cog.a", "r").unwrap();
    // Host file is still a plain Vec<RevokedHost>.
    let hosts: Vec<RevokedHost> =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(list.subjects_path(), dir.path().join(SUBJECTS_FILE_NAME));
    assert!(list.subjects_path().exists());
    assert_eq!(list.len(), 1, "subjects do not count as hosts");
}

#[test]
fn malformed_subjects_file_poisons_and_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("revoked_hosts.json");
    let subjects = dir.path().join(SUBJECTS_FILE_NAME);
    std::fs::write(&subjects, b"{not json").unwrap();

    let list = RevocationList::load(path);
    assert!(list.subjects_error().is_some());
    let err = list.revoke_subject(RevocationKind::Package, "cog.a", "r").unwrap_err();
    assert!(matches!(err, RevocationError::Poisoned(_)));
    assert_eq!(std::fs::read(&subjects).unwrap(), b"{not json");
}

#[test]
fn first_revoked_checks_package_signers_and_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let (list, _) = list_in(&dir);
    let clean_key = "cd".repeat(32);
    assert!(list.first_revoked(Some("cog.a"), &vec![clean_key.clone()], &vec![]).is_none());

    list.revoke_subject(RevocationKind::ArtifactHash, HASH, "r").unwrap();
    let hit = list
        .first_revoked(Some("cog.a"), &vec![clean_key], &vec![HASH.to_uppercase()])
        .unwrap();
    assert_eq!(hit.kind, RevocationKind::ArtifactHash);
}
