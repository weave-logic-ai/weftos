//! Real-file tests for `weaver migrate user-chain` (tempdirs only).
#![cfg(feature = "exochain")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use clawft_kernel::chain::ChainManager;
use clawft_kernel::chain_migrate::{MigrateError, MigrateOptions, Outcome, migrate_user_chain, migrate_with_hook};
use clawft_kernel::chain_storage::{ChainLock, choose_default_chain};
use clawft_types::runtime_paths::{
    LEGACY_MIGRATED_MARKER, MIGRATED_FROM_FILE, RuntimePaths, user_chain_root,
};
use ed25519_dalek::SigningKey;

fn far() -> SystemTime {
    SystemTime::now() + Duration::from_secs(86_400)
}

/// A signed legacy chain (rvf + json + key + tree + anchors) with 6 events.
fn legacy(root: &Path) {
    let p = RuntimePaths::at(root);
    std::fs::create_dir_all(p.chain_dir()).unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    std::fs::write(p.chain_key(), key.to_bytes()).unwrap();
    let cm = ChainManager::new(0, 1000).with_signing_key(key);
    for i in 0..5 {
        cm.append("test", "evt", Some(serde_json::json!({ "i": i })));
    }
    cm.save_to_rvf(&p.chain_rvf()).unwrap();
    cm.save_to_file(&p.chain_checkpoint()).unwrap();
    std::fs::write(p.chain_tree(), "{\"tree\":1}").unwrap();
    std::fs::write(p.anchors_ledger(), "{\"a\":1}\n").unwrap();
}

fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    for n in ["chain.rvf", "chain.json", "chain.key", "chain.tree.json", "chain/anchors.jsonl"] {
        v.push((n.to_string(), std::fs::read(root.join(n)).unwrap()));
    }
    v
}

fn opts<'a>(from: &'a Path, to: &'a Path, dry: bool, now: SystemTime) -> MigrateOptions<'a> {
    MigrateOptions { from, to, dry_run: dry, now, tool_version: "test", allow_unsigned: false }
}

struct Fx {
    _t: tempfile::TempDir,
    from: PathBuf,
    to: PathBuf,
}

fn fx() -> Fx {
    let t = tempfile::tempdir().unwrap();
    let from = t.path().join("home/.clawft");
    let to = t.path().join("home/.weftos/chain");
    legacy(&from);
    Fx { _t: t, from, to }
}

fn refused(r: Result<Outcome, MigrateError>) -> String {
    match r {
        Err(MigrateError::Refused(m)) => m,
        other => panic!("expected refusal, got {other:?}"),
    }
}

#[test]
fn dry_run_writes_nothing() {
    let f = fx();
    let before = snapshot(&f.from);
    let Outcome::DryRun(plan) = migrate_user_chain(&opts(&f.from, &f.to, true, far())).unwrap()
    else {
        panic!("not a dry run")
    };
    assert_eq!(plan.head.events, 6);
    assert_eq!(plan.head.signature, "verified");
    assert_eq!(plan.files.len(), 5);
    assert!(!f.to.exists());
    assert!(!f.to.parent().unwrap().exists(), "no destination parent created");
    assert!(!f.from.join("chain.lock").exists());
    assert!(!f.from.join(LEGACY_MIGRATED_MARKER).exists());
    assert_eq!(snapshot(&f.from), before);
}

