//! Container-backed children: the engine client, mounts, launch and the
//! registration check.

use super::*;

impl Launcher {
    pub(super) fn container_client(
        &self,
        cfg: super::super::container::OperatorConfig,
    ) -> super::super::container::EngineClient {
        super::super::container::EngineClient {
            cfg,
            runner: Arc::new(clawft_kernel::workload_runtime::SystemRunner),
        }
    }

    pub(super) fn container_mounts(&self, id: &str, root: &Path) -> super::super::container::Mounts {
        let run = self.run_dir(id);
        super::super::container::Mounts {
            project: root.to_path_buf(),
            runtime: run.join("guest"),
            trust: run.clone(),
            link: self
                .cfg
                .parent_socket
                .parent()
                .unwrap_or(Path::new("/"))
                .to_path_buf(),
            guest_runtime: format!("/weftos/run/{id}"),
            supervisor_id: key_id(&self.parts.user_key.verifying_key().to_bytes()),
        }
    }

    pub(super) fn require_child_endpoint(&self) -> Result<(), String> {
        validate_child_endpoint(&self.cfg.parent_socket)
    }

    pub(super) async fn launch_container(
        &self,
        spec: &ChildSpec,
        token: &str,
    ) -> Result<ChildRef, RuntimeError> {
        if !cfg!(target_os = "linux") {
            return Err(RuntimeError::AdmissionRefused(
                "Linux container driver is available only on Linux".into(),
            ));
        }
        let id = spec.project_id.as_str();
        self.require_child_endpoint().map_err(backend)?;
        let cfg = super::super::container::OperatorConfig::load(&self.cfg.home).map_err(backend)?;
        let mounts = self.container_mounts(id, &spec.root);
        std::fs::create_dir_all(&mounts.runtime).map_err(backend)?;
        let client = self.container_client(cfg.clone());
        if let Some(old) = state::read(&self.run_dir(id)).and_then(|s| s.container) {
            if old.engine != cfg.engine {
                return Err(backend(
                    "operator engine changed while a container is persisted",
                ));
            }
            client
                .remove_exited(&old.id, id, &mounts)
                .await
                .map_err(backend)?;
        }
        self.write_run_files(spec, token)?;
        let uid = nix::unistd::geteuid().as_raw();
        let gid = nix::unistd::getegid().as_raw();
        let cid = match client.create_unverified(id, &mounts, uid, gid).await {
            Ok(cid) => cid,
            Err(first) => {
                // A previous create may have returned its ID just before a
                // daemon crash. Resolve the occupied name to a verified ID;
                // no operation targets the name after discovery.
                let old = client
                    .inspect_named(id, &mounts)
                    .await
                    .map_err(|_| backend(first))?;
                if old.running {
                    return Err(backend(
                        "a verified container already occupies the project name",
                    ));
                }
                client
                    .remove_exited(&old.id, id, &mounts)
                    .await
                    .map_err(backend)?;
                client
                    .create_unverified(id, &mounts, uid, gid)
                    .await
                    .map_err(backend)?
            }
        };
        let host_socket = mounts.runtime.join(SOCKET_NAME);
        state::update(&self.run_dir(id), |st| {
            st.state = clawft_types::project::ChildState::Starting;
            st.container = Some(super::state::ContainerState {
                engine: cfg.engine.clone(),
                id: cid.clone(),
                host_socket: host_socket.clone(),
            });
            st.pid = None;
            st.started_unix = Some(state::now_unix());
        });
        if state::read(&self.run_dir(id))
            .and_then(|s| s.container)
            .is_none_or(|s| s.id != cid)
        {
            let cleanup = client.remove_exited(&cid, id, &mounts).await;
            return Err(backend(format!(
                "container {cid} identity could not be persisted; verified cleanup: {cleanup:?}"
            )));
        }
        client.inspect(&cid, id, &mounts).await.map_err(backend)?;
        // `create` returns the immutable ID. The guest cannot run until the
        // protected spawn file and nonce ledger both bind that exact ID.
        let spawn_path = self.run_dir(id).join(SPAWN_JSON_FILE);
        let mut spawn: SpawnFile =
            serde_json::from_slice(&std::fs::read(&spawn_path).map_err(backend)?)
                .map_err(backend)?;
        spawn.container = Some(ContainerTransport {
            engine: cfg.engine.clone(),
            container_id: cid.clone(),
            guest_parent_socket: format!("{}/child.sock", super::super::container::GUEST_LINK).into(),
            guest_runtime_root: mounts.guest_runtime.clone().into(),
            guest_trust_root: super::super::container::GUEST_TRUST.into(),
            guest_project_root: super::super::container::GUEST_PROJECT.into(),
            host_child_socket: host_socket.clone(),
        });
        spawn.write(&spawn_path).map_err(backend)?;
        expect_spawn(SpawnExpectation {
            project_id: id.into(),
            nonce: spawn.nonce.clone(),
            pid: None,
            container: Some(clawft_rpc::mesh_local::ContainerRegistration {
                engine: cfg.engine.clone(),
                container_id: cid.clone(),
                host_socket: host_socket.to_string_lossy().into_owned(),
            }),
            exe_sha: self.exe_sha(),
            root: spec.root.clone(),
            expires_unix: spawn.expires_unix,
        })
        .map_err(backend)?;
        let inspected = client.start(&cid, id, &mounts).await.map_err(backend)?;
        let host_pid = inspected
            .host_pid
            .ok_or_else(|| backend("engine did not inspect a host PID"))?;
        let engine = cfg.engine.clone();
        self.procs().insert(
            id.into(),
            Entry {
                proc: Proc::Container {
                    id: cid.clone(),
                    host_pid,
                    cfg,
                    mounts,
                },
                stop: Arc::new(AtomicBool::new(false)),
            },
        );
        self.spawns.fetch_add(1, Ordering::SeqCst);
        Ok(ChildRef {
            project_id: id.into(),
            identity: ChildIdentity::Container {
                engine,
                immutable_container_id: cid,
                host_pid,
            },
        })
    }

    /// Registration is admitted only for a currently inspected, running ID
    /// with the persisted mount contract and parent-selected host socket.
    pub async fn verify_container(
        &self,
        id: &str,
        c: &clawft_rpc::mesh_local::ContainerRegistration,
    ) -> Result<u32, String> {
        self.require_child_endpoint()?;
        let st = state::read(&self.run_dir(id)).ok_or("missing supervisor state")?;
        let saved = st.container.ok_or("no supervised container")?;
        if saved.engine != c.engine
            || saved.id != c.container_id
            || saved.host_socket != Path::new(&c.host_socket)
        {
            return Err("container differs from persisted supervisor identity".into());
        }
        let manifest = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(|e| e.to_string())?
            .ok_or("project manifest missing")?;
        let cfg = super::super::container::OperatorConfig::load(&self.cfg.home)?;
        if cfg.engine != c.engine {
            return Err("operator engine changed".into());
        }
        let mounts = self.container_mounts(id, &manifest.root);
        if saved.host_socket != mounts.runtime.join(SOCKET_NAME) {
            return Err("persisted host socket differs from the supervised runtime".into());
        }
        let inspected = self
            .container_client(cfg)
            .inspect(&c.container_id, id, &mounts)
            .await?;
        if !inspected.running {
            return Err("container is not running".into());
        }
        inspected
            .host_pid
            .ok_or_else(|| "engine supplied no host PID".into())
    }
}
