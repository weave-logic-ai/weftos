//! Explicit persistent Wasmtime project driver. No artifact comes from a project manifest.
//! Operator configuration and launch receipts live outside the guest preopen.
use super::{SupError, Supervisor, SupervisorConfig, WorkloadHost, adopt, state};
use clawft_types::project::canon::hex_encode;
use clawft_types::project::{ProjectManifest, ProjectSandbox, SpawnFile};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

// One verifier shared with the runner crate, without linking Wasmtime into weaver.
#[path = "../../../clawft-wasm-host/src/project_kernel/adoption.rs"]
mod proof;

pub const ADAPTER: &str = "wasmtime-project-v1";
const RECEIPT: &str = "wasmtime-receipt.json";
type Result<T> = std::result::Result<T, String>;

/// Private operator-owned <home>/.weftos/project-wasmtime.json. Both artifacts
/// are pinned; changing a pin deliberately refuses adoption of the old runner.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorConfig {
    pub adapter: String,
    pub runner: PathBuf,
    pub runner_sha256: String,
    pub artifact: PathBuf,
    pub artifact_sha256: String,
    pub lifetime_fuel: u64,
    pub memory_bytes: usize,
    pub lifetime_secs: u64,
}
fn digest(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}
fn private(path: &Path, dir: bool) -> Result<()> {
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !path.is_absolute()
        || path.canonicalize().map_err(|e| e.to_string())? != path
        || m.file_type().is_symlink()
        || m.uid() != nix::unistd::getuid().as_raw()
        || m.mode() & 0o077 != 0
        || (if dir { !m.is_dir() } else { !m.is_file() })
    {
        return Err(format!(
            "not a canonical private {}: {}",
            if dir { "directory" } else { "file" },
            path.display()
        ));
    }
    Ok(())
}
fn read_private(path: &Path) -> Result<Vec<u8>> {
    private(path, false)?;
    let mut bytes = Vec::new();
    fs::OpenOptions::new().read(true).custom_flags(nix::libc::O_NOFOLLOW).open(path)
        .map_err(|e| e.to_string())?.take(64 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("oversize private configuration".into());
    }
    Ok(bytes)
}
fn has_receipt(run: &Path) -> bool {
    // Includes broken symlinks; a malformed Wasmtime record never falls back.
    !matches!(fs::symlink_metadata(run.join(RECEIPT)), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}
pub fn selected(manifest: &ProjectManifest, run: &Path) -> Result<&'static str> {
    selected_serve(&manifest.serve.clone().unwrap_or_default(), run)
}
fn selected_serve(serve: &clawft_types::project::ServeSection, run: &Path) -> Result<&'static str> {
    match (serve.sandbox, serve.adapter.as_deref()) {
        (ProjectSandbox::Wasmtime, Some(ADAPTER)) if cfg!(feature = "wasmtime-project") => Ok(ADAPTER),
        // Seatbelt and container projects run the native kernel through their own drivers.
        (
            ProjectSandbox::Logical | ProjectSandbox::Seatbelt | ProjectSandbox::LinuxContainer,
            None | Some("logical"),
        ) if !has_receipt(run) => Ok("logical"),
        _ => Err("unsupported or mismatched project sandbox/adapter; no fallback (Wasmtime requires the wasmtime-project feature)".into()),
    }
}
impl OperatorConfig {
    pub fn load(cfg: &SupervisorConfig, root: &Path) -> Result<Self> {
        let dir = cfg.home.join(".weftos");
        private(&dir, true)?;
        for protected in [&dir, &cfg.run_root, &cfg.manifests_dir] {
            if protected.starts_with(root) || root.starts_with(protected) {
                return Err("project root overlaps supervisor authority paths".into());
            }
        }
        let c: Self =
            serde_json::from_slice(&read_private(&dir.join("project-wasmtime.json"))?).map_err(|e| e.to_string())?;
        if c.adapter != ADAPTER
            || !(1..=10_000_000_000_000).contains(&c.lifetime_fuel)
            || !(16 * 1024 * 1024..=512 * 1024 * 1024).contains(&c.memory_bytes)
            || !(1..=604800).contains(&c.lifetime_secs)
        {
            return Err("invalid Wasmtime adapter/resource bounds".into());
        }
        for (path, hash) in [(&c.runner, &c.runner_sha256), (&c.artifact, &c.artifact_sha256)] {
            if !path.is_absolute()
                || path.starts_with(root)
                || hash.len() != 64
                || !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(
                    "artifacts must be operator-owned absolute paths outside the project with SHA256 pins".into(),
                );
            }
        }
        Ok(c)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    adapter: String,
    project_id: String,
    root: PathBuf,
    runner: PathBuf,
    runner_sha256: String,
    artifact: PathBuf,
    artifact_sha256: String,
    parent_socket: PathBuf,
    user_pubkey: String,
}
fn receipt(run: &Path) -> Result<Receipt> {
    private(run, true)?;
    serde_json::from_slice(&read_private(&run.join(RECEIPT))?).map_err(|e| e.to_string())
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    clawft_kernel::parent_policy::write_atomic_0600(path, bytes).map_err(|e| e.to_string())
}
fn pinned(path: &Path, hash: &str) -> Result<Vec<u8>> {
    // Hash the opened descriptor then stage those exact bytes: no hash/exec race.
    if path.canonicalize().map_err(|e| e.to_string())? != path {
        return Err("artifact symlink refused".into());
    }
    let f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if !m.is_file() || m.uid() != nix::unistd::getuid().as_raw() || m.mode() & 0o022 != 0 {
        return Err("artifact is not an operator-owned regular file".into());
    }
    let mut bytes = Vec::new();
    f.take(128 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 128 * 1024 * 1024 || digest(&bytes) != hash {
        return Err("artifact SHA256 mismatch or oversize".into());
    }
    Ok(bytes)
}
fn stage(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(if executable { 0o700 } else { 0o600 })
        .open(path)
    {
        Ok(mut f) => {
            f.write_all(bytes)
                .and_then(|_| f.sync_all())
                .map_err(|e| e.to_string())?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            private(path, false)?;
            if fs::read(path).map_err(|e| e.to_string())? != bytes {
                return Err("staged artifact differs".into());
            }
        }
        Err(e) => return Err(e.to_string()),
    }
    Ok(())
}
/// Consume the parent's expiring nonce and build the runner's exact core ABI launch.
/// Called after write_run_files and before spawning; never inherits HOME or PATH.
pub fn launch(
    cfg: &SupervisorConfig,
    spec: &clawft_kernel::workload_runtime::ChildSpec,
    run: &Path,
    user: &str,
) -> Result<(PathBuf, PathBuf)> {
    let c = OperatorConfig::load(cfg, &spec.root)?;
    private(run, true)?;
    if run.starts_with(&spec.root) || spec.root.starts_with(run) {
        return Err("guest/runner paths overlap".into());
    }
    if adopt::lock_held(&run.join("kernel.lock")) {
        return Err("runner lock already held".into());
    }
    if let Ok(pid) = fs::read_to_string(run.join("kernel.pid")) {
        let pid = pid.trim().parse::<u32>().map_err(|_| "invalid runner PID")?;
        if pid <= 1 || pid > i32::MAX as u32 || super::child::pid_alive(pid) {
            return Err("unadopted live runner; refusing replacement".into());
        }
    }
    if has_receipt(run) {
        validate(cfg, &spec.project_id, &spec.root, run, user)?;
    }
    let state = spec.root.join(".weftos");
    private(&state, true)?;
    let runner = run.join(format!("wasmtime-runner-{}", c.runner_sha256));
    let artifact = run.join(format!("wasmtime-guest-{}.wasm", c.artifact_sha256));
    stage(&runner, &pinned(&c.runner, &c.runner_sha256)?, true)?;
    stage(&artifact, &pinned(&c.artifact, &c.artifact_sha256)?, false)?;
    let spawn = SpawnFile::read_and_consume(&run.join("spawn.json"), state::now_unix()).map_err(|e| e.to_string())?;
    if spawn.project_id != spec.project_id
        || spawn.root != spec.root
        || spawn.parent_socket != cfg.parent_socket
        || spawn.user_pubkey != user
    {
        return Err("spawn binding mismatch".into());
    }
    let parent_policy: Value = serde_json::from_slice(&read_private(
        &run.join(clawft_types::runtime_paths::PARENT_POLICY_FILE),
    )?)
    .map_err(|e| e.to_string())?;
    let r = Receipt {
        adapter: ADAPTER.into(),
        project_id: spec.project_id.clone(),
        root: spec.root.clone(),
        runner: runner.clone(),
        runner_sha256: c.runner_sha256,
        artifact: artifact.clone(),
        artifact_sha256: c.artifact_sha256.clone(),
        parent_socket: cfg.parent_socket.clone(),
        user_pubkey: user.into(),
    };
    write_private(&run.join(RECEIPT), &serde_json::to_vec(&r).map_err(|e| e.to_string())?)?;
    let config = json!({"adapter":ADAPTER,"artifact":artifact,"artifact_sha256":c.artifact_sha256,
        "project_root":spec.root,"parent_socket":cfg.parent_socket,"runtime_dir":run,"project_id":spec.project_id,
        "user_pubkey":user,"spawn_nonce":spawn.nonce,"parent_policy":parent_policy,"depth":1,"parent":"user",
        "lifetime_fuel":c.lifetime_fuel,"memory_bytes":c.memory_bytes,"lifetime_secs":c.lifetime_secs});
    let path = run.join("wasmtime-launch.json");
    write_private(&path, &serde_json::to_vec(&config).map_err(|e| e.to_string())?)?;
    // Dead runner only, after validating authority. Never unlink a live listener.
    for file in ["kernel.sock", "kernel.pid"] {
        match fs::remove_file(run.join(file)) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok((runner, path))
}
fn validate(cfg: &SupervisorConfig, id: &str, root: &Path, run: &Path, user: &str) -> Result<Receipt> {
    let r = receipt(run)?;
    let c = OperatorConfig::load(cfg, root)?;
    if r.adapter != ADAPTER
        || r.project_id != id
        || r.root != root
        || r.parent_socket != cfg.parent_socket
        || r.user_pubkey != user
        || r.runner_sha256 != c.runner_sha256
        || r.artifact_sha256 != c.artifact_sha256
        || r.runner != run.join(format!("wasmtime-runner-{}", c.runner_sha256))
        || r.artifact != run.join(format!("wasmtime-guest-{}.wasm", c.artifact_sha256))
    {
        return Err("Wasmtime receipt differs from operator/project authority".into());
    }
    pinned(&r.runner, &r.runner_sha256)?;
    pinned(&r.artifact, &r.artifact_sha256)?;
    Ok(r)
}
/// Synchronous OS identity guard for signal delivery after signed adoption.
pub fn identity(run: &Path, pid: u32) -> bool {
    let Ok(r) = receipt(run) else { return false };
    if !adopt::identity_ok(run, pid, &r.runner) || pinned(&r.runner, &r.runner_sha256).is_err() {
        return false;
    }
    #[cfg(target_os = "linux")]
    let actual = fs::read_link(format!("/proc/{pid}/exe")).ok();
    #[cfg(not(target_os = "linux"))]
    let actual = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    actual.as_deref() == Some(r.runner.as_path())
}
pub fn is_wasm_run(run: &Path) -> bool {
    has_receipt(run)
}

impl Supervisor {
    pub(super) fn selected_adapter(&self, id: &str) -> std::result::Result<&'static str, SupError> {
        let m = clawft_types::project::find_by_id(&self.cfg.manifests_dir, id)
            .map_err(|e| SupError::Identity(e.to_string()))?
            .ok_or_else(|| SupError::NotRegistered(id.to_owned()))?;
        selected(&m, &self.run_dir(id)).map_err(SupError::Identity)
    }
    pub(super) fn driver_host(&self, adapter: &str) -> std::result::Result<&WorkloadHost, SupError> {
        match adapter {
            "logical" => Ok(&self.host),
            ADAPTER if cfg!(feature = "wasmtime-project") => Ok(&self.wasm_host),
            _ => Err(SupError::Identity("unsupported project adapter".into())),
        }
    }
    pub(super) async fn wasm_proof(&self, id: &str, pid: u32) -> Result<Value> {
        if self.selected_adapter(id).map_err(|e| e.to_string())? != ADAPTER {
            return Err("Wasmtime selection required".into());
        }
        let (w, _) = self.prepare(id).map_err(|e| e.to_string())?;
        let clawft_kernel::workload_runtime::WorkloadSource::Project(p) = w.source else {
            return Err("not project".into());
        };
        let user = self.deps.cert_env.user_key.verifying_key().to_bytes();
        let run = self.run_dir(id);
        let r = validate(&self.cfg, id, &p.root, &run, &hex_encode(&user))?;
        if !identity(&run, pid) {
            return Err("runner PID/executable/lock mismatch".into());
        }
        let view = crate::project_cert_rpc::current_view(&self.deps.cert_env).map_err(|e| e.to_string())?;
        let cert = view.current_cert(id).ok_or("no certified project identity")?;
        let mut nonce = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let nonce = hex_encode(&nonce);
        let socket = self.socket(id);
        let reply = self
            .deps
            .io
            .wasm_handshake(&socket, &nonce)
            .await
            .ok_or("signed handshake unavailable")?;
        proof::verify_adoption(
            &reply,
            &nonce,
            cert,
            &user,
            &proof::Expected {
                project_id: id,
                pid,
                socket: socket.to_str().ok_or("non-UTF8 socket")?,
                root_sha256: &digest(p.root.as_os_str().as_encoded_bytes()),
                artifact_sha256: &r.artifact_sha256,
            },
        )?;
        Ok(reply)
    }
    /// Valid project ids with a runtime directory, sorted.
    pub(super) fn run_ids(&self) -> Vec<String> {
        let Ok(rd) = fs::read_dir(&self.cfg.run_root) else { return Vec::new() };
        let mut ids: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|id| clawft_types::project::validate_id(id).is_ok())
            .collect();
        ids.sort();
        ids
    }
    pub(super) async fn scan_project(&self, id: &str) -> Option<adopt::Found> {
        let run = self.run_dir(id);
        // Only an explicit Wasmtime selection or a Wasmtime receipt leaves the native scan.
        if !has_receipt(&run) && self.selected_adapter(id).ok() != Some(ADAPTER) {
            return adopt::scan_one(&run, id, &self.cfg.exe, self.deps.io.as_ref()).await;
        }
        let pid_text = match fs::read_to_string(run.join("kernel.pid")) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !adopt::lock_held(&run.join("kernel.lock")) => {
                return None;
            }
            Err(e) => {
                return Some(adopt::Found::Unverifiable {
                    id: id.into(),
                    pid: None,
                    reason: adopt::Skip::BadPidFile(e.to_string()),
                });
            }
        };
        let pid = pid_text
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|p| *p > 1 && *p <= i32::MAX as u32);
        let reason = match pid {
            Some(pid) if !super::child::pid_alive(pid) => Some(adopt::Skip::Dead),
            Some(pid) => match self.wasm_proof(id, pid).await {
                Ok(_) => None,
                Err(e) => Some(adopt::Skip::HandshakeFailed(e)),
            },
            None => Some(adopt::Skip::BadPidFile("invalid PID".into())),
        };
        Some(match reason {
            Some(reason) => adopt::Found::Unverifiable {
                id: id.into(),
                pid,
                reason,
            },
            None => adopt::Found::Adopted {
                id: id.into(),
                pid: pid.unwrap(),
            },
        })
    }
}

/// Establish private supervisor storage before the common spawn writer runs.
pub fn preflight(cfg: &SupervisorConfig, root: &Path, run: &Path) -> Result<()> {
    OperatorConfig::load(cfg, root)?;
    private(&cfg.run_root, true)?;
    if run.parent() != Some(cfg.run_root.as_path()) || run.starts_with(root) {
        return Err("unsafe runtime path".into());
    }
    match fs::DirBuilder::new().mode(0o700).create(run) {
        Ok(()) => fs::set_permissions(run, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.to_string()),
    }
    private(run, true)?;
    if adopt::lock_held(&run.join("kernel.lock")) { return Err("runner lock held before spawn preparation".into()); }
    match fs::read_to_string(run.join("kernel.pid")) {
        Ok(p) => {
            let pid = p.trim().parse::<u32>().map_err(|_| "invalid existing PID")?;
            if pid <= 1 || pid > i32::MAX as u32 || super::child::pid_alive(pid) { return Err("live/unverifiable existing runner".into()); }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string()),
    }
    private(&root.join(".weftos"), true)
}

pub fn validate_log(run: &Path) -> Result<()> {
    let path = run.join("kernel.log");
    match fs::symlink_metadata(&path) {
        Ok(_) => private(&path, false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests;