#[test]
fn full_migration_verifies_and_leaves_source_byte_identical() {
    let f = fx();
    let before = snapshot(&f.from);
    let Outcome::Migrated(plan) = migrate_user_chain(&opts(&f.from, &f.to, false, far())).unwrap()
    else {
        panic!("not migrated")
    };
    assert_eq!(snapshot(&f.from), before, "source untouched");
    assert_eq!(snapshot(&f.to), before, "copy identical");
    // The copy restores through the kernel path with the same head.
    let cm = ChainManager::load_from_rvf(&f.to.join("chain.rvf"), 1000).unwrap();
    assert_eq!(cm.sequence(), plan.head.sequence);
    assert_eq!(cm.len(), 6);
    assert!(cm.verify_integrity().valid);
    // Markers.
    let m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.to.join(MIGRATED_FROM_FILE)).unwrap()).unwrap();
    assert_eq!(m["head_hash"], plan.head.hash);
    assert_eq!(m["sequence"], plan.head.sequence);
    assert_eq!(m["source"], f.from.to_string_lossy().as_ref());
    let txt = std::fs::read_to_string(f.from.join(LEGACY_MIGRATED_MARKER)).unwrap();
    assert!(txt.contains(&format!("migrated-to: {}", f.to.display())), "{txt}");
    // We created chain.lock only for the run and removed it again.
    assert!(!f.from.join("chain.lock").exists());
    // No temp leftovers beside the destination.
    let left: Vec<_> = std::fs::read_dir(f.to.parent().unwrap()).unwrap().collect();
    assert_eq!(left.len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(f.to.join("chain.key")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Idempotent re-run, and metamorphic: same hashes either way.
    assert!(matches!(
        migrate_user_chain(&opts(&f.from, &f.to, false, far())).unwrap(),
        Outcome::AlreadyMigrated(Some(_), false)
    ));
    let other = f.to.with_file_name("chain2");
    migrate_user_chain(&opts(&f.from, &other, false, far())).unwrap();
    assert_eq!(snapshot(&other), snapshot(&f.to));
}

#[cfg(unix)]
#[test]
fn held_lock_is_refused() {
    let f = fx();
    let _held = ChainLock::acquire(&f.from.join("chain.json")).unwrap();
    let m = refused(migrate_user_chain(&opts(&f.from, &f.to, false, far())));
    assert!(m.contains("in use by another kernel"), "{m}");
    let m = refused(migrate_user_chain(&opts(&f.from, &f.to, true, far())));
    assert!(m.contains("in use"), "dry run checks the lock too: {m}");
    assert!(!f.to.exists());
}

#[test]
fn recent_mtime_without_lock_is_refused() {
    let f = fx();
    let m = refused(migrate_user_chain(&opts(&f.from, &f.to, false, SystemTime::now())));
    assert!(m.contains("older kernel"), "{m}");
    assert!(!f.to.exists());
    assert!(!f.from.join("chain.lock").exists(), "refusal must not leave a lock");
}

#[test]
fn differing_destination_is_refused_identical_is_already_migrated() {
    let f = fx();
    std::fs::create_dir_all(&f.to).unwrap();
    std::fs::write(f.to.join("chain.json"), "different").unwrap();
    let m = refused(migrate_user_chain(&opts(&f.from, &f.to, false, far())));
    assert!(m.contains("differs"), "{m}");
    assert_eq!(std::fs::read(f.to.join("chain.json")).unwrap(), b"different");

    // Byte-identical copy without a marker.
    let g = fx();
    std::fs::create_dir_all(g.to.join("chain")).unwrap();
    for (n, b) in snapshot(&g.from) {
        std::fs::write(g.to.join(n), b).unwrap();
    }
    assert!(matches!(
        migrate_user_chain(&opts(&g.from, &g.to, false, far())).unwrap(),
        Outcome::AlreadyMigrated(None, true)
    ));

    // A non-empty destination holding no chain.
    let h = fx();
    std::fs::create_dir_all(&h.to).unwrap();
    std::fs::write(h.to.join("stray"), "x").unwrap();
    assert!(refused(migrate_user_chain(&opts(&h.from, &h.to, false, far()))).contains("not empty"));
}

#[test]
fn source_changed_after_migration_is_refused_not_noop() {
    let f = fx();
    migrate_user_chain(&opts(&f.from, &f.to, false, far())).unwrap();
    std::fs::write(f.from.join("chain.tree.json"), "{\"tree\":2}").unwrap();
    let m = refused(migrate_user_chain(&opts(&f.from, &f.to, false, far())));
    assert!(m.contains("diverged"), "{m}");
}

#[test]
fn corrupted_copy_is_rolled_back() {
    let f = fx();
    let before = snapshot(&f.from);
    let r = migrate_with_hook(&opts(&f.from, &f.to, false, far()), &mut |_, tmp| {
        let rvf = tmp.join("chain.rvf");
        let mut b = std::fs::read(&rvf).unwrap();
        let mid = b.len() / 2;
        b[mid] ^= 0xff;
        std::fs::write(rvf, b).unwrap();
    });
    assert!(matches!(r, Err(MigrateError::VerifyFailed(_))), "{r:?}");
    assert!(!f.to.exists());
    let left: Vec<_> = std::fs::read_dir(f.to.parent().unwrap_or(&f.from)).unwrap().collect();
    assert!(left.iter().all(|e| !e.as_ref().unwrap().file_name().to_string_lossy().contains("migrating")));
    assert_eq!(snapshot(&f.from), before);
    assert!(!f.from.join(LEGACY_MIGRATED_MARKER).exists());
}

#[test]
fn source_mutated_mid_copy_aborts_and_leaves_no_destination() {
    let f = fx();
    let r = migrate_with_hook(&opts(&f.from, &f.to, false, far()), &mut |src, _| {
        std::fs::write(src.join("chain/anchors.jsonl"), "{\"a\":2}\n").unwrap();
    });
    let m = refused(r);
    assert!(m.contains("changed during the copy"), "{m}");
    assert!(!f.to.exists());
    let parent = f.to.parent().unwrap();
    assert_eq!(std::fs::read_dir(parent).unwrap().count(), 0, "temp removed");
}

#[test]
fn truncated_json_only_source_fails_and_leaves_nothing() {
    let t = tempfile::tempdir().unwrap();
    let from = t.path().join("legacy");
    std::fs::create_dir_all(&from).unwrap();
    legacy(&from);
    std::fs::remove_file(from.join("chain.rvf")).unwrap();
    let j = std::fs::read(from.join("chain.json")).unwrap();
    std::fs::write(from.join("chain.json"), &j[..j.len() - 40]).unwrap();
    let to = t.path().join("dest");
    let r = migrate_user_chain(&opts(&from, &to, false, far()));
    assert!(matches!(r, Err(MigrateError::VerifyFailed(_))), "{r:?}");
    assert!(!to.exists());
}

#[test]
fn nothing_to_migrate() {
    let t = tempfile::tempdir().unwrap();
    let r = migrate_user_chain(&opts(t.path(), &t.path().join("d"), false, far()));
    assert!(matches!(r, Err(MigrateError::NothingToMigrate(_))));
}

/// A fake home with a migrated legacy chain, and a chain-less project.
fn booted() -> (Fx, PathBuf, PathBuf) {
    let f = fx();
    let home = f.from.parent().unwrap().to_path_buf();
    assert_eq!(user_chain_root(&home), f.to);
    migrate_user_chain(&opts(&f.from, &f.to, false, far())).unwrap();
    let proj = home.join("proj");
    std::fs::create_dir_all(proj.join(".weftos")).unwrap();
    std::fs::write(proj.join(".weftos/project.toml"), "").unwrap();
    (f, home, proj)
}

#[test]
fn project_boot_after_migration_is_refused_not_silently_moved() {
    let (f, home, proj) = booted();
    let paths = RuntimePaths::resolve_with(None, Some(&proj), Some(&home));
    let c = choose_default_chain(&paths, Some(&home), false, false, far());
    assert!(c.legacy_in_use);
    assert!(c.refusal.expect("refused").contains("migrated"));
    // The override is --adopt-legacy-chain only.
    assert!(choose_default_chain(&paths, Some(&home), false, true, far()).refusal.is_none());
    // A project that has its own chain keeps it.
    std::fs::create_dir_all(proj.join(".weftos/runtime")).unwrap();
    std::fs::write(proj.join(".weftos/runtime/chain.json"), "{}").unwrap();
    let c = choose_default_chain(&paths, Some(&home), false, false, far());
    assert!(c.refusal.is_none() && !c.legacy_in_use);
    let _ = f;
}

#[test]
fn user_profile_with_marker_and_no_user_chain_is_refused() {
    let (f, home, _proj) = booted();
    std::fs::remove_dir_all(&f.to).unwrap(); // the documented rollback
    let paths = RuntimePaths::user_with(None, Some(&home));
    let c = choose_default_chain(&paths, Some(&home), false, false, far());
    assert!(c.refusal.expect("refused").contains("--adopt-legacy-chain"));
    assert!(choose_default_chain(&paths, Some(&home), false, true, far()).refusal.is_none());
}

#[test]
fn boot_falling_back_to_a_migrated_legacy_chain_is_refused() {
    let (f, home, _proj) = booted();
    // Outside any project: the root is ~/.clawft itself.
    let outside = home.join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    let paths = RuntimePaths::resolve_with(None, Some(&outside), Some(&home));
    let c = choose_default_chain(&paths, Some(&home), false, false, far());
    let r = c.refusal.expect("must refuse");
    assert!(r.contains("was migrated") && r.contains("--adopt-legacy-chain"), "{r}");
    assert!(r.contains(&f.to.display().to_string()), "{r}");
    // The override and isolation both work.
    assert!(choose_default_chain(&paths, Some(&home), false, true, far()).refusal.is_none());
    let iso = RuntimePaths::resolve_with(Some("/iso"), Some(&outside), Some(&home));
    assert!(choose_default_chain(&iso, Some(&home), false, false, far()).refusal.is_none());
}

#[test]
fn project_falling_back_to_legacy_when_user_chain_is_gone_is_refused() {
    let (f, home, proj) = booted();
    std::fs::remove_dir_all(&f.to).unwrap();
    let paths = RuntimePaths::resolve_with(None, Some(&proj), Some(&home));
    let c = choose_default_chain(&paths, Some(&home), false, false, far());
    assert!(c.legacy_in_use);
    assert!(c.refusal.expect("refuse").contains("was migrated"));
    assert!(choose_default_chain(&paths, Some(&home), false, true, far()).refusal.is_none());
}

#[test]
fn crash_before_source_marker_is_completed_by_a_rerun() {
    let f = fx();
    migrate_user_chain(&opts(&f.from, &f.to, false, far())).unwrap();
    std::fs::remove_file(f.from.join(LEGACY_MIGRATED_MARKER)).unwrap();
    // Dry run reports but does not write.
    migrate_user_chain(&opts(&f.from, &f.to, true, far())).unwrap();
    assert!(!f.from.join(LEGACY_MIGRATED_MARKER).exists());
    let r = migrate_user_chain(&opts(&f.from, &f.to, false, far())).unwrap();
    assert!(matches!(r, Outcome::AlreadyMigrated(Some(_), true)), "{r:?}");
    let txt = std::fs::read_to_string(f.from.join(LEGACY_MIGRATED_MARKER)).unwrap();
    assert!(txt.contains("migrated-to:"));
    assert!(matches!(
        migrate_user_chain(&opts(&f.from, &f.to, false, far())).unwrap(),
        Outcome::AlreadyMigrated(Some(_), false)
    ));
}

#[cfg(unix)]
#[test]
fn marker_write_failure_is_an_error_and_rerun_completes() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    let from = f.from.clone();
    let r = migrate_with_hook(&opts(&f.from, &f.to, false, far()), &mut |_, _| {
        std::fs::set_permissions(&from, std::fs::Permissions::from_mode(0o555)).unwrap();
    });
    std::fs::set_permissions(&f.from, std::fs::Permissions::from_mode(0o755)).unwrap();
    let Err(MigrateError::Io(m)) = r else { panic!("expected Io error, got {r:?}") };
    assert!(m.contains("NOT yet blocked"), "{m}");
    assert!(f.to.join("chain.rvf").exists(), "destination stays in place");
    let r = migrate_user_chain(&opts(&f.from, &f.to, false, far())).unwrap();
    assert!(matches!(r, Outcome::AlreadyMigrated(_, true)), "{r:?}");
}

