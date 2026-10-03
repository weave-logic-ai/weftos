//! `weaver update` against a loopback mock release server, fake receipts in
//! temp dirs and a fake daemon host. Nothing here reaches GitHub, the real
//! `~/.config`, installed binaries, a service manager or a real daemon.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use clawft_rpc::doctor::DoctorEnv;
use clawft_rpc::doctor::env::RuntimeSource;

use super::daemon_restart::{Action, DaemonStatus, Host, Inputs};
use super::update_flow::{Ctx, Opts, Outcome, execute};
use super::update_install::Inject;
use super::update_test_support::{Mock, Rel, publish, script, TRIPLE};

pub(super) fn env_for(root: &Path) -> DoctorEnv {
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    DoctorEnv {
        cwd: root.join("proj"),
        path_dirs: Vec::new(),
        extra_bin_dirs: Vec::new(),
        config_dir: home.join(".config"),
        cargo_home: home.join(".cargo"),
        runtime_dir: home.join(".clawft"),
        runtime_source: RuntimeSource::Global,
        home,
        ps_override: Some(String::new()),
        probe_timeout: Duration::from_secs(5),
        probe_scripts: true,
    }
}

pub(super) fn install_old(dir: &Path, version: &str, names: &[&str]) {
    std::fs::create_dir_all(dir).unwrap();
    for n in names {
        let p = dir.join(n);
        std::fs::write(&p, script(n, version)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

pub(super) fn write_receipt(env: &DoctorEnv, prefix: &Path) -> PathBuf {
    let p = env.config_dir.join("weftos/weftos-receipt.json");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let r = serde_json::json!({"binaries": ["weft", "weaver", "weftos"], "install_layout": "flat",
        "install_prefix": prefix, "source": {"app_name": "weftos"}, "version": "0.8.0"});
    std::fs::write(&p, r.to_string()).unwrap();
    p
}

pub(super) fn version_of(p: &Path) -> String {
    let out = Command::new(p).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[derive(Default)]
pub(super) struct Fake {
    pub(super) alive: Vec<u32>,
    pub(super) exe: Option<PathBuf>,
    pub(super) launchd: Option<u32>,
    pub(super) performed: RefCell<Vec<Action>>,
    pub(super) asked_status: RefCell<u32>,
}

impl Host for Fake {
    fn alive(&self, pid: u32) -> bool {
        self.alive.contains(&pid)
    }
    fn exe_of(&self, _: u32) -> Option<PathBuf> {
        self.exe.clone()
    }
    fn status(&self, _: &Path) -> Option<DaemonStatus> {
        let n = *self.asked_status.borrow();
        *self.asked_status.borrow_mut() += 1;
        Some(DaemonStatus { sha: if n == 0 { "old".into() } else { "new".into() }, pid: Some(4242) })
    }
    fn launchd_main_pid(&self, _: u32) -> Option<u32> {
        self.launchd
    }
    fn systemd_main_pid(&self) -> Option<u32> {
        None
    }
    fn perform(&self, a: &Action) -> Result<(), String> {
        self.performed.borrow_mut().push(a.clone());
        Ok(())
    }
    fn settle(&self) {}
}

pub(super) struct World {
    pub(super) _dir: tempfile::TempDir,
    pub(super) root: PathBuf,
    pub(super) mock: Mock,
    pub(super) env: DoctorEnv,
    pub(super) prefix: PathBuf,
}

impl World {
    /// A receipt-managed install of `old` in `<root>/prefix`, release `new` published.
    pub(super) fn receipt_install(old: &str, new: &str, rel: &Rel) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mock = Mock::start();
        publish(&mock, &root.join("work"), new, rel);
        let env = env_for(&root);
        let prefix = root.join("prefix");
        install_old(&prefix, old, &["weft", "weaver", "weftos"]);
        write_receipt(&env, &prefix);
        World { _dir: dir, root, mock, env, prefix }
    }

    pub(super) fn ctx<'a>(&self, exe: PathBuf, host: &'a Fake, prompt: &'a dyn Fn(&str) -> bool) -> Ctx<'a> {
        let rt = self.root.join("run");
        std::fs::create_dir_all(&rt).unwrap();
        Ctx {
            src: self.mock.source(),
            triple: TRIPLE.into(),
            current_version: "0.8.0".into(),
            current_exe: exe.clone(),
            dirty: false,
            env: self.env.clone(),
            restart_base: Some(Inputs {
                pid_file: rt.join("kernel.pid"),
                socket: rt.join("kernel.sock"),
                installed_exe: exe,
                uid: 501,
                manifests_dir: None,
                polls: 2,
            }),
            host,
            interactive: false,
            prompt,
            sudo_root: false,
            inject: Inject::default(),
        }
    }

    pub(super) fn weaver(&self) -> PathBuf {
        self.prefix.join("weaver")
    }

    pub(super) fn versions(&self) -> Vec<String> {
        ["weft", "weaver", "weftos"].iter().map(|n| version_of(&self.prefix.join(n))).collect()
    }

    pub(super) fn leftovers(&self) -> Vec<String> {
        std::fs::read_dir(&self.prefix)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect()
    }
}

pub(super) fn no(_: &str) -> bool {
    false
}

pub(super) fn run(ctx: &Ctx<'_>, opts: Opts) -> (anyhow::Result<Outcome>, String) {
    let mut out = Vec::new();
    let r = execute(ctx, &opts, &mut out);
    (r, String::from_utf8(out).unwrap())
}

pub(super) const OLD: [&str; 3] = ["weft 0.8.0", "weaver 0.8.0", "weftos 0.8.0"];

pub(super) fn with_daemon(w: &World, host_launchd: Option<u32>) -> Fake {
    std::fs::create_dir_all(w.root.join("run")).unwrap();
    std::fs::write(w.root.join("run/kernel.pid"), "4242\n").unwrap();
    Fake { alive: vec![4242], exe: Some(w.weaver()), launchd: host_launchd, ..Fake::default() }
}

#[test]
fn receipt_install_updates_every_binary_and_the_receipt() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let host = Fake::default();
    let ctx = w.ctx(w.weaver(), &host, &no);
    let (r, out) = run(&ctx, Opts::default());
    assert_eq!(r.unwrap(), Outcome::Installed { version: "0.9.0".into(), daemon_restarted: false }, "{out}");
    assert_eq!(w.versions(), ["weft 0.9.0", "weaver 0.9.0", "weftos 0.9.0"]);
    assert!(w.leftovers().is_empty(), "{:?}", w.leftovers());
    let rc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(w.env.config_dir.join("weftos/weftos-receipt.json")).unwrap()).unwrap();
    assert_eq!(rc["version"], "0.9.0");
    assert_eq!(rc["install_prefix"], w.prefix.to_str().unwrap());
    assert!(host.performed.borrow().is_empty());
}

#[test]
fn bad_checksum_fails_closed_and_installs_nothing() {
    for rel in [
        Rel { corrupt: Some("weftos"), ..Rel::default() },
        Rel { bad_unified: true, ..Rel::default() },
        Rel { no_sha: Some("clawft-cli"), ..Rel::default() },
        Rel { payload_version: Some("0.1.0"), ..Rel::default() },
    ] {
        let w = World::receipt_install("0.8.0", "0.9.0", &rel);
        let host = Fake::default();
        let ctx = w.ctx(w.weaver(), &host, &no);
        let (r, _) = run(&ctx, Opts::default());
        assert!(r.is_err());
        assert_eq!(w.versions(), OLD);
        assert!(w.leftovers().is_empty());
    }
}

#[test]
fn failure_mid_swap_restores_every_binary() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let host = Fake::default();
    for at in [0, 1, 2] {
        let mut ctx = w.ctx(w.weaver(), &host, &no);
        ctx.inject = Inject { swap_at: Some(at), rollback: false };
        let (r, _) = run(&ctx, Opts::default());
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("restored"), "{msg}");
        assert_eq!(w.versions(), OLD, "failure at swap {at}");
        assert!(w.leftovers().is_empty(), "failure at swap {at}: {:?}", w.leftovers());
    }
}

