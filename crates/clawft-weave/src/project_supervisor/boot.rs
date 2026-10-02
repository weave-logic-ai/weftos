//! Boot-time pieces of the supervisor: adoption of verified leftovers, the
//! idle pass and the `post_boot` wiring for the user daemon.

use super::*;

impl Supervisor {
    /// File an expired registry session for an adopted child so it can
    /// re-register without a spawn nonce. Uses the REAL certified project
    /// key: a zero or default key would silently break the child's signed
    /// heartbeats. Without a certificate in force nothing is filed.
    fn file_adopted_session(&self, id: &str, pid: u32) {
        use crate::mesh_local_registry::{NewSession, registry};
        let Ok(view) = crate::project_cert_rpc::current_view(&self.deps.cert_env) else { return };
        let Some(cert) = view.current_cert(id) else { return };
        let Some(project_pubkey) = clawft_types::project::canon::hex_decode::<32>(&cert.project_pubkey) else {
            return;
        };
        registry().adopt_expired(NewSession {
            project_id: id.to_owned(),
            socket: self.socket(id),
            pid,
            addresses: vec![id.to_owned()],
            topic_prefixes: vec![format!("chain/{id}/")],
            version: env!("CARGO_PKG_VERSION").to_owned(),
            project_key_id: cert.project_key_id.clone(),
            project_pubkey,
        });
    }

    /// Adopt the verified child `pid` of `id` into supervision. The project
    /// gate must be held. A verified child the supervisor will not manage is
    /// never left silently running: a revoked project's child is stopped
    /// (identity re-checked before any signal), anything else is reported
    /// with the reason.
    pub(super) async fn adopt_one(
        self: &Arc<Self>,
        id: &str,
        pid: u32,
        slot: &Arc<Slot>,
    ) -> Result<(), adopt::Skip> {
        let (w, manifest) = match self.prepare(id) {
            Ok(x) => x,
            Err(e) => return Err(self.refuse_adopted(id, pid, e).await),
        };
        self.launcher.adopt(id, pid);
        let handle = match self.host.load(&w, &Self::host_cfg(id)).await {
            Ok(h) => h,
            Err(e) => {
                self.launcher.forget(id);
                return Err(adopt::Skip::Refused(format!(
                    "verified kernel pid {pid} not managed (load refused: {e}); it was left running"
                )));
            }
        };
        let g = {
            let mut st = slot.st();
            st.handle = Some(handle);
            st.tracker = Some(self.tracker_for(&manifest));
            st.generation += 1;
            st.generation
        };
        self.file_adopted_session(id, pid);
        self.set_state(id, slot, ChildState::Running);
        self.chain("project.kernel.adopted", json!({"project_id": id, "pid": pid}));
        self.spawn_monitor(id.to_owned(), Arc::clone(slot), g);
        Ok(())
    }

    async fn refuse_adopted(&self, id: &str, pid: u32, e: SupError) -> adopt::Skip {
        if !matches!(e, SupError::Revoked(_)) {
            tracing::warn!(project = %id, error = %e, "verified project kernel not managed");
            return adopt::Skip::Refused(format!("verified kernel pid {pid} not managed: {e}; it was left running"));
        }
        // Revoked: it must not keep running. Terminate re-verifies pid, exe
        // and lock before any signal, so a recycled pid is never hit.
        self.launcher.adopt(id, pid);
        let stopped = self
            .launcher
            .terminate(&ChildRef { project_id: id.to_owned(), pid }, self.cfg.term_grace)
            .await;
        let alive = self.launcher.pid_of(id).is_some();
        self.launcher.forget(id);
        self.launcher.revoke_tokens(id);
        self.chain("project.kernel.stopped", json!({"project_id": id, "pid": pid, "reason": "revoked at adoption"}));
        adopt::Skip::Refused(if alive || stopped.is_err() {
            format!("project is revoked and kernel pid {pid} could NOT be stopped; stop it by hand")
        } else {
            format!("project is revoked; kernel pid {pid} was stopped")
        })
    }

