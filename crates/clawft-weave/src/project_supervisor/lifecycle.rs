//! Starting a project's child: `ensure_running`, the locked start, build
//! recording and the readiness wait.

use super::*;

impl Supervisor {
    /// Start (or find) the project's child and wait for its handshake.
    /// Idempotent and safe to call concurrently: exactly one child starts.
    pub async fn ensure_running(self: &Arc<Self>, id: &str) -> Result<Running, SupError> {
        clawft_types::project::validate_id(id).map_err(|e| SupError::InvalidId(e.to_string()))?;
        let slot = self.slot(id);
        let _g = slot.gate.lock().await;
        let selected = self.selected_adapter(id)?;
        if slot.st().handle.as_ref().is_some_and(|h| h.runtime != selected) {
            return Err(SupError::Identity("running project adapter differs from manifest".into()));
        }
        let observed = match self.launcher.probe(id).await {
            ChildProbe::Running { identity } => Some(identity.host_pid()),
            ChildProbe::Unverifiable { reason } => return Err(SupError::LiveLeftover(reason)),
            _ => None,
        };
        let current = slot.st().state;
        if let Some(pid) = observed {
            match current {
                ChildState::Running => {
                    if selected == wasmtime::ADAPTER {
                        self.wasm_proof(id, pid).await.map_err(SupError::Identity)?;
                    }
                    return Ok(Running {
                        socket: self.socket(id),
                        pid,
                        started: false,
                    });
                }
                // An automatic restart in flight: its child has a pid but
                // has not bound its socket yet. Never hand that socket out;
                // wait (bounded) for the handshake like a fresh start does.
                ChildState::Starting => {
                    return match self.wait_ready(id).await {
                        Ok(pid) => {
                            self.note_build(id).await;
                            if slot.st().state == ChildState::Starting {
                                self.set_state(id, &slot, ChildState::Running);
                            }
                            Ok(Running {
                                socket: self.socket(id),
                                pid,
                                started: false,
                            })
                        }
                        Err(why) => Err(SupError::NotReady(why)),
                    };
                }
                _ => {}
            }
        }
        if state::is_marked_revoked(&self.cfg.run_root, id) {
            return Err(SupError::Revoked(id.to_owned()));
        }
        let failed = {
            let st = slot.st();
            (st.state == ChildState::Failed).then(|| st.failed.clone().unwrap_or_default())
        };
        if let Some(why) = failed {
            return Err(SupError::Failed(why));
        }
        if let Some(found) = self.scan_one_container(id).await {
            match found {
                Found::AdoptedContainer {
                    host_pid,
                    guest_pid,
                    ..
                } => {
                    return match self
                        .adopt_one_container(id, host_pid, guest_pid, &slot)
                        .await
                    {
                        Ok(()) => Ok(Running {
                            socket: self.socket(id),
                            pid: host_pid,
                            started: false,
                        }),
                        Err(reason) => Err(SupError::LiveLeftover(reason)),
                    };
                }
                Found::UnverifiableContainer { reason, .. }
                    if reason == "container is not running" => {}
                Found::UnverifiableContainer { reason, .. } => {
                    return Err(SupError::LiveLeftover(reason));
                }
                _ => {}
            }
        }
        // A live verified kernel that nobody supervises (an adoption that
        // was skipped, a restarted daemon) is taken over, never duplicated.
        if let Some(found) = self.scan_project(id).await {
            match found {
                Found::Adopted { pid, .. } => {
                    return match self.adopt_one(id, pid, &slot).await {
                        Ok(()) => Ok(Running {
                            socket: self.socket(id),
                            pid,
                            started: false,
                        }),
                        Err(reason) => Err(SupError::LiveLeftover(reason.to_string())),
                    };
                }
                Found::Unverifiable {
                    pid: Some(pid),
                    reason: adopt::Skip::HandshakeFailed(m),
                    ..
                } => {
                    return Err(SupError::LiveLeftover(format!(
                        "pid {pid} holds the lock but {m}"
                    )));
                }
                Found::Unverifiable { .. } => {}
                Found::AdoptedContainer { .. } | Found::UnverifiableContainer { .. } => {}
            }
        }
        self.start_locked(id, &slot).await
    }

    /// `project.start`: same as [`ensure_running`](Self::ensure_running).
    pub async fn start(self: &Arc<Self>, id: &str) -> Result<Running, SupError> {
        self.ensure_running(id).await
    }

