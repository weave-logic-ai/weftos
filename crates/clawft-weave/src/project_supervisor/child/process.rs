//! The OS side of one child: waiting for its exit, the `ChildLauncher`
//! contract, native spawn, terminate, probe and signalling.

use super::*;

impl Launcher {
    /// Wait until the child of `id` is gone and say how it ended.
    pub async fn wait_exit(&self, id: &str) -> ExitInfo {
        enum W {
            Owned(watch::Receiver<Option<ExitInfo>>),
            Adopted(u32),
            Container(
                String,
                super::super::container::OperatorConfig,
                super::super::container::Mounts,
            ),
            None,
        }
        let w = match self.procs().get(id).map(|e| &e.proc) {
            Some(Proc::Owned { exit, .. }) => W::Owned(exit.clone()),
            Some(Proc::Adopted { pid }) => W::Adopted(*pid),
            Some(Proc::Container {
                id, cfg, mounts, ..
            }) => W::Container(id.clone(), cfg.clone(), mounts.clone()),
            None => W::None,
        };
        match w {
            W::None => ExitInfo::default(),
            W::Owned(mut rx) => loop {
                if let Some(info) = *rx.borrow() {
                    return info;
                }
                if rx.changed().await.is_err() {
                    return rx.borrow().unwrap_or_default();
                }
            },
            W::Adopted(pid) => {
                while adopted_alive(pid) {
                    tokio::time::sleep(self.cfg.exit_poll).await;
                }
                prune_if_gone(pid);
                // A clean shutdown removes kernel.pid; a crash leaves it.
                let clean = !self.run_dir(id).join("kernel.pid").exists();
                ExitInfo {
                    code: None,
                    signal: None,
                    clean_hint: clean,
                }
            }
            W::Container(cid, cfg, mounts) => {
                let client = self.container_client(cfg);
                loop {
                    match client.inspect(&cid, id, &mounts).await {
                        Ok(i) if !i.running => {
                            return ExitInfo {
                                code: i.exit_code,
                                signal: None,
                                clean_hint: i.exit_code == Some(0),
                            };
                        }
                        // Inspection uncertainty cannot be treated as a crash:
                        // that would permit an ungoverned duplicate launch.
                        _ => tokio::time::sleep(self.cfg.exit_poll).await,
                    }
                }
            }
        }
    }
}

#[async_trait]
impl ChildLauncher for Launcher {
    async fn spawn(&self, spec: &ChildSpec) -> Result<ChildRef, RuntimeError> {
        let id = spec.project_id.as_str();
        if matches!(self.probe_inner(id).await, ChildProbe::Running { .. }) {
            return Err(RuntimeError::InvalidState(format!(
                "{id} already has a live kernel"
            )));
        }
        if let Some(saved) = state::read(&self.run_dir(id)).and_then(|s| s.container) {
            let cfg = super::super::container::OperatorConfig::load(&self.cfg.home).map_err(backend)?;
            if cfg.engine != saved.engine {
                return Err(backend(
                    "operator engine differs from the persisted container",
                ));
            }
            let inspected = self
                .container_client(cfg)
                .inspect(&saved.id, id, &self.container_mounts(id, &spec.root))
                .await
                .map_err(backend)?;
            if inspected.running {
                return Err(RuntimeError::InvalidState(format!(
                    "{id} already has a running container"
                )));
            }
        }
        self.forget(id);
        let manifest = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(backend)?
            .ok_or_else(|| backend("missing project manifest"))?;
        let adapter = super::super::wasmtime::selected(&manifest, &self.run_dir(id)).map_err(backend)?;
        if adapter != spec.adapter {
            return Err(backend("loaded adapter differs from manifest"));
        }
        if adapter == super::super::wasmtime::ADAPTER {
            super::super::wasmtime::preflight(&self.cfg, &spec.root, &self.run_dir(id)).map_err(backend)?;
        }
        let token = self.issue_token(id)?;
        let run_dir = self.run_dir(id);
        let sandbox = manifest.serve.map_or(ProjectSandbox::Logical, |s| s.sandbox);
        let started = match sandbox {
            ProjectSandbox::Logical | ProjectSandbox::Seatbelt | ProjectSandbox::Wasmtime => {
                match self.write_run_files(spec, &token) {
                    Ok(()) => self.launch(spec, &run_dir).await,
                    Err(e) => Err(e),
                }
            }
            ProjectSandbox::LinuxContainer => self.launch_container(spec, &token).await,
        };
        if started.is_err() {
            // Nothing is left behind by a failed spawn: no live token, no
            // spawn file with a nonce in it, no outstanding expectation.
            cancel_spawn(id);
            self.revoke_tokens(id);
            self.clean_spawn_file(id);
        }
        started
    }

