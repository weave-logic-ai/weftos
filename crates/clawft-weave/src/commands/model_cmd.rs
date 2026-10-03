//! `weaver model adopt | list | verify | explain` (card mesh-placement-17).
//!
//! Local verbs (no daemon): they work on the node's model registry
//! (`<runtime dir>/models/registry.json`, or `--registry`). Adoption hashes
//! existing HF cache / MLX / GGUF / Ollama files where they lie and records
//! an operator attestation; nothing is copied and nothing is shared unless
//! `--redistributable` is given. The daemon advertises `model.present` from
//! the same registry on its next full facts probe.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use clawft_kernel::model_manifest::{
    AdoptInput, CheckMode, FileOutcome, LocalityDecision, ModelFormat, ModelRegistry, ModelSource,
    ModelState, NodeHolding, Sharing, StoreTier, TierResolver, TransferPolicy, decide, scan_dir,
    scan_file, scan_ollama,
};
use clawft_kernel::workload_pkg::{TrustAnchors, key_id_for, signing_key_from_hex};

/// `weaver model` arguments.
#[derive(Args, Debug)]
pub struct ModelArgs {
    /// Subcommand.
    #[command(subcommand)]
    pub command: ModelCommand,
    /// Registry file (default: `<runtime dir>/models/registry.json`).
    #[arg(long, global = true)]
    pub registry: Option<PathBuf>,
}

/// `weaver model` subcommands.
#[derive(Subcommand, Debug)]
pub enum ModelCommand {
    /// Adopt model files in place: hash them where they lie and attest the hashes.
    Adopt(AdoptArgs),
    /// List adopted models with their current state.
    List,
    /// Check an adopted model's files against its attestation.
    Verify {
        /// Model name or package id.
        model: String,
        /// Re-hash every file, not only those whose size or mtime changed.
        #[arg(long)]
        full: bool,
    },
    /// Explain the fetch-versus-relocate decision for a model over described nodes.
    Explain(ExplainArgs),
}

/// Weight layout.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum FormatArg {
    /// MLX quantisation directory.
    Mlx,
    /// GGUF file or directory.
    Gguf,
    /// Safetensors (HF snapshot).
    Safetensors,
}

/// `weaver model adopt`.
#[derive(Args, Debug)]
pub struct AdoptArgs {
    /// Model directory or single weight file; for `--ollama`, the Ollama models directory.
    pub path: PathBuf,
    /// Model name (the `model` parameter of `perf.infer.tok_s`).
    #[arg(long)]
    pub name: String,
    /// Layout (ignored with `--ollama`).
    #[arg(long, value_enum, default_value = "mlx")]
    pub format: FormatArg,
    /// Adopt through an Ollama manifest, `<name>:<tag>`.
    #[arg(long, value_name = "NAME:TAG")]
    pub ollama: Option<String>,
    /// Hugging Face repo id (provenance).
    #[arg(long)]
    pub hf_repo: Option<String>,
    /// Hugging Face revision (provenance).
    #[arg(long)]
    pub hf_revision: Option<String>,
    /// Operator signing key file (64 hex chars; see `weaver workload keygen`).
    #[arg(long)]
    pub key: PathBuf,
    /// Trust file pinning the operator key (weftos.workload-trust.v1).
    #[arg(long)]
    pub trust: PathBuf,
    /// Explicitly opt in to sharing the weights with other nodes.
    #[arg(long)]
    pub redistributable: bool,
    /// Replace an existing model of the same name.
    #[arg(long)]
    pub replace: bool,
}

/// `weaver model explain`.
#[derive(Args, Debug)]
pub struct ExplainArgs {
    /// Model name or package id.
    pub model: String,
    /// A node: `<id>=complete|none[:<free GiB>][,ineligible]` (repeat).
    #[arg(long = "node", value_name = "SPEC", required = true)]
    pub nodes: Vec<String>,
    /// Allow fetching weights from another node.
    #[arg(long)]
    pub allow_transfer: bool,
    /// Largest transfer, in GiB (default: no ceiling).
    #[arg(long)]
    pub max_gib: Option<u64>,
}

fn registry_path(args: &ModelArgs) -> PathBuf {
    args.registry
        .clone()
        .unwrap_or_else(|| clawft_rpc::runtime_dir().join("models/registry.json"))
}

fn load_trust(path: &Path) -> anyhow::Result<TrustAnchors> {
    TrustAnchors::from_trust_json(&std::fs::read(path)?).map_err(|e| anyhow::anyhow!(e))
}

fn adopt(reg: &ModelRegistry, a: AdoptArgs) -> anyhow::Result<()> {
    let anchors = load_trust(&a.trust)?;
    let key = signing_key_from_hex(&std::fs::read_to_string(&a.key)?).map_err(anyhow::Error::msg)?;
    let key_id = key_id_for(&key.verifying_key().to_bytes());
    let format = match a.format {
        FormatArg::Mlx => ModelFormat::Mlx,
        FormatArg::Gguf => ModelFormat::Gguf,
        FormatArg::Safetensors => ModelFormat::Safetensors,
    };
    let input = AdoptInput {
        name: a.name.clone(),
        format,
        source: ModelSource {
            hf_repo: a.hf_repo.clone(),
            hf_revision: a.hf_revision.clone(),
            ollama_tag: None,
        },
        redistributable: a.redistributable,
    };
    eprintln!("hashing in place (nothing is copied) ...");
    let scanned = if let Some(spec) = &a.ollama {
        let (n, t) = spec
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("--ollama expects <name>:<tag>"))?;
        scan_ollama(&a.path, n, t, input)?
    } else if a.path.is_file() {
        scan_file(&a.path, input)?
    } else {
        scan_dir(&a.path, input)?
    };
    let bytes = scanned.body.total_bytes();
    let shards = scanned.body.shards.len();
    let adopted = reg.adopt(scanned, &key, &key_id, &anchors, a.replace)?;
    println!("adopted {} ({shards} shard(s), {bytes} bytes)", a.name);
    println!("package id {}", adopted.package_id);
    println!(
        "sharing    {}",
        if a.redistributable { "opted in (redistributable)" } else { "off (not redistributable)" }
    );
    Ok(())
}

