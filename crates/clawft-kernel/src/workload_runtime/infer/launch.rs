//! Managed-mode launch plan: argv from the spec and the resolved model,
//! environment, working directory, and the "is something already on this
//! port" guard that keeps the adapter from ever starting over a server it
//! does not own.

use std::path::PathBuf;

use super::config::ManagedConfig;
use super::spec::{InferFlavor, InferenceSpec};
use crate::model_manifest::{ModelFormat, ResolvedModel};
use crate::workload_runtime::types::RuntimeError;

/// A launch ready to spawn.
#[derive(Debug, Clone)]
pub struct Launch {
    /// Launcher script.
    pub program: PathBuf,
    /// Arguments (argv, never a shell string).
    pub args: Vec<String>,
    /// Working directory (created at load).
    pub dir: PathBuf,
    /// Full environment.
    pub env: Vec<(String, String)>,
}

fn refuse(m: impl Into<String>) -> RuntimeError {
    RuntimeError::AdmissionRefused(m.into())
}

/// The model must be in a format the runtime loads.
pub fn check_format(flavor: InferFlavor, m: &ResolvedModel) -> Result<(), RuntimeError> {
    let ok = match flavor {
        InferFlavor::LlamaCpp => m.format == ModelFormat::Gguf,
        InferFlavor::MlxLm => matches!(m.format, ModelFormat::Mlx | ModelFormat::Safetensors),
        InferFlavor::Ollama => m.format == ModelFormat::Ollama,
    };
    if ok {
        Ok(())
    } else {
        Err(refuse(format!(
            "{} cannot load a {} model ({})",
            flavor.id(),
            m.format.as_str(),
            m.name
        )))
    }
}

/// Where the server reads weights from: the first GGUF shard (llama.cpp
/// finds the rest) or the model directory (MLX).
fn weights_arg(flavor: InferFlavor, m: &ResolvedModel) -> Result<String, RuntimeError> {
    let p = match flavor {
        InferFlavor::LlamaCpp => m
            .shards
            .first()
            .cloned()
            .ok_or_else(|| refuse("model has no shards"))?,
        _ => m.root.clone(),
    };
    p.to_str()
        .map(str::to_string)
        .ok_or_else(|| refuse("model path is not UTF-8"))
}

/// Build the launch for a llama.cpp or mlx-lm instance. The argument shape
/// follows the model lab's launchers (`serve-llamacpp <model.gguf>
/// [--draft m] [--kv q] [--ctx n] [--port p] [--host h]`, `serve <model>
/// --port p --host h`); it lives in this one function.
pub fn build_launch(
    flavor: InferFlavor,
    spec: &InferenceSpec,
    model: &ResolvedModel,
    draft: Option<&ResolvedModel>,
    cfg: &ManagedConfig,
    instance_id: &str,
) -> Result<Launch, RuntimeError> {
    check_format(flavor, model)?;
    let program = cfg.serve_program.clone().ok_or_else(|| {
        refuse(format!(
            "{} managed mode needs a serve program",
            flavor.id()
        ))
    })?;
    let mut args = vec![weights_arg(flavor, model)?];
    args.extend(["--port".into(), spec.port()?.to_string()]);
    args.extend(["--host".into(), spec.bind_ip()?.to_string()]);
    if flavor == InferFlavor::LlamaCpp {
        if let Some(c) = spec.serve.ctx {
            args.extend(["--ctx".into(), c.to_string()]);
        }
        if let Some(q) = &spec.serve.kv_quant {
            args.extend(["--kv".into(), q.clone()]);
        }
        if let Some(d) = draft {
            check_format(flavor, d)?;
            args.extend(["--draft".into(), weights_arg(flavor, d)?]);
        }
    }
    args.extend(spec.serve.extra_args.iter().cloned());
    let mut env = cfg.env.clone();
    for name in &cfg.env_passthrough {
        if let Ok(v) = std::env::var(name) {
            env.push((name.clone(), v));
        }
    }
    Ok(Launch {
        program,
        args,
        dir: cfg.data_root.join(instance_id),
        env,
    })
}