#[test]
fn homebrew_copy_is_refused_with_brew_command_and_untouched() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let cellar = w.root.join("opt/Cellar/weftos/0.8.0/bin");
    install_old(&cellar, "0.8.0", &["weft", "weaver", "weftos"]);
    let bin = w.root.join("opt/bin");
    std::fs::create_dir_all(&bin).unwrap();
    for n in ["weft", "weaver", "weftos"] {
        std::os::unix::fs::symlink(cellar.join(n), bin.join(n)).unwrap();
    }
    let host = Fake::default();
    let ctx = w.ctx(bin.join("weaver"), &host, &no);
    let (r, out) = run(&ctx, Opts::default());
    let Outcome::Refused { command } = r.unwrap() else { panic!("{out}") };
    assert!(command.starts_with("brew upgrade"), "{command}");
    assert_eq!(version_of(&cellar.join("weaver")), "weaver 0.8.0");
    assert_eq!(w.mock.asset_requests(), 0);
}

#[test]
fn dev_cargo_and_unmatched_receipt_installs_are_refused() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let host = Fake::default();

    // build.sh marker.
    let dev = w.root.join("dev/bin");
    install_old(&dev, "0.8.0", &["weft", "weaver", "weftos"]);
    let marker = w.env.config_dir.join("weftos/dev-install.json");
    std::fs::write(&marker, serde_json::json!({"installs": [{"path": dev.join("weaver")}]}).to_string()).unwrap();
    let (r, _) = run(&w.ctx(dev.join("weaver"), &host, &no), Opts::default());
    assert!(matches!(r.unwrap(), Outcome::Refused { command } if command.contains("build.sh")));

    // A -dirty build with no marker.
    let other = w.root.join("other");
    install_old(&other, "0.8.0", &["weaver"]);
    let mut ctx = w.ctx(other.join("weaver"), &host, &no);
    ctx.dirty = true;
    assert!(matches!(run(&ctx, Opts::default()).0.unwrap(), Outcome::Refused { .. }));

    // cargo install ledger.
    let cb = w.env.cargo_home.join("bin");
    install_old(&cb, "0.8.0", &["weaver"]);
    std::fs::write(w.env.cargo_home.join(".crates2.json"), r#"{"installs":{"clawft-weave 0.8.0 (path+file:///w)":{"bins":["weaver"]}}}"#).unwrap();
    assert!(matches!(run(&w.ctx(cb.join("weaver"), &host, &no), Opts::default()).0.unwrap(), Outcome::Refused { .. }));

    // A copy the receipt does not manage while a receipt exists.
    assert!(matches!(run(&w.ctx(other.join("weaver"), &host, &no), Opts::default()).0.unwrap(), Outcome::Refused { .. }));

    assert_eq!(w.versions(), OLD);
    assert_eq!(w.mock.asset_requests(), 0);
}

