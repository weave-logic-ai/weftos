//! Linux project-container engine operations. Every destructive operation
//! addresses and re-inspects the immutable engine ID, never a name or PID.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clawft_kernel::workload_runtime::container_cmd::{self, CmdOutput, CommandRunner, Engine};
use serde::Deserialize;

const LIMIT: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
/// Executable in every operator-pinned project image. Keep separate from
/// the /weftos directory tree used by bind mounts.
pub const GUEST_EXECUTABLE: &str = "/usr/local/bin/weaver";
pub const GUEST_PROJECT: &str = "/weftos/project";
pub const GUEST_TRUST: &str = "/weftos/trust";
pub const GUEST_LINK: &str = "/weftos/parent";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorConfig {
    /// An image containing the real `weaver` at `/usr/local/bin/weaver`, pinned by digest.
    pub image: String,
    pub engine: String,
}

impl OperatorConfig {
    pub fn load(home: &Path) -> Result<Self, String> {
        let path = home.join(".weftos/project-container.json");
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        for p in [path.parent().unwrap_or(home), path.as_path()] {
            let meta = std::fs::symlink_metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
            if meta.file_type().is_symlink()
                || meta.uid() != nix::unistd::geteuid().as_raw()
                || meta.permissions().mode() & 0o022 != 0
            {
                return Err(format!("{} is not parent-owned and private", p.display()));
            }
        }
        if !std::fs::symlink_metadata(&path)
            .map_err(|e| e.to_string())?
            .is_file()
        {
            return Err(format!("{} is not a regular file", path.display()));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let cfg: Self =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        container_cmd::validate_base_image(&cfg.image).map_err(|e| e.to_string())?;
        cfg.engine_kind()?;
        Ok(cfg)
    }

    pub fn engine_kind(&self) -> Result<Engine, String> {
        match self.engine.as_str() {
            "docker" => Ok(Engine::Docker),
            "podman" => Ok(Engine::Podman),
            _ => Err("project containers require Linux Docker or Podman".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Mounts {
    pub project: PathBuf,
    pub runtime: PathBuf,
    pub trust: PathBuf,
    pub link: PathBuf,
    pub guest_runtime: String,
    /// Stable user-daemon key id, the supervisor across daemon restarts.
    pub supervisor_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspected {
    pub id: String,
    pub running: bool,
    pub host_pid: Option<u32>,
    pub exit_code: Option<i32>,
}

fn valid_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn name(id: &str) -> String {
    format!("weftos-project-{}", id.to_ascii_lowercase())
}

fn mount(src: &Path, dst: &str, readonly: bool) -> String {
    format!(
        "type=bind,src={},dst={}{}",
        src.display(),
        dst,
        if readonly { ",readonly" } else { "" }
    )
}

/// Docker and Podman share these flags. Unsupported flags fail at `create`;
/// there is no weaker fallback.
pub fn create_args(
    cfg: &OperatorConfig,
    project_id: &str,
    m: &Mounts,
    uid: u32,
    gid: u32,
) -> Result<Vec<String>, String> {
    cfg.engine_kind()?;
    if !m.project.is_absolute()
        || !m.runtime.is_absolute()
        || !m.trust.is_absolute()
        || !m.link.is_absolute()
    {
        return Err("project container mounts must be absolute".into());
    }
    // A file cannot be the parent of a mount directory; an ancestor mount
    // would also hide the image's executable. Reject both before engine IO.
    let executable = Path::new(GUEST_EXECUTABLE);
    for target in [GUEST_PROJECT, m.guest_runtime.as_str(), GUEST_TRUST, GUEST_LINK] {
        let target = Path::new(target);
        if !target.is_absolute()
            || target.components().any(|c| matches!(c, std::path::Component::ParentDir))
            || target.starts_with(executable)
            || executable.starts_with(target)
        {
            return Err(format!("guest mount {} conflicts with executable {GUEST_EXECUTABLE}", target.display()));
        }
    }
    let mut v = vec![
        "create".into(),
        "--pull=never".into(),
        "--name".into(),
        name(project_id),
        "--label".into(),
        format!("weftos.project={project_id}"),
        "--label".into(),
        format!("weftos.supervisor={}", m.supervisor_id),
        "--read-only".into(),
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges".into(),
        "--pids-limit".into(),
        "64".into(),
        "--network".into(),
        "none".into(),
        "--user".into(),
        format!("{uid}:{gid}"),
        "--tmpfs".into(),
        "/tmp:rw,nosuid,nodev,size=64m".into(),
        "--mount".into(),
        mount(&m.project, GUEST_PROJECT, false),
        "--mount".into(),
        mount(&m.runtime, &m.guest_runtime, false),
        "--mount".into(),
        mount(&m.trust, GUEST_TRUST, true),
        "--mount".into(),
        mount(&m.link, GUEST_LINK, true),
        "--env".into(),
        format!("WEFTOS_RUNTIME_DIR={}", m.guest_runtime),
        "--env".into(),
        format!("WEFTOS_TRUST_DIR={GUEST_TRUST}"),
        "--env".into(),
        format!("WEFTOS_PROJECT_ID={project_id}"),
        "--env".into(),
        format!("HOME={GUEST_PROJECT}"),
        "--workdir".into(),
        GUEST_PROJECT.into(),
        "--entrypoint".into(),
        GUEST_EXECUTABLE.into(),
    ];
    if cfg.engine == "podman" {
        // Rootless Podman otherwise maps the host-owned private trust files
        // to an unrelated uid inside the guest.
        v.extend(["--userns".into(), "keep-id".into()]);
    }
    v.push(cfg.image.clone());
    v.extend(super::child::child_args(project_id));
    Ok(v)
}

fn checked(out: CmdOutput, op: &str) -> Result<String, String> {
    if out.timed_out || out.status != Some(0) {
        return Err(format!(
            "container {op} failed: {}",
            out.stderr.chars().take(500).collect::<String>()
        ));
    }
    Ok(out.stdout.trim().to_owned())
}

#[derive(Deserialize)]
struct Raw {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "State")]
    state: RawState,
    #[serde(rename = "Config")]
    config: RawConfig,
    #[serde(rename = "HostConfig")]
    host_config: RawHostConfig,
    #[serde(rename = "NetworkSettings")]
    network_settings: RawNetworkSettings,
    #[serde(rename = "Mounts")]
    mounts: Vec<RawMount>,
}
#[derive(Deserialize)]
struct RawState {
    #[serde(rename = "Running")]
    running: bool,
    #[serde(rename = "Pid")]
    pid: u32,
    #[serde(rename = "ExitCode")]
    exit_code: i32,
}
#[derive(Deserialize)]
struct RawConfig {
    #[serde(rename = "Image")]
    image: String,
    #[serde(rename = "Entrypoint")]
    entrypoint: Vec<String>,
    #[serde(rename = "User")]
    user: String,
    #[serde(rename = "Labels")]
    labels: std::collections::HashMap<String, String>,
}
#[derive(Deserialize)]
struct RawHostConfig {
    #[serde(rename = "ReadonlyRootfs")]
    readonly_rootfs: bool,
    #[serde(rename = "Privileged")]
    privileged: bool,
    #[serde(rename = "CapAdd")]
    cap_add: Option<Vec<String>>,
    #[serde(rename = "CapDrop")]
    cap_drop: Vec<String>,
    #[serde(rename = "SecurityOpt")]
    security_opt: Vec<String>,
    #[serde(rename = "PidsLimit")]
    pids_limit: i64,
    #[serde(rename = "NetworkMode")]
    network_mode: String,
}
#[derive(Deserialize)]
struct RawNetworkSettings {
    #[serde(rename = "Networks")]
    networks: serde_json::Value,
}
#[derive(Deserialize)]
struct RawMount {
    #[serde(rename = "Source")]
    source: String,
    #[serde(rename = "Destination")]
    destination: String,
    #[serde(rename = "RW")]
    rw: bool,
}

pub fn parse_inspect(
    text: &str,
    expected_id: &str,
    project_id: &str,
    m: &Mounts,
    expected_image: &str,
) -> Result<Inspected, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("inspect JSON: {e}"))?;
    let one = v.as_array().and_then(|a| a.first()).unwrap_or(&v);
    let r: Raw = serde_json::from_value(one.clone()).map_err(|e| format!("inspect fields: {e}"))?;
    if !valid_id(&r.id) || r.id != expected_id {
        return Err("inspect returned another container ID".into());
    }
    if r.config.image != expected_image {
        return Err("container image differs from the operator pin".into());
    }
    if r.config.entrypoint != [GUEST_EXECUTABLE] {
        return Err("container entrypoint differs from the supervised weaver executable".into());
    }
    let h = &r.host_config;
    if !h.readonly_rootfs
        || h.privileged
        || h.cap_add.as_ref().is_some_and(|v| !v.is_empty())
        || !h.cap_drop.iter().any(|c| c.eq_ignore_ascii_case("all"))
        || !h.security_opt.iter().any(|s| s == "no-new-privileges")
        || !(1..=64).contains(&h.pids_limit)
        || h.network_mode != "none"
        || !match &r.network_settings.networks {
            serde_json::Value::Null => true,
            serde_json::Value::Object(n) => n.keys().all(|k| k == "none"),
            _ => false,
        }
    {
        return Err("container isolation differs from the supervised contract".into());
    }
    let user = format!(
        "{}:{}",
        nix::unistd::geteuid().as_raw(),
        nix::unistd::getegid().as_raw()
    );
    if r.config.user != user {
        return Err("container user differs from the supervisor uid/gid".into());
    }
    if r.config.labels.get("weftos.project").map(String::as_str) != Some(project_id)
        || r.config.labels.get("weftos.supervisor").map(String::as_str)
            != Some(m.supervisor_id.as_str())
    {
        return Err("container labels do not identify this supervised project".into());
    }
    if r.mounts.len() != 4 {
        return Err("container has an additional or missing mount".into());
    }
    for (src, dst, rw) in [
        (&m.project, GUEST_PROJECT, true),
        (&m.runtime, m.guest_runtime.as_str(), true),
        (&m.trust, GUEST_TRUST, false),
        (&m.link, GUEST_LINK, false),
    ] {
        if !r
            .mounts
            .iter()
            .any(|x| x.source == src.to_string_lossy() && x.destination == dst && x.rw == rw)
        {
            return Err(format!(
                "container mount {dst} differs from the supervised contract"
            ));
        }
    }
    Ok(Inspected {
        id: r.id,
        running: r.state.running,
        host_pid: (r.state.running && r.state.pid != 0).then_some(r.state.pid),
        exit_code: (!r.state.running).then_some(r.state.exit_code),
    })
}

pub struct EngineClient {
    pub cfg: OperatorConfig,
    pub runner: Arc<dyn CommandRunner>,
}

impl EngineClient {
    async fn call(&self, args: Vec<String>) -> Result<CmdOutput, String> {
        self.runner
            .run(self.cfg.engine_kind()?.cli(), &args, TIMEOUT, LIMIT)
            .await
            .map_err(|e| e.to_string())
    }
    /// Return the engine ID before any inspection so the supervisor can
    /// durably record it before a later verification error.
    pub async fn create_unverified(
        &self,
        project_id: &str,
        m: &Mounts,
        uid: u32,
        gid: u32,
    ) -> Result<String, String> {
        let id = checked(
            self.call(create_args(&self.cfg, project_id, m, uid, gid)?)
                .await?,
            "create",
        )?;
        if !valid_id(&id) {
            return Err("engine create did not return an immutable 64-hex ID".into());
        }
        Ok(id)
    }
    /// Discover a deterministic-name leftover, but return only after its
    /// immutable ID and full contract have been inspected. Never mutate by name.
    pub async fn inspect_named(&self, project_id: &str, m: &Mounts) -> Result<Inspected, String> {
        let out = self.call(vec!["inspect".into(), name(project_id)]).await?;
        let body = checked(out, "inspect by name")?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let one = v.as_array().and_then(|a| a.first()).unwrap_or(&v);
        let id = one
            .get("Id")
            .and_then(|x| x.as_str())
            .ok_or("inspect has no immutable ID")?;
        parse_inspect(&body, id, project_id, m, &self.cfg.image)
    }
    pub async fn inspect(
        &self,
        id: &str,
        project_id: &str,
        m: &Mounts,
    ) -> Result<Inspected, String> {
        if !valid_id(id) {
            return Err("invalid immutable container ID".into());
        }
        let out = self.call(vec!["inspect".into(), id.into()]).await?;
        parse_inspect(
            &checked(out, "inspect")?,
            id,
            project_id,
            m,
            &self.cfg.image,
        )
    }
    pub async fn start(&self, id: &str, project_id: &str, m: &Mounts) -> Result<Inspected, String> {
        let before = self.inspect(id, project_id, m).await?;
        if before.running {
            return Err("container already running before start".into());
        }
        checked(self.call(vec!["start".into(), id.into()]).await?, "start")?;
        let after = self.inspect(id, project_id, m).await?;
        if !after.running || after.host_pid.is_none() {
            return Err("container did not start with an inspected host PID".into());
        }
        Ok(after)
    }
    pub async fn stop(
        &self,
        id: &str,
        project_id: &str,
        m: &Mounts,
        grace: Duration,
    ) -> Result<(), String> {
        if !self.inspect(id, project_id, m).await?.running {
            return Ok(());
        }
        // A nonzero `stop` can mean its grace elapsed. The next inspected
        // state decides whether a kill is needed; inspection failure blocks
        // every destructive follow-up.
        let _ = self
            .call(container_cmd::stop_cmd(id, grace))
            .await
            .and_then(|out| checked(out, "stop"));
        if self.inspect(id, project_id, m).await?.running {
            // Keep the exited engine record so restart can re-inspect the
            // immutable ID before removing it. `rm -f` would strand the
            // supervisor with a persisted ID that no longer exists.
            self.inspect(id, project_id, m).await?;
            let _ = self
                .call(vec!["kill".into(), id.into()])
                .await
                .and_then(|out| checked(out, "kill"));
            if self.inspect(id, project_id, m).await?.running {
                return Err("container remained running after verified kill".into());
            }
        }
        Ok(())
    }
    pub async fn remove_exited(
        &self,
        id: &str,
        project_id: &str,
        m: &Mounts,
    ) -> Result<(), String> {
        if self.inspect(id, project_id, m).await?.running {
            return Err("container still running".into());
        }
        // Re-inspect immediately before the destructive call.
        if self.inspect(id, project_id, m).await?.running {
            return Err("container restarted during removal".into());
        }
        checked(self.call(vec!["rm".into(), id.into()]).await?, "remove")?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "container_tests.rs"]
mod tests;
