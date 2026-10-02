//! Identity journal tests; helpers come from the parent test module.

use super::*;
use serde_json::json;

#[test]
fn journal_roundtrip_missing_is_empty_and_order_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let j = IdentityJournal::new(dir.path());
    assert!(j.read(false).unwrap().is_empty());
    assert!(!dir.path().join("identity.lock").exists(), "a reader created the lock file");
    let recs = vec![
        JournalRecord::Register { cert: cert() },
        JournalRecord::Revoke { project_id: PID.into(), key_id: "k".into() },
    ];
    let l = j.lock().unwrap();
    assert!(j.exists(), "first lock creates the journal");
    for r in &recs {
        j.append(&l, r).unwrap();
    }
    assert_eq!(j.read_locked(&l, false).unwrap(), recs);
    drop(l);
    assert_eq!(j.read(false).unwrap(), recs);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(j.path()).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn a_corrupt_empty_or_unknown_journal_is_an_error_and_is_never_overwritten() {
    let rec = JournalRecord::Revoke { project_id: PID.into(), key_id: "k".into() };
    let good = serde_json::to_string(&rec).unwrap();
    for (name, content) in [
        ("empty", String::new()),
        ("garbage", "not json\n".to_owned()),
        ("torn", format!("{good}\n{{\"op\":\"reg")),
        ("blank line", format!("{good}\n\n{good}\n")),
        ("unknown op", "{\"op\":\"nuke\"}\n".to_owned()),
        ("unknown field", "{\"op\":\"revoke\",\"project_id\":\"a\",\"key_id\":\"b\",\"x\":1}\n".to_owned()),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let j = IdentityJournal::new(dir.path());
        std::fs::write(j.path(), &content).unwrap();
        assert!(matches!(j.read(false), Err(IdentityError::JournalCorrupt { .. })), "{name}");
        let l = j.lock().unwrap();
        assert!(j.append(&l, &rec).is_err(), "{name}");
        assert_eq!(std::fs::read_to_string(j.path()).unwrap(), content, "{name}: modified");
    }
}

#[cfg(unix)]
#[test]
fn an_unreadable_journal_is_an_error_not_empty() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let j = IdentityJournal::new(dir.path());
    let rec = JournalRecord::Revoke { project_id: PID.into(), key_id: "k".into() };
    {
        let l = j.lock().unwrap();
        j.append(&l, &rec).unwrap();
    }
    std::fs::set_permissions(j.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = std::fs::read(j.path()).is_ok(); // running as root
    let r = j.read(false);
    std::fs::set_permissions(j.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    if !readable_anyway {
        assert!(matches!(r, Err(IdentityError::JournalCorrupt { .. })));
    }
}

#[test]
fn the_journal_lock_excludes_other_holders() {
    let dir = tempfile::tempdir().unwrap();
    let j = IdentityJournal::new(dir.path());
    let held = j.lock().unwrap();
    assert!(j.try_lock().unwrap().is_none(), "second holder got the lock");
    let j2 = j.clone();
    let t = std::thread::spawn(move || j2.try_lock().unwrap().is_none());
    assert!(t.join().unwrap());
    drop(held);
    assert!(j.try_lock().unwrap().is_some());
}

#[test]
fn a_missing_journal_is_corrupt_once_the_install_has_used_it() {
    let dir = tempfile::tempdir().unwrap();
    let j = IdentityJournal::new(dir.path());
    drop(j.lock().unwrap()); // initialises the journal
    std::fs::remove_file(j.path()).unwrap();
    assert!(matches!(j.read(false), Err(IdentityError::JournalCorrupt { .. })));
    // The lock file is gone too: other evidence still convicts it.
    std::fs::remove_file(dir.path().join("identity.lock")).unwrap();
    assert!(j.read(false).unwrap().is_empty());
    let e = j.read(true).unwrap_err();
    assert!(e.to_string().contains("project.identity.repair"), "{e}");
}

#[test]
fn append_refuses_a_journal_without_a_final_newline() {
    let dir = tempfile::tempdir().unwrap();
    let j = IdentityJournal::new(dir.path());
    let l = j.lock().unwrap();
    let rec = JournalRecord::Revoke { project_id: PID.into(), key_id: "k".into() };
    let line = serde_json::to_string(&rec).unwrap();
    std::fs::write(j.path(), &line).unwrap(); // parses, but no trailing newline
    assert_eq!(j.read_locked(&l, false).unwrap(), vec![rec.clone()]);
    assert!(matches!(j.append(&l, &rec), Err(IdentityError::JournalCorrupt { .. })));
    assert_eq!(std::fs::read_to_string(j.path()).unwrap(), line);
}

#[test]
fn replace_moves_the_old_journal_aside_and_export_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let j = IdentityJournal::new(dir.path());
    let l = j.lock().unwrap();
    std::fs::write(j.path(), "garbage\n").unwrap();
    let old = cert();
    let v = view(
        &chain_with(&[("project.register", json!({"cert": old}))]),
        &[JournalRecord::Revoke { project_id: OTHER_PID.into(), key_id: "dead".into() }],
        &[],
    );
    let recs = v.export_records();
    let moved = j.replace(&l, &recs).unwrap().unwrap();
    assert_eq!(std::fs::read_to_string(moved).unwrap(), "garbage\n");
    assert_eq!(j.read_locked(&l, false).unwrap(), recs);
    let v2 = view(&[], &recs, &[]);
    assert_eq!(v2.bound_key_id(PID), Some(old.project_key_id.as_str()));
    assert!(v2.is_revoked(OTHER_PID, "dead"));
}
