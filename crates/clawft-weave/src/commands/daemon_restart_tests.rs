//! Tests for [`super`] with an injected fake host; no real process is signalled.

    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        alive: Vec<u32>,
        exes: Vec<(u32, &'static str)>,
        launchd: Option<u32>,
        systemd: Option<u32>,
        /// sha answers, consumed per `status` call; the last repeats.
        shas: RefCell<Vec<&'static str>>,
        performed: RefCell<Vec<Action>>,
        /// Pids this fake was asked about (alive/exe lookups).
        asked: RefCell<Vec<u32>>,
        fail: bool,
    }

    impl Host for Fake {
        fn alive(&self, pid: u32) -> bool {
            self.asked.borrow_mut().push(pid);
            self.alive.contains(&pid)
        }
        fn exe_of(&self, pid: u32) -> Option<PathBuf> {
            self.asked.borrow_mut().push(pid);
            self.exes.iter().find(|(p, _)| *p == pid).map(|(_, e)| PathBuf::from(e))
        }
        fn status(&self, _: &Path) -> Option<(String, String)> {
            let mut s = self.shas.borrow_mut();
            let sha = if s.len() > 1 { s.remove(0) } else { *s.first()? };
            Some(("0.8.1".into(), sha.into()))
        }
        fn launchd_main_pid(&self, _: u32) -> Option<u32> {
            self.launchd
        }
        fn systemd_main_pid(&self) -> Option<u32> {
            self.systemd
        }
        fn perform(&self, a: &Action) -> Result<(), String> {
            self.performed.borrow_mut().push(a.clone());
            if self.fail { Err("boom".into()) } else { Ok(()) }
        }
        fn settle(&self) {}
    }

    fn inputs(dir: &Path, pid_text: Option<&str>) -> Inputs {
        if let Some(t) = pid_text {
            std::fs::write(dir.join("kernel.pid"), t).unwrap();
        }
        Inputs {
            pid_file: dir.join("kernel.pid"),
            socket: dir.join("kernel.sock"),
            installed_exe: PathBuf::from("/opt/w/weaver"),
            uid: 501,
            manifests_dir: None,
            polls: 3,
        }
    }

    fn healthy() -> Fake {
        Fake {
            alive: vec![4242],
            exes: vec![(4242, "/opt/w/weaver")],
            shas: RefCell::new(vec!["old", "new"]),
            ..Fake::default()
        }
    }

    #[test]
    fn missing_pid_file_does_nothing() {
        let d = tempfile::tempdir().unwrap();
        let f = healthy();
        let r = restart_with(&inputs(d.path(), None), &f);
        assert!(matches!(r.outcome, Outcome::NotRunning(_)));
        assert!(f.performed.borrow().is_empty());
    }

    #[test]
    fn dead_pid_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { alive: vec![], ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242\n")), &f);
        assert!(matches!(r.outcome, Outcome::NotRunning(_)));
        assert!(f.performed.borrow().is_empty());
    }

    #[test]
    fn non_weaver_pid_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { exes: vec![(4242, "/usr/bin/postgres")], ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        assert!(matches!(&r.outcome, Outcome::Refused(m) if m.contains("not a weaver")));
        assert!(f.performed.borrow().is_empty());
    }

    #[test]
    fn unknown_exe_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { exes: vec![], ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        assert!(matches!(r.outcome, Outcome::Refused(_)));
        assert!(f.performed.borrow().is_empty());
    }

    #[test]
    fn garbage_and_reserved_pids_are_refused() {
        for text in ["", "abc", "0", "1", "-5"] {
            let d = tempfile::tempdir().unwrap();
            let f = healthy();
            let r = restart_with(&inputs(d.path(), Some(text)), &f);
            assert!(matches!(r.outcome, Outcome::Refused(_)), "{text:?}");
            assert!(f.performed.borrow().is_empty());
            assert!(f.asked.borrow().is_empty(), "must not even look at {text:?}");
        }
    }

    #[test]
    fn exe_differing_from_installed_reports_without_restarting() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { exes: vec![(4242, "/other/place/weaver")], ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        assert!(matches!(r.outcome, Outcome::ExeMismatch { .. }));
        assert!(f.performed.borrow().is_empty());
    }

    #[test]
    fn deleted_suffix_still_matches_installed_binary() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { exes: vec![(4242, "/opt/w/weaver (deleted)")], ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        assert!(matches!(r.outcome, Outcome::Restarted { .. }), "{r:?}");
    }

    #[test]
    fn sighup_targets_only_the_pid_file_pid() {
        let d = tempfile::tempdir().unwrap();
        // 70730 stands in for another project's live daemon: alive, a weaver.
        let f = Fake {
            alive: vec![4242, 70730],
            exes: vec![(4242, "/opt/w/weaver"), (70730, "/opt/w/weaver")],
            ..healthy()
        };
        let r = restart_with(&inputs(d.path(), Some("4242\n")), &f);
        assert_eq!(*f.performed.borrow(), vec![Action::Sighup { pid: 4242 }]);
        assert!(matches!(r.outcome, Outcome::Restarted { method: "sighup", confirmed: true, .. }));
        assert!(!f.asked.borrow().contains(&70730));
    }

    #[test]
    fn launchd_used_only_when_service_pid_is_the_file_pid() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { launchd: Some(4242), ..healthy() };
        restart_with(&inputs(d.path(), Some("4242")), &f);
        assert_eq!(*f.performed.borrow(), vec![Action::LaunchdKickstart { uid: 501 }]);

        // Loaded, but managing some other pid: fall back to SIGHUP on the file pid.
        let f = Fake { launchd: Some(999), ..healthy() };
        restart_with(&inputs(d.path(), Some("4242")), &f);
        assert_eq!(*f.performed.borrow(), vec![Action::Sighup { pid: 4242 }]);
    }

    #[test]
    fn systemd_used_when_its_main_pid_is_the_file_pid() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { systemd: Some(4242), ..healthy() };
        restart_with(&inputs(d.path(), Some("4242")), &f);
        assert_eq!(*f.performed.borrow(), vec![Action::SystemctlRestart]);
    }

    #[test]
    fn launchd_wins_over_systemd() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { launchd: Some(4242), systemd: Some(4242), ..healthy() };
        restart_with(&inputs(d.path(), Some("4242")), &f);
        assert_eq!(*f.performed.borrow(), vec![Action::LaunchdKickstart { uid: 501 }]);
    }

    #[test]
    fn failed_action_is_reported() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { fail: true, ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        assert!(matches!(r.outcome, Outcome::Failed(_)));
    }

    #[test]
    fn unchanged_sha_and_pid_is_not_confirmed() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake { shas: RefCell::new(vec!["same"]), ..healthy() };
        let r = restart_with(&inputs(d.path(), Some("4242")), &f);
        match r.outcome {
            Outcome::Restarted { confirmed, before, after, .. } => {
                assert!(!confirmed);
                assert_eq!(before, after);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn legacy_daemons_are_listed_not_touched() {
        let d = tempfile::tempdir().unwrap();
        let mdir = d.path().join("projects");
        std::fs::create_dir_all(&mdir).unwrap();
        let rt = d.path().join("example/.weftos/runtime");
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::write(rt.join("kernel.pid"), "70730\n").unwrap();
        let id = "01JABCDEFGHJKMNPQRSTVWXYZ0";
        std::fs::write(
            mdir.join(format!("{id}.toml")),
            format!(
                "schema = 1\nid = \"{id}\"\nname = \"example\"\nroot = \"{}\"\nstate = \"active\"\n\
created = \"2026-01-01T00:00:00Z\"\nlast_seen = \"2026-01-01T00:00:00Z\"\n\n[legacy]\nruntime_dir = \"{}\"\n",
                d.path().join("example").display(),
                rt.display()
            ),
        )
        .unwrap();
        let run = d.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        let mut i = inputs(&run, Some("4242"));
        i.manifests_dir = Some(mdir);
        let f = Fake { alive: vec![4242, 70730], ..healthy() };
        let r = restart_with(&i, &f);
        assert_eq!(r.legacy.len(), 1, "{r:?}");
        assert_eq!(r.legacy[0].pid, Some(70730));
        assert!(r.legacy[0].alive);
        assert_eq!(*f.performed.borrow(), vec![Action::Sighup { pid: 4242 }]);
        assert!(r.lines("x").iter().any(|l| l.contains("left alone")));
    }

    #[test]
    fn parsers() {
        assert_eq!(parse_launchctl_pid("x = {\n\tpid = 812\n\tstate = running\n}"), Some(812));
        assert_eq!(parse_launchctl_pid("state = not running"), None);
        assert_eq!(parse_systemctl_show("ActiveState=active\nMainPID=77\n"), Some(77));
        assert_eq!(parse_systemctl_show("ActiveState=inactive\nMainPID=0\n"), None);
        assert_eq!(parse_systemctl_show("ActiveState=active\nMainPID=0\n"), None);
    }
