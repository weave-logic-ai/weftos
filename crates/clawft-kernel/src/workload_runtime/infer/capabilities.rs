//! Capability probes: what a node may advertise after checking the runtime
//! is really there (ADR-101 section 4).

use std::time::Duration;

use clawft_types::placement::{AttrValue, Capability, CapabilityId, Provenance};

use super::config::{InferConfig, InferMode};
use super::probe::{Health, ServerClient};
use super::spec::InferFlavor;

fn cap(id: &str) -> Option<Capability> {
    CapabilityId::new(id)
        .ok()
        .map(|id| Capability::new(id, Provenance::Probed))
}

fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

async fn run_version(prog: &std::path::Path, args: &[String]) -> Option<String> {
    let mut cmd = tokio::process::Command::new(prog);
    cmd.args(args)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(5), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Some(
        text.lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(120)
            .collect(),
    )
}

/// Probe and build the capability list. Empty when the runtime is not
/// there: a node claims nothing it did not just check.
pub async fn probe(cfg: &InferConfig, probe_port: u16) -> Vec<Capability> {
    let mut version: Option<String> = None;
    let present = match (&cfg.mode, cfg.flavor) {
        (InferMode::Managed(m), f) if f != InferFlavor::Ollama => {
            if !m.serve_program.as_deref().is_some_and(is_executable) {
                return Vec::new();
            }
            match &m.version_probe {
                Some((p, a)) => match run_version(p, a).await {
                    Some(v) => {
                        version = Some(v);
                        true
                    }
                    None => false,
                },
                None => true,
            }
        }
        _ => {
            let Ok(client) =
                ServerClient::new(format!("http://127.0.0.1:{probe_port}"), cfg.probe_timeout)
            else {
                return Vec::new();
            };
            let r = client.probe(cfg.flavor).await;
            version = r.version.clone();
            !matches!(r.health, Health::Unreachable(_))
        }
    };
    if !present {
        return Vec::new();
    }
    let mode = match cfg.mode {
        InferMode::Adopted => "adopted",
        InferMode::Managed(_) => "managed",
    };
    let formats: Vec<AttrValue> = cfg
        .flavor
        .formats()
        .iter()
        .map(|f| AttrValue::from(*f))
        .collect();
    let mut runtime = match cap(cfg.flavor.capability()) {
        Some(c) => c
            .with_attr("mode", mode)
            .with_attr("formats", AttrValue::List(formats)),
        None => return Vec::new(),
    };
    if let Some(v) = &version {
        runtime = runtime.with_attr("version", v.as_str());
    }
    let mut out = vec![runtime];
    for f in cfg.flavor.formats() {
        out.extend(cap(&format!("format.{f}")));
    }
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        out.extend(cap("accel.gpu.metal"));
    }
    out
}