#[cfg(unix)]
#[test]
fn modes_are_private_and_stale_temp_dirs_are_swept() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    let parent = f.to.parent().unwrap();
    let stale = parent.join(".chain.migrating-99999");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("chain.key"), "old").unwrap();
    migrate_user_chain(&opts(&f.from, &f.to, false, far())).unwrap();
    assert!(!stale.exists(), "stale temp swept");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(parent), 0o700);
    assert_eq!(mode(&f.to), 0o700);
    assert_eq!(mode(&f.to.join("chain")), 0o700);
    for n in ["chain.rvf", "chain.json", "chain.key", "chain.tree.json", MIGRATED_FROM_FILE] {
        assert_eq!(mode(&f.to.join(n)), 0o600, "{n}");
    }
}

#[test]
fn missing_key_is_refused_unless_allowed() {
    let f = fx();
    std::fs::remove_file(f.from.join("chain.key")).unwrap();
    let m = refused(migrate_user_chain(&opts(&f.from, &f.to, false, far())));
    assert!(m.contains("--allow-unsigned"), "{m}");
    assert!(!f.to.exists());
    let mut o = opts(&f.from, &f.to, false, far());
    o.allow_unsigned = true;
    assert!(matches!(migrate_user_chain(&o).unwrap(), Outcome::Migrated(_)));
}

#[test]
fn marker_is_not_trusted_when_the_destination_chain_was_damaged() {
    let f = fx();
    migrate_user_chain(&opts(&f.from, &f.to, false, far())).unwrap();
    let rvf = f.to.join("chain.rvf");
    let b = std::fs::read(&rvf).unwrap();
    std::fs::write(&rvf, &b[..b.len() / 2]).unwrap();
    let m = refused(migrate_user_chain(&opts(&f.from, &f.to, false, far())));
    assert!(m.contains("not trusting"), "{m}");
}
