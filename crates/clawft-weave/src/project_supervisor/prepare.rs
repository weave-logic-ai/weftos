//! Spawn preparation: the refusals checked before a child starts and the
//! workload the `logical` adapter runs.

use super::*;

impl Supervisor {
    /// The spawn refusals plus the workload the `logical` adapter runs.
    pub(super) fn prepare(&self, id: &str) -> Result<(VerifiedWorkload, ProjectManifest), SupError> {
        clawft_types::project::validate_id(id).map_err(|e| SupError::InvalidId(e.to_string()))?;
        let manifest = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(|e| SupError::Identity(e.to_string()))?
            .ok_or_else(|| SupError::NotRegistered(id.to_owned()))?;
        if manifest.is_workspace() {
            return Err(SupError::Workspace(id.to_owned()));
        }
        let root = manifest.root.clone();
        let home = self
            .cfg
            .home
            .canonicalize()
            .unwrap_or_else(|_| self.cfg.home.clone());
        let canon = root
            .canonicalize()
            .map_err(|_| SupError::RootMissing(root.clone()))?;
        if canon == home || canon == Path::new("/") {
            return Err(SupError::RootIsHome(canon));
        }
        if !canon.is_dir() {
            return Err(SupError::RootMissing(root));
        }
        let project = clawft_types::project::read_project_toml(&canon)
            .ok()
            .flatten();
        let found = project.as_ref().map(|p| p.id.clone());
        if found.as_deref() != Some(id) {
            return Err(SupError::IdMismatch {
                manifest: id.to_owned(),
                found,
            });
        }
        if let Some(parent_id) = project.as_ref().and_then(|p| p.parent.as_deref()) {
            self.check_nested_parent(id, parent_id, &canon)?;
        }
        let sandbox = manifest
            .serve
            .as_ref()
            .map_or(ProjectSandbox::Logical, |s| s.sandbox);
        if sandbox == ProjectSandbox::LinuxContainer {
            // A read-only bind is ineffective when the same files are also
            // reachable through the project's writable bind.
            let protected_paths = [
                self.cfg.run_root.clone(),
                self.cfg.manifests_dir.clone(),
                self.cfg.home.join(".weftos"),
            ];
            if let Some(protected) = overlapping_parent_path(&canon, &protected_paths) {
                return Err(SupError::Identity(format!(
                    "container project root overlaps parent-controlled path {}",
                    protected.display()
                )));
            }
        }
        let sock = if sandbox == ProjectSandbox::LinuxContainer {
            self.run_dir(id).join("guest").join(SOCKET_NAME)
        } else {
            self.socket(id)
        };

        if sock.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(SupError::SocketPathTooLong(sock));
        }
        let legacy = canon.join(".weftos").join("runtime").join(LOCK_FILE_NAME);
        if adopt::lock_held(&legacy) {
            return Err(SupError::LegacyDaemonRunning(legacy));
        }
        let view = crate::project_cert_rpc::current_view(&self.deps.cert_env)
            .map_err(|e| SupError::Identity(e.kind().to_owned() + ": " + &e.to_string()))?;
        let cert = view.current_cert(id).cloned();
        // Revoke is terminal: the marker is never lifted by the supervisor.
        // The journal says the same: a project whose key was revoked and
        // that has no certificate in force is refused even when the marker
        // is missing (a full disk, a hand-removed file), instead of being
        // spawned just to die on `project_revoked` and burn its restart
        // budget.
        if state::is_marked_revoked(&self.cfg.run_root, id)
            || (cert.is_none() && view.was_revoked(id))
        {
            return Err(SupError::Revoked(id.to_owned()));
        }
        let upub = self.deps.cert_env.user_key.verifying_key().to_bytes();
        let facts = ProjectFacts {
            cert: cert.as_ref(),
            user_pubkey: &upub,
            revocations: &view,
            manifest_id: id,
            root: &canon,
            policy_hash: "",
        };
        let mut w = prepare_project(&facts).map_err(|e| match e {
            ProjectPrepareError::Identity(m) => SupError::Identity(m),
            other => SupError::Identity(other.to_string()),
        })?;
        let adapter = wasmtime::selected(&manifest, &self.run_dir(id)).map_err(SupError::Identity)?;
        if adapter == wasmtime::ADAPTER {
            wasmtime::OperatorConfig::load(&self.cfg, &canon).map_err(SupError::Identity)?;
        }
        if let clawft_kernel::workload_runtime::WorkloadSource::Project(p) = &mut w.source {
            p.adapter = adapter.into();
        }
        Ok((w, manifest))
    }

    /// A nested project may be supervised only under a registered master
    /// whose canonical root contains it. This prevents a forged `parent`
    /// field from claiming another project's authority or escaping its tree.
    pub(super) fn check_nested_parent(&self, id: &str, parent_id: &str, root: &Path) -> Result<(), SupError> {
        clawft_types::project::validate_id(parent_id)
            .map_err(|e| SupError::Nested(e.to_string()))?;
        if parent_id == id {
            return Err(SupError::Nested("project cannot parent itself".into()));
        }
        let parent = clawft_types::project::find_by_id(&self.cfg.manifests_dir, parent_id)
            .map_err(|e| SupError::Nested(e.to_string()))?
            .ok_or_else(|| SupError::Nested(format!("master {parent_id} is not registered")))?;
        if parent.state != clawft_types::project::ProjectState::Active {
            return Err(SupError::Nested(format!(
                "master {parent_id} is not active"
            )));
        }
        let parent_root = parent
            .root
            .canonicalize()
            .map_err(|e| SupError::Nested(format!("master root: {e}")))?;
        if root == parent_root || !root.starts_with(&parent_root) {
            return Err(SupError::Nested(format!(
                "{} is outside master root {}",
                root.display(),
                parent_root.display()
            )));
        }
        let parent_toml = clawft_types::project::read_project_toml(&parent_root)
            .map_err(|e| SupError::Nested(e.to_string()))?
            .ok_or_else(|| SupError::Nested("master has no project.toml".into()))?;
        if parent_toml.id != parent_id || !parent_toml.is_weave_master() {
            return Err(SupError::Nested(format!(
                "{parent_id} has not enabled weave.master"
            )));
        }
        Ok(())
    }
}
