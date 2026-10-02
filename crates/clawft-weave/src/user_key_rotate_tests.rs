//! Tests for [`super`]: every path is injected, never `HOME`.

use std::os::unix::fs::PermissionsExt;

use chrono::TimeZone;
use clawft_types::project::cert::key_id;

use super::*;
use crate::user_key::chain_key_path;

fn t() -> DateTime<Utc> {
    Utc.timestamp_opt(1_800_000_000, 0).unwrap()
}

fn setup(seed: [u8; 32], as_user_key: bool) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let dir = home.join(".weftos");
    std::fs::create_dir_all(dir.join("chain")).unwrap();
    let p = if as_user_key { user_key_path(&home) } else { chain_key_path(&home) };
    std::fs::write(&p, seed).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    let manifests = dir.join("projects");
    (tmp, home, manifests)
}

fn kid(seed: &[u8; 32]) -> String {
    key_id(&pk(seed))
}

#[test]
fn rotation_replaces_the_key_records_a_dual_signed_handover_and_keeps_the_old_key_aside() {
    let (_t, home, manifests) = setup([1u8; 32], true);
    assert!(matches!(
        rotate_user_key(&home, &manifests, true, t()).unwrap(),
        RotateOutcome::WouldRotate { .. }
    ));
    assert_eq!(read_seed(&user_key_path(&home)).unwrap(), [1u8; 32], "dry run writes nothing");

    let RotateOutcome::Rotated { seq, old_key_id, new_key_id, retired } = rotate_user_key(&home, &manifests, false, t()).unwrap() else {
        panic!("expected a rotation")
    };
    assert_eq!((seq, old_key_id.as_str()), (1, kid(&[1u8; 32]).as_str()));
    let now_seed = read_seed(&user_key_path(&home)).unwrap();
    assert_eq!(kid(&now_seed), new_key_id);
    assert_ne!(now_seed, [1u8; 32]);
    let retired = retired.expect("old user.key kept");
    assert_eq!(read_seed(&retired).unwrap(), [1u8; 32]);
    assert_eq!(std::fs::metadata(&retired).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(!user_key_path(&home).with_file_name("user.key.next").exists());

    // The log verifies old -> new and accepts the old key only up to the point.
    let h = RotationLog::new(&manifests).history(&pk(&now_seed)).unwrap();
    assert_eq!(h.rotations(), 1);
    assert!(h.accepts(&pk(&[1u8; 32]), t()));
    assert!(!h.accepts(&pk(&[1u8; 32]), t() + chrono::Duration::seconds(1)));

    // A second rotation chains onto the first.
    let RotateOutcome::Rotated { seq, .. } = rotate_user_key(&home, &manifests, false, t() + chrono::Duration::seconds(60)).unwrap() else {
        panic!()
    };
    assert_eq!(seq, 2);
    let latest = read_seed(&user_key_path(&home)).unwrap();
    assert_eq!(RotationLog::new(&manifests).history(&pk(&latest)).unwrap().rotations(), 2);
}

#[test]
fn a_chain_key_identity_is_rotated_by_writing_user_key_and_chain_key_is_untouched() {
    let (_t, home, manifests) = setup([4u8; 32], false);
    let chain = chain_key_path(&home);
    let before = std::fs::read(&chain).unwrap();
    let RotateOutcome::Rotated { retired, .. } = rotate_user_key(&home, &manifests, false, t()).unwrap() else { panic!() };
    assert!(retired.is_none());
    assert_eq!(std::fs::read(&chain).unwrap(), before);
    let (k, src) = resolve_user_key(&home, false).unwrap();
    assert!(matches!(src, crate::user_key::KeySource::UserKey(_)));
    assert!(RotationLog::new(&manifests).history(&k.verifying_key().to_bytes()).is_ok());
    assert_eq!(RotationLog::new(&manifests).read().unwrap()[0].old_pubkey, clawft_types::project::canon::hex_encode(&pk(&[4u8; 32])));
}

#[test]
fn a_crash_between_the_log_and_the_key_swap_is_finished_by_running_it_again() {
    let (_t, home, manifests) = setup([1u8; 32], true);
    let RotateOutcome::Rotated { retired, new_key_id, .. } = rotate_user_key(&home, &manifests, false, t()).unwrap() else { panic!() };
    // Put the files back as they were after the record was appended.
    let user = user_key_path(&home);
    let next = user.with_file_name("user.key.next");
    std::fs::rename(&user, &next).unwrap();
    std::fs::rename(retired.unwrap(), &user).unwrap();

    let RotateOutcome::Rotated { seq, new_key_id: again, .. } = rotate_user_key(&home, &manifests, false, t() + chrono::Duration::seconds(30)).unwrap() else { panic!() };
    assert_eq!((seq, again), (1, new_key_id), "same record, no second rotation");
    assert_eq!(RotationLog::new(&manifests).read().unwrap().len(), 1);
    assert_eq!(kid(&read_seed(&user).unwrap()), RotationLog::new(&manifests).read().unwrap()[0].new_key_id);
}

#[test]
fn rotation_refuses_while_the_user_daemon_holds_the_chain() {
    let (_t, home, manifests) = setup([1u8; 32], true);
    let ckpt = clawft_types::runtime_paths::user_chain_checkpoint(&home);
    let _held = clawft_kernel::chain_storage::ChainLock::acquire(&ckpt).unwrap();
    let e = rotate_user_key(&home, &manifests, false, t()).unwrap_err();
    assert!(matches!(e, RotateError::DaemonRunning(_)), "{e}");
    assert_eq!(read_seed(&user_key_path(&home)).unwrap(), [1u8; 32]);
    assert!(!manifests.join(clawft_kernel::project_identity::ROTATION_FILE).exists());
}