    /// Verify and adopt children left by an earlier daemon (see [`adopt`]).
    pub async fn adopt_on_boot(self: &Arc<Self>) -> Vec<Found> {
        let mut found = adopt::scan(&self.cfg.run_root, &self.cfg.exe, self.deps.io.as_ref()).await;
        for f in &mut found {
            match f {
                Found::Adopted { id, pid } => {
                    let (id2, pid2) = (id.clone(), *pid);
                    let slot = self.slot(&id2);
                    let _g = slot.gate.lock().await;
                    if let Err(reason) = self.adopt_one(&id2, pid2, &slot).await {
                        *f = Found::Unverifiable { id: id2, pid: Some(pid2), reason };
                    }
                }
                Found::Unverifiable { id, pid, reason } if *reason != adopt::Skip::Dead => {
                    tracing::warn!(project = %id, pid = ?pid, %reason, "leftover project kernel not adopted and not signalled");
                }
                Found::Unverifiable { .. } => {}
            }
        }
        *self.leftovers.lock().unwrap_or_else(|e| e.into_inner()) = found.clone();
        found
    }

    /// One idle pass at `now_unix`: stop every running project that has been
    /// quiet for its `idle_stop_secs`. Returns the ids stopped.
    pub async fn idle_pass(self: &Arc<Self>, now_unix: u64) -> Vec<String> {
        let running: Vec<String> = self
            .status_all()
            .await
            .into_iter()
            .filter(|s| s.state == ChildState::Running)
            .map(|s| s.project_id)
            .collect();
        let mut stopped = Vec::new();
        for id in running {
            let Ok(Some(m)) = clawft_types::project::find_by_id(&self.cfg.manifests_dir, &id) else {
                continue;
            };
            let secs = m.serve.as_ref().map_or(0, |s| s.idle_stop_secs());
            let activity = self.deps.activity.activity(&id);
            if idle::should_stop(now_unix, secs, activity.as_ref()) {
                let slot = self.slot(&id);
                let _g = slot.gate.lock().await;
                if slot.st().state == ChildState::Running
                    && matches!(self.stop_locked(&id, &slot, "idle").await, Ok(true))
                {
                    stopped.push(id);
                }
            }
        }
        stopped
    }

    /// Run [`idle_pass`](Self::idle_pass) every `idle_poll` until the
    /// supervisor is dropped.
    pub fn spawn_idle_loop(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let every = self.cfg.idle_poll;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(this) = weak.upgrade() else { return };
                this.idle_pass(state::now_unix()).await;
            }
        });
    }
}

/// `post_boot` body for the user daemon: build the supervisor from the
/// booted kernel, install it, adopt leftovers and start the idle loop. A
/// no-op for any other profile.
pub fn post_boot(kernel: &clawft_kernel::Kernel<clawft_platform::NativePlatform>) {
    if !crate::user_daemon::is_active() {
        return;
    }
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    let Some(chain) = kernel.chain_manager().cloned() else { return };
    let Some(user_key) = chain.signing_key_clone() else { return };
    let (Some(home), Some(manifests_dir)) = (
        clawft_types::runtime_paths::home_dir(),
        crate::project_rpc::configured_dir(),
    ) else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else { return };
    let paths = clawft_types::runtime_paths::RuntimePaths::resolve();
    // The run root the revoked-marker writer uses too (one derivation).
    let Some(run_root) = crate::project_cert_rpc::user_run_root() else { return };
    let mut cfg = SupervisorConfig::new(&home, exe);
    cfg.run_root = run_root;
    cfg.parent_socket = paths.socket();
    cfg.manifests_dir = manifests_dir.clone();
    let gate = kernel.governance_gate().cloned();
    // The parent's real caps, so a child's merged limits start from them
    // (the overlay can only tighten). Boot-time values, like the caps.
    let kc = kernel.kernel_config();
    let parent_limits = clawft_types::config::overlay::Limits {
        max_processes: Some(u64::from(kc.max_processes)),
        spawn_budget: kc.agent.as_ref().map(|a| u64::from(a.subagents.max_per_conv)),
        ..Default::default()
    };
    let deps = Deps {
        cert_env: CertEnv { chain, user_key: user_key.clone(), manifests_dir: manifests_dir.clone() },
        snapshot: Arc::new(move || {
            gate.as_ref().and_then(|g| g.governance_snapshot()).map(|mut s| {
                s.limits = parent_limits;
                s
            })
        }),
        tokens: crate::token_rpc::authority_for_kernel(kernel),
        activity: Arc::new(idle::RegistryActivity),
        io: Arc::new(io::RpcChildIo::new(user_key, manifests_dir)),
        gate: None,
    };
    let sup = Supervisor::new(cfg, deps);
    if !install_global(Arc::clone(&sup)) {
        return;
    }
    rt.spawn(async move {
        let found = sup.adopt_on_boot().await;
        tracing::info!(children = found.len(), "project supervisor adoption scan done");
        sup.spawn_idle_loop();
    });
}