#[test]
fn a_shadow_copy_of_another_channel_blocks_the_whole_update() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let cellar = w.root.join("opt/Cellar/clawft-cli/0.8.0/bin");
    install_old(&cellar, "0.8.0", &["weft"]);
    std::fs::remove_file(w.prefix.join("weft")).unwrap();
    std::os::unix::fs::symlink(cellar.join("weft"), w.prefix.join("weft")).unwrap();
    let host = Fake::default();
    let (r, _) = run(&w.ctx(w.weaver(), &host, &no), Opts::default());
    assert!(matches!(r.unwrap(), Outcome::Refused { command } if command.starts_with("brew upgrade")));
    assert_eq!(version_of(&w.prefix.join("weaver")), "weaver 0.8.0");
    assert_eq!(w.mock.asset_requests(), 0);
}

#[test]
fn no_receipt_updates_only_binaries_next_to_weaver() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    std::fs::remove_file(w.env.config_dir.join("weftos/weftos-receipt.json")).unwrap();
    std::fs::remove_file(w.prefix.join("weftos")).unwrap();
    let host = Fake::default();
    let (r, out) = run(&w.ctx(w.weaver(), &host, &no), Opts { force: true, ..Opts::default() });
    assert!(matches!(r.unwrap(), Outcome::Installed { .. }), "{out}");
    assert_eq!(version_of(&w.prefix.join("weft")), "weft 0.9.0");
    assert_eq!(version_of(&w.prefix.join("weaver")), "weaver 0.9.0");
    assert!(!w.prefix.join("weftos").exists());
    assert!(out.contains("weftos is not installed"), "{out}");
}

