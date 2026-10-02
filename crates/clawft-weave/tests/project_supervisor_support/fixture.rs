//! Fixture for `project_supervisor.rs`: a tempdir `HOME`, one registered
//! project, a user chain and a [`Supervisor`] whose children are the test
//! binary itself acting as a fake project kernel. Nothing here touches the
//! real `~/.weftos`, `~/.clawft` or a running daemon.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clawft_kernel::chain::{ChainEvent, ChainManager};
use clawft_kernel::gate::GovernanceSnapshot;
use clawft_kernel::token_authority::TokenAuthority;
use clawft_types::project::{ChildState, ProjectManifest, ServeSection, ServeVia, adopt_or_init, write_manifest};
use clawft_weave::project_cert_rpc::CertEnv;
use clawft_weave::project_supervisor::idle::{Activity, ActivitySource};
use clawft_weave::project_supervisor::io::RpcChildIo;
use clawft_weave::project_supervisor::{Deps, Supervisor, SupervisorConfig};
use ed25519_dalek::SigningKey;

/// Activity the test controls.
#[derive(Default)]
pub struct FakeActivity(pub Mutex<Option<Activity>>);

impl ActivitySource for FakeActivity {
    fn activity(&self, _project_id: &str) -> Option<Activity> {
        *self.0.lock().unwrap()
    }
}

pub struct Fixture {
    pub tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub mdir: PathBuf,
    pub run_root: PathBuf,
    pub root: PathBuf,
    pub id: String,
    pub chain: Arc<ChainManager>,
    pub user_key: SigningKey,
    pub tokens: Arc<TokenAuthority>,
    pub activity: Arc<FakeActivity>,
}

pub fn user_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

impl Fixture {
    pub fn new() -> Self {
        // Short: `<run>/<ULID>/kernel.sock` must fit in a unix socket path.
        let tmp = tempfile::Builder::new().prefix("wsup").tempdir_in("/tmp").unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("home");
        let mdir = home.join(".weftos/projects");
        let run_root = home.join(".weftos/run");
        let root = base.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&run_root).unwrap();
        let m = adopt_or_init(&root, &mdir, Some("demo")).unwrap();
        let chain = Arc::new(ChainManager::new(0, 1000));
        let tokens = Arc::new(TokenAuthority::new(Arc::clone(&chain), "test-node"));
        Self {
            home,
            mdir,
            run_root,
            root: m.root.clone(),
            id: m.id,
            chain,
            user_key: user_key(),
            tokens,
            activity: Arc::new(FakeActivity::default()),
            tmp,
        }
    }

    pub fn manifest(&self) -> ProjectManifest {
        clawft_types::project::find_by_id(&self.mdir, &self.id).unwrap().unwrap()
    }

    pub fn set_serve(&self, f: impl FnOnce(&mut ServeSection)) {
        let mut m = self.manifest();
        let mut s = m.serve.take().unwrap_or_default();
        s.via = ServeVia::ChildKernel;
        f(&mut s);
        m.serve = Some(s);
        write_manifest(&self.mdir, &m).unwrap();
    }

    pub fn run_dir(&self) -> PathBuf {
        self.run_root.join(&self.id)
    }

    /// What the fake kernel does when started (`serve`, `crash`, `exit0`, ...).
    pub fn behavior(&self, text: &str) {
        std::fs::create_dir_all(self.run_dir()).unwrap();
        std::fs::write(self.run_dir().join("behavior"), text).unwrap();
    }

    pub fn cfg(&self) -> SupervisorConfig {
        let mut c = SupervisorConfig::new(&self.home, std::env::current_exe().unwrap());
        c.backoff_initial = Duration::from_millis(20);
        c.backoff_max = Duration::from_millis(80);
        c.term_grace = Duration::from_millis(400);
        c.kill_grace = Duration::from_secs(1);
        c.ready_timeout = Duration::from_secs(10);
        c.ready_poll = Duration::from_millis(10);
        c.idle_poll = Duration::from_secs(3600);
        c.exit_poll = Duration::from_millis(20);
        c
    }

    pub fn deps(&self) -> Deps {
        Deps {
            cert_env: CertEnv {
                chain: Arc::clone(&self.chain),
                user_key: self.user_key.clone(),
                manifests_dir: self.mdir.clone(),
            },
            snapshot: Arc::new(|| {
                Some(GovernanceSnapshot { rules: Vec::new(), risk_threshold: 0.7, human_approval_required: false })
            }),
            tokens: Some(Arc::clone(&self.tokens)),
            activity: Arc::clone(&self.activity) as Arc<dyn ActivitySource>,
            io: Arc::new(RpcChildIo::new(self.user_key.clone(), self.mdir.clone())),
            gate: None,
        }
    }

    pub fn supervisor(&self) -> Arc<Supervisor> {
        Supervisor::new(self.cfg(), self.deps())
    }

    pub fn events(&self, kind: &str) -> Vec<ChainEvent> {
        self.chain.tail_from(0).into_iter().filter(|e| e.kind == kind).collect()
    }

    /// Pids named by `kernel.pid` files under the run root.
    fn child_pids(&self) -> Vec<u32> {
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.run_root) {
            for e in rd.flatten() {
                if let Ok(s) = std::fs::read_to_string(e.path().join("kernel.pid"))
                    && let Ok(p) = s.trim().parse()
                {
                    v.push(p);
                }
            }
        }
        v
    }
}

impl Drop for Fixture {
    /// Never leave a fake kernel behind, pass or fail: they are ours, named
    /// by pid files inside this tempdir. A pid file can outlive its process,
    /// and the pid may since belong to someone else, so only a process that
    /// is this test binary (a fake kernel) is ever signalled.
    fn drop(&mut self) {
        let me = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        for pid in self.child_pids() {
            let ours = clawft_weave::project_supervisor::adopt::process_exe_name(pid) == me;
            if !ours {
                continue;
            }
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

/// Poll `f` until it is true or `secs` pass.
pub async fn wait_until(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub async fn wait_state(sup: &Supervisor, id: &str, want: ChildState, secs: u64) {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let got = sup.status(id).await.state;
        if got == want {
            return;
        }
        assert!(Instant::now() < end, "state is {got:?}, wanted {want:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The `spawn.json` a fake kernel saw (the real child consumes and deletes
/// it, so the fake copies it first).
pub fn seen_spawn(run_dir: &std::path::Path) -> clawft_types::project::SpawnFile {
    serde_json::from_slice(&std::fs::read(run_dir.join("spawn.seen.json")).unwrap()).unwrap()
}