fn human(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

fn state_text(s: &ModelState) -> String {
    match s {
        ModelState::Ready => "ready".into(),
        ModelState::Degraded { reason, .. } => format!("degraded ({reason})"),
        ModelState::Refused { reason } => format!("REFUSED ({reason})"),
    }
}

fn list(reg: &ModelRegistry) -> anyhow::Result<()> {
    let tiers = TierResolver::system_default();
    let entries = reg.entries();
    if entries.is_empty() {
        println!("No models adopted");
        return Ok(());
    }
    println!("{:<28} {:<12} {:<10} {:<9} {:<9} STATE", "NAME", "FORMAT", "SIZE", "SHARDS", "STORE");
    for (id, e) in entries {
        let body = e.body()?;
        let check = reg.check(&id, CheckMode::Stat)?;
        let store = match tiers.classify(&e.root).0 {
            StoreTier::Internal => "internal",
            StoreTier::External => "external",
        };
        println!(
            "{:<28} {:<12} {:<10} {:<9} {:<9} {}",
            body.name,
            body.format.as_str(),
            human(body.total_bytes()),
            body.shards.len(),
            store,
            state_text(&check.state)
        );
    }
    Ok(())
}

fn verify(reg: &ModelRegistry, model: &str, full: bool) -> anyhow::Result<()> {
    let mode = if full { CheckMode::Full } else { CheckMode::Lazy };
    let check = reg.check(model, mode)?;
    for (path, outcome) in &check.files {
        let text = match outcome {
            FileOutcome::Ok { rehashed: true } => "ok (hashed)".to_string(),
            FileOutcome::Ok { rehashed: false } => "ok (unchanged)".to_string(),
            FileOutcome::Missing => "MISSING".to_string(),
            FileOutcome::Mismatch { actual } => format!("MISMATCH (found {actual})"),
        };
        println!("{path}: {text}");
    }
    println!("{}: {}", check.name, state_text(&check.state));
    if !check.state.is_ready() {
        std::process::exit(2);
    }
    Ok(())
}

/// Parse `<id>=complete|none[:<free GiB>][,ineligible]` into a holding.
fn parse_node(spec: &str, body: &clawft_kernel::model_manifest::ModelPackageBody) -> anyhow::Result<NodeHolding> {
    let (id, rest) = spec
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("--node expects <id>=<complete|none>[:<free GiB>][,ineligible], got {spec:?}"))?;
    let mut parts = rest.split(',');
    let held = parts.next().unwrap_or_default();
    let eligible = !parts.any(|p| p == "ineligible");
    let (what, free) = match held.split_once(':') {
        Some((w, f)) => (w, Some(f.parse::<u64>()? * (1u64 << 30))),
        None => (held, None),
    };
    let complete = match what {
        "complete" => true,
        "none" => false,
        other => anyhow::bail!("node state must be complete or none, got {other:?}"),
    };
    Ok(NodeHolding {
        node_id: id.to_string(),
        eligible,
        shards: if complete { body.shard_hashes().map(String::from).collect() } else { Default::default() },
        complete,
        free_bytes: free,
    })
}

fn explain(reg: &ModelRegistry, a: &ExplainArgs) -> anyhow::Result<()> {
    let (_, entry) = reg.get(&a.model)?;
    let body = entry.body()?;
    let nodes = a
        .nodes
        .iter()
        .map(|s| parse_node(s, &body))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let policy = TransferPolicy {
        allow_weight_transfer: a.allow_transfer,
        max_bytes: a.max_gib.map(|g| g << 30),
        ..TransferPolicy::default()
    };
    let plan = decide(&body, &nodes, &policy, Sharing::from_manifest(&body));
    for line in &plan.notes {
        println!("  {line}");
    }
    if let LocalityDecision::Unplaceable(_) = plan.decision {
        std::process::exit(2);
    }
    Ok(())
}

/// Run a `weaver model` subcommand.
pub fn run(args: ModelArgs) -> anyhow::Result<()> {
    let reg = ModelRegistry::open(registry_path(&args))?;
    match args.command {
        ModelCommand::Adopt(a) => adopt(&reg, a),
        ModelCommand::List => list(&reg),
        ModelCommand::Verify { model, full } => verify(&reg, &model, full),
        ModelCommand::Explain(a) => explain(&reg, &a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_kernel::model_manifest::{ModelPackageBody, ModelSource};
    use clawft_kernel::workload_pkg::FileRef;

    fn body() -> ModelPackageBody {
        ModelPackageBody {
            name: "M".into(),
            format: ModelFormat::Mlx,
            shards: vec![FileRef { path: "a.safetensors".into(), size: 10, blake3: "0".repeat(64) }],
            tokenizer_blake3: None,
            template_blake3: None,
            source: ModelSource { hf_repo: Some("o/m".into()), ..ModelSource::default() },
            redistributable: false,
        }
    }

    #[test]
    fn node_specs_parse() {
        let b = body();
        let n = parse_node("mac=complete", &b).unwrap();
        assert!(n.complete && n.eligible && n.shards.len() == 1);
        let n = parse_node("pi=none:200,ineligible", &b).unwrap();
        assert!(!n.complete && !n.eligible && n.free_bytes == Some(200 << 30));
        assert!(parse_node("pi", &b).is_err());
        assert!(parse_node("pi=some", &b).is_err());
    }
}