    pub(super) async fn start_locked(
        self: &Arc<Self>,
        id: &str,
        slot: &Arc<Slot>,
    ) -> Result<Running, SupError> {
        let (w, manifest) = self.prepare(id)?;
        let existing = {
            let mut st = slot.st();
            match st.tracker.as_mut() {
                Some(t) => {
                    let s = manifest.serve.clone().unwrap_or_default();
                    t.reconfigure(
                        s.restart_max(),
                        Duration::from_secs(s.restart_window_secs()),
                    );
                }
                None => st.tracker = Some(self.tracker_for(&manifest)),
            }
            st.handle.clone()
        };
        let handle = match existing {
            Some(h) => {
                if h.runtime != self.selected_adapter(id)? {
                    return Err(SupError::Identity(
                        "loaded project adapter changed; unload before switching".into(),
                    ));
                }
                h
            }
            None => {
                let h = self
                    .driver_host(self.selected_adapter(id)?)?
                    .load(&w, &Self::host_cfg(id))
                    .await?;
                slot.st().handle = Some(h.clone());
                h
            }
        };
        let bumped = {
            let mut st = slot.st();
            st.generation += 1;
            st.failed = None;
            st.state = ChildState::Starting;
            st.generation
        };
        if let Err(e) = self.driver_host(&handle.runtime)?.start(&handle).await {
            self.set_state(id, slot, ChildState::Stopped);
            return Err(e.into());
        }
        if let Some(t) = slot.st().tracker.as_mut() {
            t.on_started(Instant::now());
        }
        let pid = self.launcher.pid_of(id).unwrap_or(0);
        self.chain(
            "project.kernel.started",
            json!({"project_id": id, "pid": pid}),
        );
        self.set_state(id, slot, ChildState::Starting);
        self.spawn_monitor(id.to_owned(), Arc::clone(slot), bumped);
        match self.wait_ready(id).await {
            Ok(pid) => {
                self.set_state(id, slot, ChildState::Running);
                self.note_build(id).await;
                self.record_kernel_build(id).await;
                Ok(Running {
                    socket: self.socket(id),
                    pid,
                    started: true,
                })
            }
            Err(why) => Err(SupError::NotReady(why)),
        }
    }

    /// Write the version and build of the kernel just started into the
    /// manifest's `[serve]` (`kernel_version`, `kernel_sha`; supervisor-written,
    /// never by the owner), only when they changed.
    pub(super) async fn record_kernel_build(&self, id: &str) {
        // A runner/guest stamp is not the native weaver build stamp.
        if self.selected_adapter(id).ok() != Some("logical") {
            return;
        }
        let (dir, id) = (self.cfg.manifests_dir.clone(), id.to_owned());
        let r = tokio::task::spawn_blocking(move || {
            clawft_types::project::update_manifest(&dir, &id, |m| {
                let s = m.serve.get_or_insert_with(Default::default);
                s.kernel_version = Some(env!("CARGO_PKG_VERSION").to_owned());
                s.kernel_sha = Some(env!("BUILD_GIT_HASH").to_owned());
            })
        })
        .await;
        if !matches!(r, Ok(Ok(Some(_)))) {
            tracing::warn!("could not record the project kernel build in the manifest");
        }
    }

    /// Record the build the running child reports (its handshake `sha` and
    /// `version`) in `state.json`, where [`status`](Self::status) and the
    /// doctor compare it with this daemon's build (a child outlives
    /// `weaver update`; adoption never replaces it).
    pub(super) async fn note_build(&self, id: &str) {
        if self.selected_adapter(id).ok() == Some(wasmtime::ADAPTER) {
            let Some(pid) = self.launcher.pid_of(id) else { return };
            let Ok(h) = self.wasm_proof(id, pid).await else { return };
            state::update(&self.run_dir(id), |s| {
                s.kernel_sha = h["sha"].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
                s.kernel_version = h["version"].as_str().map(str::to_owned);
            });
            return;
        }
        let Some(h) = self.deps.io.handshake(&self.socket(id)).await else {
            return;
        };
        if h.project_id.as_deref() != Some(id) {
            return;
        }
        state::update(&self.run_dir(id), |s| {
            s.kernel_sha = (!h.sha.is_empty()).then(|| h.sha.clone());
            s.kernel_version = (!h.version.is_empty()).then(|| h.version.clone());
        });
    }

    /// Wait for the child's handshake to name this project.
    pub(super) async fn wait_ready(&self, id: &str) -> Result<u32, String> {
        let deadline = tokio::time::Instant::now() + self.cfg.ready_timeout;
        let sock = self.socket(id);
        loop {
            let Some(pid) = self.launcher.pid_of(id) else {
                let info = self.launcher.wait_exit(id).await;
                return Err(format!(
                    "the child exited (code {:?}, signal {:?}) before answering; see {}",
                    info.code,
                    info.signal,
                    self.run_dir(id).join("kernel.log").display()
                ));
            };
            // The answer must come from the process we launched: a stale
            // socket or a squatter answering for the project is not it.
            if self.selected_adapter(id).ok() == Some(wasmtime::ADAPTER) {
                if self.wasm_proof(id, pid).await.is_ok() {
                    return Ok(pid);
                }
            } else if let Some(h) = self.deps.io.handshake(&sock).await
                && h.project_id.as_deref() == Some(id)
            {
                let container = state::read(&self.run_dir(id)).and_then(|s| s.container);
                if let Some(c) = container {
                    let binding = clawft_rpc::mesh_local::ContainerRegistration {
                        engine: c.engine,
                        container_id: c.id,
                        host_socket: c.host_socket.to_string_lossy().into_owned(),
                    };
                    if self.launcher.verify_container(id, &binding).await == Ok(pid)
                        && crate::mesh_local_registry::registry()
                            .facts_at(id)
                            .is_some_and(|f| f.container.as_ref() == Some(&binding))
                        && self.prove_child(id, &sock).await.is_ok()
                    {
                        return Ok(pid);
                    }
                } else if h.pid == pid {
                    return Ok(pid);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "no handshake from {} within {:?}",
                    sock.display(),
                    self.cfg.ready_timeout
                ));
            }
            tokio::time::sleep(self.cfg.ready_poll).await;
        }
    }
}
