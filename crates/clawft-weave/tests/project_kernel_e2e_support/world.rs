//! The world of `project_kernel_e2e.rs`: an in-process user daemon (a kernel
//! with an isolated user chain, the real RPC dispatch on its owner socket at
//! `$WEFTOS_RUNTIME_DIR/kernel.sock` and child-only socket, and the real `project_hooks::post_boot`
//! that installs the supervisor and runs the adoption scan), plus small RPC
//! and wait helpers. `HOME` and `$WEFTOS_RUNTIME_DIR` are a per-process
//! tempdir set in `main` before any thread exists; nothing here reads or
//! writes the real `~/.weftos`, `~/.clawft` or any running daemon.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clawft_kernel::boot::Kernel;
use clawft_kernel::chain::ChainEvent;
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig};
use clawft_weave::project_supervisor::{self, Supervisor};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, watch};

type KernelRef = Arc<RwLock<Kernel<NativePlatform>>>;

/// The per-process directories `main` creates and exports.
pub struct Dirs {
    pub base: PathBuf,
    pub home: PathBuf,
    pub run_root: PathBuf,
    /// The manifest store, deliberately NOT `<home>/.weftos/projects` and not
    /// beside the run root: the revoked marker must not be derived from it.
    pub manifests: PathBuf,
}

impl Dirs {
    /// Under `/tmp`, not `$TMPDIR`: on macOS `$TMPDIR` is a ~50-byte
    /// `/var/folders/...` path, and `<run>/<26-char ULID>/kernel.sock` must
    /// fit the 104-byte `sun_path` (the supervisor refuses a longer one,
    /// `MAX_SOCKET_PATH`). A fresh `tempdir_in` per run, never a shared dir.
    pub fn create() -> Self {
        let base = tempfile::Builder::new()
            .prefix("wk2e")
            .tempdir_in("/tmp")
            .unwrap()
            .keep()
            .canonicalize()
            .unwrap();
        let d = Self {
            home: base.join("h"),
            run_root: base.join("r"),
            manifests: base.join("m").join("projects"),
            base,
        };
        for p in [&d.home, &d.run_root, &d.manifests] {
            std::fs::create_dir_all(p).unwrap();
        }
        d
    }
}

pub struct World {
    pub dirs: Dirs,
    pub sock: PathBuf,
    pub kernel: KernelRef,
    pub sup: Arc<Supervisor>,
    _shutdown: watch::Sender<bool>,
}

impl World {
    /// Boot the user daemon. Must run inside the test runtime.
    pub async fn start(dirs: Dirs) -> Self {
        clawft_weave::user_daemon::enter();
        clawft_weave::project_rpc::init_manifests_dir(dirs.manifests.clone());
        clawft_weave::scope_gate::init(Some(dirs.manifests.clone()), false);
        let kcfg = KernelConfig {
            chain: Some(ChainConfig::isolated_in(&dirs.base.join("userchain"))),
            ..KernelConfig::default()
        };
        let kernel = Kernel::boot(Config::default(), kcfg, Arc::new(NativePlatform::new()))
            .await
            .expect("boot the user daemon's kernel");
        // Keep the owner's CLI endpoint separate from the endpoint named in
        // child spawn.json. Use the production binder for the latter.
        let sock = clawft_types::runtime_paths::RuntimePaths::resolve().socket();
        assert_eq!(sock, dirs.run_root.join("kernel.sock"), "user root is $WEFTOS_RUNTIME_DIR");
        let listener = UnixListener::bind(&sock).unwrap();
        let child_sock = clawft_weave::user_daemon::child_socket_path(&dirs.run_root);
        let child_listener = clawft_weave::user_daemon::bind_child_socket(&dirs.run_root).unwrap();
        // The real post_boot seam: supervisor + adoption scan + idle loop.
        clawft_weave::project_hooks::post_boot(&kernel, &Default::default()).unwrap();
        let sup = project_supervisor::global().expect("post_boot installed the supervisor");
        assert_eq!(sup.config().parent_socket, child_sock, "children use the child-only endpoint");
        let kernel: KernelRef = Arc::new(RwLock::new(kernel));
        let (tx, mut rx) = watch::channel(false);
        let (k, t) = (Arc::clone(&kernel), tx.clone());
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accepted = listener.accept() => match accepted {
                        Ok((s, _)) => {
                            tokio::spawn(clawft_weave::daemon::handle_connection(s, Arc::clone(&k), t.clone()));
                        }
                        Err(_) => break,
                    },
                    _ = rx.changed() => if *rx.borrow() { break; },
                }
            }
        });
        let (k, t, mut rx) = (Arc::clone(&kernel), tx.clone(), tx.subscribe());
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accepted = child_listener.accept() => match accepted {
                        Ok((s, _)) => {
                            tokio::spawn(clawft_weave::daemon::handle_connection_child(s, Arc::clone(&k), t.clone()));
                        }
                        Err(_) => break,
                    },
                    _ = rx.changed() => if *rx.borrow() { break; },
                }
            }
        });
        Self { dirs, sock, kernel, sup, _shutdown: tx }
    }

    /// Admin call on the user daemon (same-uid literal scope, ADR-070).
    pub async fn call(&self, method: &str, params: Value) -> Value {
        rpc(&self.sock, method, params, Some("admin")).await
    }

    /// Events on the user chain of `kind`.
    pub async fn user_events(&self, kind: &str) -> Vec<ChainEvent> {
        let k = self.kernel.read().await;
        k.chain_manager().unwrap().tail_from(0).into_iter().filter(|e| e.kind == kind).collect()
    }

    /// The last lines of a child's `kernel.log` (for failure messages).
    pub fn log_tail(&self, id: &str) -> String {
        let text = std::fs::read_to_string(self.dirs.run_root.join(id).join("kernel.log")).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(40)..].join("\n")
    }
}

