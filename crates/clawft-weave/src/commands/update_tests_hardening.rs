//! Hardening tests for `weaver update`: unmanaged confirmation, hostile
//! archives, curl isolation, sudo, rollback paths.


use super::update_flow::{Opts, Outcome};
use super::update_install::Inject;
use super::update_test_support::{Evil, Rel};
use super::update_tests::{Fake, OLD, World, no, run, version_of, with_daemon};

#[test]
fn unmanaged_install_needs_force_or_a_confirmed_prompt() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    std::fs::remove_file(w.env.config_dir.join("weftos/weftos-receipt.json")).unwrap();
    let host = Fake::default();
    // Script run, no --force: refused with the command, nothing changed or downloaded.
    let (r, out) = run(&w.ctx(w.weaver(), &host, &no), Opts::default());
    assert!(matches!(r.unwrap(), Outcome::Refused { command } if command == "weaver update --force"), "{out}");
    // Terminal, answered no.
    let mut ctx = w.ctx(w.weaver(), &host, &no);
    ctx.interactive = true;
    assert!(matches!(run(&ctx, Opts::default()).0.unwrap(), Outcome::Refused { .. }));
    assert_eq!(w.versions(), OLD);
    assert_eq!(w.mock.asset_requests(), 0);
    // --check and --dry-run do not need the confirmation.
    assert!(matches!(run(&ctx, Opts { check: true, ..Opts::default() }).0.unwrap(), Outcome::Available { .. }));
    // Terminal, answered yes.
    let yes = |_: &str| true;
    let mut ctx = w.ctx(w.weaver(), &host, &yes);
    ctx.interactive = true;
    assert!(matches!(run(&ctx, Opts::default()).0.unwrap(), Outcome::Installed { .. }));
    assert_eq!(version_of(&w.prefix.join("weaver")), "weaver 0.9.0");
}

#[test]
fn hostile_archives_are_refused_after_a_valid_checksum() {
    for (evil, why) in [
        (Evil::Symlink, "only files and directories"),
        (Evil::Hardlink, "only files and directories"),
        (Evil::DotDot, "escapes"),
        (Evil::Big, "more than"),
    ] {
        let w = World::receipt_install("0.8.0", "0.9.0", &Rel { evil: Some(evil), ..Rel::default() });
        let host = Fake::default();
        let mut ctx = w.ctx(w.weaver(), &host, &no);
        ctx.src.max_extract_bytes = 1024;
        let (r, out) = run(&ctx, Opts::default());
        let msg = format!("{:#}", r.unwrap_err());
        assert!(msg.contains(why), "{msg}");
        assert!(out.contains("sha256 verified"), "the checksum itself was fine: {out}");
        assert_eq!(w.versions(), OLD);
        assert!(w.leftovers().is_empty());
    }
}

#[test]
fn a_hostile_curlrc_has_no_effect() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let home = w.root.join("hostile-home");
    std::fs::create_dir_all(&home).unwrap();
    // Honoured, this routes every request to a closed port and disables TLS checks.
    let rc = "proxy = \"http://127.0.0.1:9\"\ninsecure\n";
    std::fs::write(home.join(".curlrc"), rc).unwrap();
    std::fs::write(home.join("_curlrc"), rc).unwrap();
    let host = Fake::default();
    let mut ctx = w.ctx(w.weaver(), &host, &no);
    let h = home.to_string_lossy().into_owned();
    ctx.src.curl_env = vec![("HOME".into(), h.clone()), ("CURL_HOME".into(), h)];
    let (r, out) = run(&ctx, Opts::default());
    assert!(matches!(r.unwrap(), Outcome::Installed { .. }), "{out}");
}

#[test]
fn restart_is_refused_when_running_as_root_via_sudo() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let host = with_daemon(&w, Some(4242));
    let mut ctx = w.ctx(w.weaver(), &host, &no);
    ctx.sudo_root = true;
    let (r, out) = run(&ctx, Opts { restart: true, ..Opts::default() });
    assert_eq!(r.unwrap(), Outcome::Installed { version: "0.9.0".into(), daemon_restarted: false });
    assert!(out.contains("via sudo"), "{out}");
    assert!(host.performed.borrow().is_empty());
}

#[test]
fn rollback_failure_is_reported_loudly() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let host = Fake::default();
    let mut ctx = w.ctx(w.weaver(), &host, &no);
    ctx.inject = Inject { swap_at: Some(2), rollback: true };
    let msg = run(&ctx, Opts::default()).0.unwrap_err().to_string();
    assert!(msg.contains("ROLLBACK INCOMPLETE"), "{msg}");
}

#[test]
fn a_real_failure_at_the_backup_step_restores_everything() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    // A non-empty directory where the backup of weftos goes: hard_link and the
    // copy fallback both fail for real, after weft was already swapped.
    let blocker = w.prefix.join(format!(".weftos.old-{}", std::process::id()));
    std::fs::create_dir_all(blocker.join("x")).unwrap();
    let host = Fake::default();
    let (r, _) = run(&w.ctx(w.weaver(), &host, &no), Opts::default());
    let msg = r.unwrap_err().to_string();
    assert!(msg.contains("weftos") && msg.contains("restored"), "{msg}");
    assert_eq!(w.versions(), OLD);
    assert!(blocker.join("x").exists());
}

#[test]
fn an_allowed_downgrade_warns() {
    let w = World::receipt_install("1.0.0", "0.9.0", &Rel::default());
    let host = Fake::default();
    let mut ctx = w.ctx(w.weaver(), &host, &no);
    ctx.current_version = "1.0.0".into();
    let (r, out) = run(&ctx, Opts { allow_downgrade: true, ..Opts::default() });
    assert!(matches!(r.unwrap(), Outcome::Installed { .. }), "{out}");
    assert!(out.contains("WARNING: --allow-downgrade: downgrading from v1.0.0 to v0.9.0"), "{out}");
}