    async fn terminate(
        &self,
        child: &ChildRef,
        grace: Duration,
    ) -> Result<Option<i32>, RuntimeError> {
        self.terminate_inner(child, grace).await
    }

    async fn probe(&self, project_id: &str) -> ChildProbe {
        self.probe_inner(project_id).await
    }
}

impl Launcher {
    pub(super) async fn launch(&self, spec: &ChildSpec, run_dir: &Path) -> Result<ChildRef, RuntimeError> {
        let id = spec.project_id.as_str();
        let sandbox = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(|e| backend(format!("sandbox manifest: {e}")))?
            .ok_or_else(|| RuntimeError::AdmissionRefused(format!("project {id} has no manifest")))?
            .serve
            .unwrap_or_default()
            .sandbox;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode_0600()
            .open(run_dir.join(LOG_FILE_NAME))
            .map_err(|e| backend(format!("kernel.log: {e}")))?;
        let log2 = log.try_clone().map_err(backend)?;
        if sandbox == ProjectSandbox::Wasmtime {
            super::super::wasmtime::validate_log(run_dir).map_err(backend)?;
            let user = clawft_types::project::canon::hex_encode(&self.parts.user_key.verifying_key().to_bytes());
            let (exe, config) = super::super::wasmtime::launch(&self.cfg, spec, run_dir, &user).map_err(backend)?;
            let mut cmd = std::process::Command::new(&exe);
            cmd.arg(config)
                .env_clear()
                .current_dir(run_dir)
                .stdin(std::process::Stdio::null())
                .stdout(log)
                .stderr(log2)
                .process_group(0);
            return self.spawn_owned(id, run_dir, cmd, &exe);
        }
        let tmp = spec.root.join(".weftos/tmp");
        std::fs::create_dir_all(&tmp).map_err(|e| backend(format!("project tmp: {e}")))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| backend(format!("project tmp permissions: {e}")))?;
        // A bounded child must never resolve an absent runtime/config path
        // through the daemon owner's HOME. Give it an in-tree home instead.
        let nested = clawft_types::project::read_project_toml(&spec.root)
            .map_err(|e| RuntimeError::AdmissionRefused(format!("project.toml: {e}")))?
            .is_some_and(|p| p.parent.is_some());
        let home = if sandbox == clawft_types::project::ProjectSandbox::Logical && !nested {
            self.cfg.home.clone()
        } else {
            let home = spec.root.join(".weftos/sandbox-home");
            std::fs::create_dir_all(&home).map_err(|e| backend(format!("sandbox HOME: {e}")))?;
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| backend(format!("sandbox HOME permissions: {e}")))?;
            home
        };
        let mut env = child_env(&home, run_dir, &spec.root, id, |k| std::env::var(k).ok());
        // A nested user instance owns its project kernels: hold a stdin liveness
        // pipe so they exit when it dies by any means (D10). A top-level user
        // daemon keeps null stdin: its children outlive a restart for adoption.
        let owned_liveness = crate::nested_boot::active().is_some();
        if owned_liveness {
            env.push((crate::parent_liveness::ENV.to_owned(), "stdin".to_owned()));
        }
        let bounded = sandbox != clawft_types::project::ProjectSandbox::Logical;
        let launcher = if bounded {
            std::env::current_exe()
                .map_err(|e| backend(format!("sandbox helper executable: {e}")))?
        } else {
            self.cfg.exe.clone()
        };
        let mut cmd = std::process::Command::new(&launcher);
        if bounded {
            cmd.arg(super::super::sandbox::HELPER_ARG).arg(&self.cfg.exe);
        }
        cmd.args(child_args(id))
            .env_clear()
            .envs(env)
            .current_dir(&spec.root)
            .stdin(if owned_liveness {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(log)
            .stderr(log2)
            .process_group(0);
        super::super::sandbox::configure(
            &mut cmd,
            sandbox,
            &spec.root,
            run_dir,
            &self.cfg.parent_socket,
            &self.cfg.exe,
        )
        .map_err(|e| RuntimeError::AdmissionRefused(format!("project sandbox: {e}")))?;
        self.spawn_owned(id, run_dir, cmd, &self.cfg.exe)
    }

    /// Start `cmd` as an owned, supervised child process: waiter thread,
    /// process-group bookkeeping, registry pid and `state.json`.
    pub(super) fn spawn_owned(
        &self,
        id: &str,
        run_dir: &Path,
        mut cmd: std::process::Command,
        exe: &Path,
    ) -> Result<ChildRef, RuntimeError> {
        let mut child = cmd
            .spawn()
            .map_err(|e| backend(format!("cannot start {}: {e}", exe.display())))?;
        let pid = child.id();
        registry().note_pid(id, pid);
        self.spawns.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = watch::channel(None);
        // Before the waiter exists: if the child dies at once, the waiter's
        // prune must find the group already noted (else it would be noted
        // after its own prune, and stay until the next accept).
        note_group(pid);
        // `Child::wait` closes stdin first, so take the write end and hold it for
        // the child's life: it closes when this thread, or the daemon, ends.
        let liveness = child.stdin.take();
        std::thread::spawn(move || {
            let info = child.wait().map(exit_info).unwrap_or_default();
            drop(liveness);
            prune_if_gone(pid);
            let _ = tx.send(Some(info));
        });
        self.procs().insert(
            id.to_owned(),
            Entry {
                proc: Proc::Owned { pid, exit: rx },
                stop: Arc::new(AtomicBool::new(false)),
            },
        );
        state::update(run_dir, |st| {
            st.state = clawft_types::project::ChildState::Starting;
            st.pid = Some(pid);
            st.container = None;
            st.exe = Some(exe.display().to_string());
            st.started_unix = Some(state::now_unix());
            // The new process has not said which build it is yet.
            st.kernel_sha = None;
            st.kernel_version = None;
        });
        Ok(ChildRef {
            project_id: id.to_owned(),
            identity: ChildIdentity::Native { host_pid: pid },
        })
    }

    pub(super) async fn terminate_inner(
        &self,
        child: &ChildRef,
        grace: Duration,
    ) -> Result<Option<i32>, RuntimeError> {
        let id = child.project_id.as_str();
        let container = match self.procs().get(id) {
            Some(e) => match &e.proc {
                Proc::Container {
                    id: cid,
                    cfg,
                    mounts,
                    ..
                } => {
                    if !matches!(&child.identity, ChildIdentity::Container { immutable_container_id, .. } if immutable_container_id == cid)
                    {
                        return Err(backend("stale container reference"));
                    }
                    e.stop.store(true, Ordering::SeqCst);
                    Some((cid.clone(), cfg.clone(), mounts.clone()))
                }
                _ => None,
            },
            None => None,
        };
        if let Some((cid, cfg, mounts)) = container {
            let client = self.container_client(cfg);
            // Signed graceful shutdown through the certified child socket.
            self.parts
                .io
                .shutdown(&mounts.runtime.join(SOCKET_NAME), id)
                .await;
            let deadline = tokio::time::Instant::now() + grace;
            loop {
                let i = client.inspect(&cid, id, &mounts).await.map_err(backend)?;
                if !i.running {
                    return Ok(i.exit_code);
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            client
                .stop(&cid, id, &mounts, self.cfg.kill_grace)
                .await
                .map_err(backend)?;
            return Ok(None);
        }
        let (owned, stop) = match self.procs().get(id) {
            Some(e) => (matches!(e.proc, Proc::Owned { .. }), Arc::clone(&e.stop)),
            None => return Ok(None),
        };
        stop.store(true, Ordering::SeqCst);
        let socket = self.run_dir(id).join(SOCKET_NAME);
        // Graceful first: kernel.shutdown (final anchor), then wait.
        self.parts.io.shutdown(&socket, id).await;
        if !self.wait_gone(id, grace).await {
            if !owned && !self.adopted_still_ours(id) {
                return Ok(None);
            }
            self.signal(id, owned, Signal::SIGTERM);
            if !self.wait_gone(id, self.cfg.kill_grace).await {
                // Identity is re-checked inside `signal` for an adopted pid:
                // between SIGTERM and SIGKILL the pid may have been recycled.
                self.signal(id, owned, Signal::SIGKILL);
                self.wait_gone(id, self.cfg.kill_grace).await;
            }
        }
        if self.pid_of(id).is_some() {
            return Err(RuntimeError::Backend(format!(
                "{id}: the kernel is still running after SIGKILL"
            )));
        }
        let info = self.wait_exit(id).await;
        Ok(info.code)
    }

    pub(super) async fn probe_inner(&self, project_id: &str) -> ChildProbe {
        enum P {
            Owned(Option<ExitInfo>, u32),
            Adopted(u32),
            Container(
                String,
                super::super::container::OperatorConfig,
                super::super::container::Mounts,
            ),
            None,
        }
        let p = match self.procs().get(project_id).map(|e| &e.proc) {
            Some(Proc::Owned { pid, exit }) => P::Owned(*exit.borrow(), *pid),
            Some(Proc::Adopted { pid }) => P::Adopted(*pid),
            Some(Proc::Container {
                id, cfg, mounts, ..
            }) => P::Container(id.clone(), cfg.clone(), mounts.clone()),
            None => P::None,
        };
        match p {
            P::None => ChildProbe::NotStarted,
            P::Owned(None, pid) => ChildProbe::Running {
                identity: ChildIdentity::Native { host_pid: pid },
            },
            P::Owned(Some(i), _) => ChildProbe::Exited {
                code: i.code,
                signal: i.signal,
            },
            P::Adopted(pid) if adopted_alive(pid) => ChildProbe::Running {
                identity: ChildIdentity::Native { host_pid: pid },
            },
            P::Adopted(_) => ChildProbe::Exited {
                code: None,
                signal: None,
            },
            P::Container(cid, cfg, mounts) => inspected_container_probe(
                self.container_client(cfg.clone())
                    .inspect(&cid, project_id, &mounts)
                    .await,
                cfg.engine,
                cid,
            ),
        }
    }
}

impl Launcher {
    pub(super) async fn wait_gone(&self, id: &str, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if self.pid_of(id).is_none() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// An adopted pid is signalled only if it still looks like our child
    /// (pid reuse since adoption must never kill a stranger).
    pub(super) fn adopted_still_ours(&self, id: &str) -> bool {
        let Some(pid) = self.pid_of(id) else {
            return false;
        };
        if super::super::wasmtime::is_wasm_run(&self.run_dir(id)) {
            return super::super::wasmtime::identity(&self.run_dir(id), pid);
        }
        super::super::adopt::identity_ok(&self.run_dir(id), pid, &self.cfg.exe)
    }

    pub(super) fn signal(&self, id: &str, owned: bool, sig: Signal) {
        let Some(pid) = self.pid_of(id) else { return };
        let target = Pid::from_raw(pid as i32);
        if owned {
            // A child we started leads its own process group.
            let _ = killpg(target, sig);
            return;
        }
        // An adopted pid is signalled only while it still verifies as ours,
        // and as a group only when it really leads one (a recycled pid that
        // leads nothing is never group-killed).
        if !self.adopted_still_ours(id) {
            return;
        }
        let leads_group = nix::unistd::getpgid(Some(target)).is_ok_and(|g| g == target);
        let _ = if leads_group {
            killpg(target, sig)
        } else {
            kill(target, sig)
        };
    }

    /// Delete `<run>/<id>/spawn.json` (the child consumes it at boot; this
    /// covers a stop, a failure or a spawn that never booted).
    pub fn clean_spawn_file(&self, id: &str) {
        let _ = std::fs::remove_file(self.run_dir(id).join(SPAWN_JSON_FILE));
    }
}

trait OpenOptionsExt0600 {
    fn mode_0600(&mut self) -> &mut Self;
}

impl OpenOptionsExt0600 for std::fs::OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt as _;
        self.mode(0o600)
    }
}