/// The executable name of this test binary (every kernel we start has it).
pub fn our_exe_name() -> Option<String> {
    std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
}

/// True when `pid` runs this test binary (one of the kernels we started).
pub fn is_ours(pid: u32) -> bool {
    project_supervisor::adopt::process_exe_name(pid) == our_exe_name()
}

/// On drop (pass or fail), SIGKILL every kernel this test started that may
/// still run: pid files under the run root naming a process that IS this
/// test binary. A pid file can outlive its process and the pid be reused,
/// so the executable is checked first.
pub struct Reaper(pub PathBuf);

impl Drop for Reaper {
    fn drop(&mut self) {
        // Every kernel the in-process supervisor started is our direct child,
        // pid file or not (one that died before writing it, or is still
        // booting); the pid files catch any that were re-parented.
        let mut pids = direct_children();
        if let Ok(rd) = std::fs::read_dir(&self.0) {
            for e in rd.flatten() {
                if let Ok(s) = std::fs::read_to_string(e.path().join("kernel.pid"))
                    && let Ok(pid) = s.trim().parse::<u32>()
                {
                    pids.push(pid);
                }
            }
        }
        for pid in pids {
            if pid != std::process::id() && is_ours(pid) {
                kill9(pid);
            }
        }
    }
}

/// Pids whose parent is this process (`pgrep -P`).
fn direct_children() -> Vec<u32> {
    std::process::Command::new("pgrep")
        .args(["-P", &std::process::id().to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect())
        .unwrap_or_default()
}

/// SIGKILL `pid`, which the caller has checked is one of our kernels.
pub fn kill9(pid: u32) {
    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), nix::sys::signal::Signal::SIGKILL);
}

/// One JSON-RPC line over a unix socket.
pub async fn rpc(sock: &Path, method: &str, params: Value, auth: Option<&str>) -> Value {
    let fut = async {
        let (r, mut w) = UnixStream::connect(sock).await.map_err(|e| e.to_string())?.into_split();
        let mut req = json!({"id": "e2e", "proto": 1, "method": method, "params": params});
        if let Some(a) = auth {
            req["auth"] = json!(a);
        }
        w.write_all(format!("{req}\n").as_bytes()).await.map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(r).read_line(&mut line).await.map_err(|e| e.to_string())?;
        serde_json::from_str::<Value>(line.trim()).map_err(|e| e.to_string())
    };
    match tokio::time::timeout(Duration::from_secs(150), fut).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => json!({"ok": false, "error": e, "error_kind": "transport"}),
        Err(_) => json!({"ok": false, "error": "timed out", "error_kind": "transport"}),
    }
}

/// Wait (bounded) until `f` returns `Some`; `what` names it on timeout.
pub async fn wait_for<T, F, Fut>(what: &str, secs: u64, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(Instant::now() < end, "timed out after {secs}s waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
