//! The launcher's run-dir files and project tokens: `spawn.json`, the user
//! pin, the signed policy, and token issue, refresh and revoke.

use super::*;

impl Launcher {
    /// SHA-256 of the kernel executable (hex), computed once.
    pub(super) fn exe_sha(&self) -> String {
        EXE_SHA
            .get_or_init(|| {
                use sha2::{Digest, Sha256};
                std::fs::read(&self.cfg.exe)
                    .map(|b| hex::encode(Sha256::digest(b)))
                    .unwrap_or_default()
            })
            .clone()
    }

    /// Write the run dir files and file the spawn expectation in the
    /// registry. Returns the spawn nonce.
    pub(super) fn write_run_files(&self, spec: &ChildSpec, token: &str) -> Result<(), RuntimeError> {
        let run_dir = self.run_dir(&spec.project_id);
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| backend(format!("{}: {e}", run_dir.display())))?;
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700));
        }
        let pubkey = self.parts.user_key.verifying_key().to_bytes();
        write_user_pin(&run_dir, &pubkey).map_err(|e| backend(format!("user.pub: {e}")))?;
        let mut snap = (self.parts.snapshot)().ok_or_else(|| {
            RuntimeError::AdmissionRefused(
                "this daemon has no governance engine to export a parent policy from".into(),
            )
        })?;
        if let Some(parent_id) = clawft_types::project::read_project_toml(&spec.root)
            .map_err(|e| RuntimeError::AdmissionRefused(format!("nested project.toml: {e}")))?
            .and_then(|pt| pt.parent)
        {
            let master = clawft_types::project::find_by_id(&self.cfg.manifests_dir, &parent_id)
                .map_err(|e| RuntimeError::AdmissionRefused(format!("master manifest: {e}")))?
                .ok_or_else(|| RuntimeError::AdmissionRefused("master is not registered".into()))?;
            if master.state != clawft_types::project::ProjectState::Active {
                return Err(RuntimeError::AdmissionRefused(
                    "master is not active".into(),
                ));
            }
            let master_root = master
                .root
                .canonicalize()
                .map_err(|e| RuntimeError::AdmissionRefused(format!("master root: {e}")))?;
            let child_root = spec
                .root
                .canonicalize()
                .map_err(|e| RuntimeError::AdmissionRefused(format!("nested child root: {e}")))?;
            if child_root == master_root || !child_root.starts_with(&master_root) {
                return Err(RuntimeError::AdmissionRefused(
                    "child escaped its master root".into(),
                ));
            }
            let master_pt = clawft_types::project::read_project_toml(&master.root)
                .map_err(|e| RuntimeError::AdmissionRefused(format!("master project.toml: {e}")))?
                .ok_or_else(|| {
                    RuntimeError::AdmissionRefused("master has no project.toml".into())
                })?;
            if master_pt.id != parent_id || !master_pt.is_weave_master() {
                return Err(RuntimeError::AdmissionRefused(
                    "master identity or weave.master changed".into(),
                ));
            }
            let signed_parent = export_rules(
                snap.rules.clone(),
                snap.risk_threshold,
                snap.human_approval_required,
                &snap.limits,
                &self.parts.user_key,
                1,
                Utc::now(),
            )
            .map_err(|e| RuntimeError::AdmissionRefused(format!("master policy base: {e}")))?;
            let overlay = load_overlay(&master.root.join(".weftos/overlay.toml"))
                .map_err(|e| RuntimeError::AdmissionRefused(format!("master overlay: {e}")))?;
            let effective = merge_overlay(&signed_parent, &overlay)
                .map_err(|e| RuntimeError::AdmissionRefused(format!("master overlay: {e}")))?;
            snap.risk_threshold = effective.risk_threshold(snap.risk_threshold);
            snap.human_approval_required = effective.human_approval(snap.human_approval_required);
            snap.limits = effective.limits;
            snap.rules = effective.rules;
        }
        export_rules_to(
            &run_dir.join(PARENT_POLICY_FILE),
            snap.rules,
            snap.risk_threshold,
            snap.human_approval_required,
            &snap.limits,
            &self.parts.user_key,
        )
        .map_err(|e| backend(format!("parent-policy.json: {e}")))?;
        let mut nonce = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let now = state::now_unix();
        let nonce = hex::encode(nonce);
        let spawn = SpawnFile::new(
            nonce.clone(),
            self.cfg.parent_socket.clone(),
            hex::encode(pubkey),
            key_id(&pubkey),
            spec.project_id.clone(),
            spec.root.clone(),
            (!token.is_empty()).then(|| token.to_owned()),
            now,
        );
        // A live registry session blocks any new registration (even with a
        // fresh nonce), so a respawn evicts the dead child's first.
        registry().evict(&spec.project_id);
        // The expectation first: if it cannot be filed (ledger full) nothing
        // is written and nothing starts.
        expect_spawn(SpawnExpectation {
            project_id: spec.project_id.clone(),
            nonce,
            pid: None,
            container: None,
            exe_sha: if spec.adapter == super::super::wasmtime::ADAPTER {
                super::super::wasmtime::OperatorConfig::load(&self.cfg, &spec.root)
                    .map_err(backend)?
                    .runner_sha256
            } else {
                self.exe_sha()
            },
            root: spec.root.clone(),
            expires_unix: spawn.expires_unix,
        })
        .map_err(|e| RuntimeError::Backend(format!("spawn ledger: {e}")))?;
        spawn.write(&run_dir.join(SPAWN_JSON_FILE)).map_err(|e| {
            cancel_spawn(&spec.project_id);
            backend(format!("spawn.json: {e}"))
        })
    }

    /// Issue the child's project token (Write only, project-scoped).
    pub(super) fn issue_token(&self, id: &str) -> Result<String, RuntimeError> {
        let Some(auth) = &self.parts.tokens else {
            return Ok(String::new());
        };
        let ttl = chrono::Duration::seconds(PROJECT_TOKEN_TTL_SECS as i64);
        let (secret, info) = auth
            .issue_project(
                id,
                ttl,
                &Issuer {
                    uid: Some(nix::unistd::getuid().as_raw()),
                },
            )
            .map_err(|e| backend(format!("project token: {e}")))?;
        let mut slots = self.token_slots();
        let slot = slots.entry(id.to_owned()).or_default();
        if let Some(old) = slot.previous.take() {
            let _ = auth.revoke(&old);
        }
        slot.previous = slot.current.take();
        slot.current = Some(info.id);
        Ok(secret)
    }

    /// Renew a child's token (`project.token.refresh`). The presented token
    /// must be a live project-scoped token for `id` that this supervisor
    /// issued last (or the one before); the older one is revoked.
    pub async fn refresh_token(
        &self,
        id: &str,
        presented: &str,
    ) -> Result<(String, chrono::DateTime<Utc>), String> {
        use clawft_kernel::token_authority::TokenScope;
        let auth = self.parts.tokens.as_ref().ok_or("no token authority")?;
        let info = auth
            .validate(presented)
            .ok_or("token is unknown, expired or revoked")?;
        if info.scope != TokenScope::Project || info.project.as_deref() != Some(id) {
            return Err("token is not a project token for this project".into());
        }
        {
            let slots = self.token_slots();
            if let Some(slot) = slots.get(id)
                && slot.current.as_deref() != Some(&info.id)
                && slot.previous.as_deref() != Some(&info.id)
            {
                return Err("token is not the one this supervisor issued last".into());
            }
        }
        if !matches!(self.probe_inner(id).await, ChildProbe::Running { .. }) {
            return Err("no verified live child for this project".into());
        }
        let ttl = chrono::Duration::seconds(PROJECT_TOKEN_TTL_SECS as i64);
        let (secret, new) = auth
            .issue_project(
                id,
                ttl,
                &Issuer {
                    uid: Some(nix::unistd::getuid().as_raw()),
                },
            )
            .map_err(|e| e.to_string())?;
        let mut slots = self.token_slots();
        let slot = slots.entry(id.to_owned()).or_default();
        if let Some(old) = slot.previous.take() {
            let _ = auth.revoke(&old);
        }
        slot.previous = slot.current.replace(new.id.clone());
        if slot.previous.is_none() {
            slot.previous = Some(info.id);
        }
        Ok((secret, new.expires_at))
    }

    /// Revoke every token issued for `id`.
    pub fn revoke_tokens(&self, id: &str) {
        let Some(auth) = &self.parts.tokens else {
            return;
        };
        if let Some(slot) = self.token_slots().remove(id) {
            for t in [slot.current, slot.previous].into_iter().flatten() {
                let _ = auth.revoke(&t);
            }
        }
    }
}