#[test]
fn check_and_dry_run_download_and_change_nothing() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let host = Fake::default();
    let ctx = w.ctx(w.weaver(), &host, &no);
    let (r, out) = run(&ctx, Opts { check: true, ..Opts::default() });
    assert_eq!(r.unwrap(), Outcome::Available { version: "0.9.0".into() });
    assert!(out.contains("Update available"), "{out}");
    let (r, out) = run(&ctx, Opts { dry_run: true, ..Opts::default() });
    assert_eq!(r.unwrap(), Outcome::DryRun);
    assert!(out.contains("v0.8.0 -> v0.9.0"), "{out}");
    assert_eq!(w.versions(), OLD);
    assert_eq!(w.mock.asset_requests(), 0);
}

#[test]
fn up_to_date_and_newer_builds_do_nothing_unless_forced() {
    let w = World::receipt_install("0.9.0", "0.9.0", &Rel::default());
    let host = Fake::default();
    let mut ctx = w.ctx(w.weaver(), &host, &no);
    ctx.current_version = "0.9.0".into();
    assert_eq!(run(&ctx, Opts::default()).0.unwrap(), Outcome::UpToDate);
    ctx.current_version = "1.0.0".into();
    let (r, out) = run(&ctx, Opts::default());
    assert_eq!(r.unwrap(), Outcome::UpToDate);
    assert!(out.contains("not downgrading"), "{out}");
    assert_eq!(w.mock.asset_requests(), 0);
    ctx.current_version = "0.9.0".into();
    assert!(matches!(run(&ctx, Opts { force: true, ..Opts::default() }).0.unwrap(), Outcome::Installed { .. }));
}


#[test]
fn restart_flag_uses_the_service_manager_through_the_host() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let host = with_daemon(&w, Some(4242));
    let ctx = w.ctx(w.weaver(), &host, &no);
    let (r, out) = run(&ctx, Opts { restart: true, ..Opts::default() });
    assert_eq!(r.unwrap(), Outcome::Installed { version: "0.9.0".into(), daemon_restarted: true }, "{out}");
    assert_eq!(*host.performed.borrow(), vec![Action::LaunchdKickstart { uid: 501 }]);
}

#[test]
fn without_a_flag_a_script_run_prints_the_service_command_and_restarts_nothing() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let host = with_daemon(&w, Some(4242));
    let ctx = w.ctx(w.weaver(), &host, &no);
    let (r, out) = run(&ctx, Opts::default());
    assert_eq!(r.unwrap(), Outcome::Installed { version: "0.9.0".into(), daemon_restarted: false });
    assert!(out.contains("launchctl kickstart -k gui/501/"), "{out}");
    assert!(host.performed.borrow().is_empty());

    // --no-restart wins even on a terminal.
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let host = with_daemon(&w, None);
    let yes = |_: &str| true;
    let mut ctx = w.ctx(w.weaver(), &host, &yes);
    ctx.interactive = true;
    let (_, out) = run(&ctx, Opts { no_restart: true, ..Opts::default() });
    assert!(out.contains("weaver kernel stop && weaver kernel start"), "{out}");
    assert!(host.performed.borrow().is_empty());
}

#[test]
fn interactive_prompt_decides_the_restart() {
    for (answer, restarted) in [(true, true), (false, false)] {
        let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
        let host = with_daemon(&w, Some(4242));
        let asked = RefCell::new(0);
        let prompt = |_: &str| {
            *asked.borrow_mut() += 1;
            answer
        };
        let mut ctx = w.ctx(w.weaver(), &host, &prompt);
        ctx.interactive = true;
        let (r, _) = run(&ctx, Opts::default());
        assert!(matches!(r.unwrap(), Outcome::Installed { daemon_restarted, .. } if daemon_restarted == restarted));
        assert_eq!(*asked.borrow(), 1);
        assert_eq!(host.performed.borrow().len(), usize::from(restarted));
    }
}

#[test]
fn untouched_copies_are_reported_with_their_own_update_command() {
    let mut w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let stale = w.root.join("usr-local");
    install_old(&stale, "0.7.0", &["weftos"]);
    w.env.extra_bin_dirs = vec![w.prefix.clone(), stale.clone()];
    let host = Fake::default();
    let (r, out) = run(&w.ctx(w.weaver(), &host, &no), Opts::default());
    assert!(matches!(r.unwrap(), Outcome::Installed { .. }));
    assert!(out.contains("were not touched"), "{out}");
    assert!(out.contains(stale.join("weftos").to_str().unwrap()), "{out}");
    assert_eq!(version_of(&stale.join("weftos")), "weftos 0.7.0");
}

