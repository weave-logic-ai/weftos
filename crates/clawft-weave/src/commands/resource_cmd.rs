//! `weaver resource` — resource tree management commands.
//!
//! Includes K1 ACL operator surface (WEFT-554):
//! `weaver resource {grant,revoke,check}` — local CapabilityChecker path
//! (in-process; daemon-persisted ACL RPC is residual).

use clap::{Args, Subcommand};
use comfy_table::{Cell, Table};

use crate::protocol::{
    Request, ResourceInspectParams, ResourceNodeInfo, ResourceRankEntry, ResourceRankParams,
    ResourceScoreParams, ResourceScoreResult, ResourceStatsResult,
};

#[derive(Args)]
pub struct ResourceArgs {
    #[command(subcommand)]
    pub command: ResourceCommand,
}

#[derive(Subcommand)]
pub enum ResourceCommand {
    /// Display the full resource tree.
    Tree,
    /// Inspect a specific resource node.
    Inspect {
        /// Resource path (e.g. "/kernel/services/cron").
        path: String,
    },
    /// Show resource tree statistics.
    Stats,
    /// Show scoring for a resource node.
    Score {
        /// Resource path (e.g. "/kernel/agents/worker-1").
        path: String,
    },
    /// Rank resource nodes by composite score.
    Rank {
        /// Number of top-ranked nodes to show.
        #[arg(short = 'n', long, default_value = "10")]
        count: usize,
    },
    /// Grant an ACL action to a principal DID on a resource path (WEFT-554).
    ///
    /// Runs against a local in-process CapabilityChecker (default-deny).
    /// Daemon-persisted ACL is residual — this surfaces the K1 engine for
    /// operators and scripts.
    Grant {
        /// Principal DID (e.g. `did:weft:alice` or opaque id).
        #[arg(long, short = 'p')]
        principal: String,
        /// Resource path (e.g. `/apps/secret`).
        #[arg(long, short = 'r')]
        path: String,
        /// Action: Read | Write | Execute | Admin | Create | Delete.
        #[arg(long, short = 'a', default_value = "Read")]
        action: String,
    },
    /// Revoke a previously granted ACL action (local checker session / re-deny).
    Revoke {
        /// Principal DID.
        #[arg(long, short = 'p')]
        principal: String,
        /// Resource path.
        #[arg(long, short = 'r')]
        path: String,
        /// Action to deny.
        #[arg(long, short = 'a', default_value = "Read")]
        action: String,
    },
    /// Check whether a principal may perform an action on a path (WEFT-554).
    Check {
        /// Principal DID.
        #[arg(long, short = 'p')]
        principal: String,
        /// Resource path.
        #[arg(long, short = 'r')]
        path: String,
        /// Action to check.
        #[arg(long, short = 'a', default_value = "Read")]
        action: String,
        /// Optional role label (Owner|Admin|Operator|Viewer). Default Viewer.
        #[arg(long, default_value = "Viewer")]
        role: String,
    },
}

pub async fn run(args: ResourceArgs) -> anyhow::Result<()> {
    // Local K1 ACL surface does not require a running daemon.
    match &args.command {
        ResourceCommand::Grant { .. }
        | ResourceCommand::Revoke { .. }
        | ResourceCommand::Check { .. } => {
            return run_local_acl(args.command);
        }
        _ => {}
    }

    let mut client = clawft_rpc::connect_or_bail().await?;

    match args.command {
        ResourceCommand::Tree => {
            let resp = client.simple_call("resource.tree").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let nodes: Vec<ResourceNodeInfo> =
                serde_json::from_value(resp.result.unwrap_or_default())?;

            let mut table = Table::new();
            table.set_header(vec!["Path", "Kind", "Children", "Hash"]);
            for n in &nodes {
                let hash_short = if n.merkle_hash.len() >= 12 {
                    format!("{}...", &n.merkle_hash[..12])
                } else {
                    n.merkle_hash.clone()
                };
                table.add_row(vec![
                    Cell::new(&n.id),
                    Cell::new(&n.kind),
                    Cell::new(n.children.len()),
                    Cell::new(hash_short),
                ]);
            }
            println!("{table}");
        }
        ResourceCommand::Inspect { path } => {
            let params = serde_json::to_value(ResourceInspectParams { path })?;
            let resp = client
                .call(Request::with_params("resource.inspect", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let node: ResourceNodeInfo = serde_json::from_value(resp.result.unwrap_or_default())?;

            println!("Resource: {}", node.id);
            println!("  Kind:       {}", node.kind);
            println!(
                "  Parent:     {}",
                node.parent.as_deref().unwrap_or("(root)")
            );
            println!("  Children:   {}", node.children.join(", "));
            println!("  Hash:       {}", node.merkle_hash);
            if node.metadata != serde_json::json!({}) {
                println!(
                    "  Metadata:   {}",
                    serde_json::to_string_pretty(&node.metadata)?
                );
            }
            if let Some(ref score) = node.scoring {
                println!("  Scoring:");
                println!("    Trust:       {:.3}", score.trust);
                println!("    Performance: {:.3}", score.performance);
                println!("    Difficulty:  {:.3}", score.difficulty);
                println!("    Reward:      {:.3}", score.reward);
                println!("    Reliability: {:.3}", score.reliability);
                println!("    Velocity:    {:.3}", score.velocity);
                println!("    Composite:   {:.3}", score.composite);
            }
        }
        ResourceCommand::Stats => {
            let resp = client.simple_call("resource.stats").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let stats: ResourceStatsResult =
                serde_json::from_value(resp.result.unwrap_or_default())?;

            println!("Resource Tree Statistics");
            println!("  Total nodes:   {}", stats.total_nodes);
            println!("  Namespaces:    {}", stats.namespaces);
            println!("  Services:      {}", stats.services);
            println!("  Agents:        {}", stats.agents);
            println!("  Devices:       {}", stats.devices);
            println!("  Root hash:     {}...", &stats.root_hash[..16]);
        }
        ResourceCommand::Score { path } => {
            let params = serde_json::to_value(ResourceScoreParams { path })?;
            let resp = client
                .call(Request::with_params("resource.score", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let score: ResourceScoreResult =
                serde_json::from_value(resp.result.unwrap_or_default())?;

            println!("Scoring: {}", score.path);
            println!("  Trust:       {:.3}", score.trust);
            println!("  Performance: {:.3}", score.performance);
            println!("  Difficulty:  {:.3}", score.difficulty);
            println!("  Reward:      {:.3}", score.reward);
            println!("  Reliability: {:.3}", score.reliability);
            println!("  Velocity:    {:.3}", score.velocity);
            println!("  Composite:   {:.3}", score.composite);
        }
        ResourceCommand::Rank { count } => {
            let params = serde_json::to_value(ResourceRankParams { count })?;
            let resp = client
                .call(Request::with_params("resource.rank", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let entries: Vec<ResourceRankEntry> =
                serde_json::from_value(resp.result.unwrap_or_default())?;

            if entries.is_empty() {
                println!("No scored resources.");
                return Ok(());
            }

            let mut table = Table::new();
            table.set_header(vec!["Rank", "Path", "Score"]);
            for (i, e) in entries.iter().enumerate() {
                table.add_row(vec![
                    Cell::new(i + 1),
                    Cell::new(&e.path),
                    Cell::new(format!("{:.4}", e.score)),
                ]);
            }
            println!("{table}");
        }
        // grant/revoke/check are handled before daemon connect.
        ResourceCommand::Grant { .. }
        | ResourceCommand::Revoke { .. }
        | ResourceCommand::Check { .. } => unreachable!("handled above"),
    }

    Ok(())
}

// ── WEFT-554 local ACL CLI (CapabilityChecker foundation) ──────────

#[cfg(feature = "exochain")]
fn parse_action(s: &str) -> anyhow::Result<exo_resource_tree::Action> {
    use exo_resource_tree::Action;
    Ok(match s.to_ascii_lowercase().as_str() {
        "read" => Action::Read,
        "write" => Action::Write,
        "execute" | "exec" => Action::Execute,
        "admin" => Action::Admin,
        "create" => Action::Create,
        "delete" => Action::Delete,
        other => Action::Custom(other.to_string()),
    })
}

#[cfg(feature = "exochain")]
fn parse_role(s: &str) -> exo_resource_tree::Role {
    use exo_resource_tree::Role;
    match s.to_ascii_lowercase().as_str() {
        "owner" => Role::Owner,
        "admin" => Role::Admin,
        "operator" => Role::Operator,
        "viewer" => Role::Viewer,
        other => Role::Custom(other.to_string()),
    }
}

/// Path for the local ACL grant list (ephemeral operator store).
#[cfg(feature = "exochain")]
fn local_acl_store_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".clawft")
        .join("acl-local-grants.json")
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg(feature = "exochain")]
struct LocalAclStore {
    /// Grants: [principal, path, action]
    grants: Vec<(String, String, String)>,
    /// Explicit denies: [principal, path, action]
    denies: Vec<(String, String, String)>,
}

#[cfg(feature = "exochain")]
fn load_local_store() -> LocalAclStore {
    let path = local_acl_store_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => LocalAclStore::default(),
    }
}

#[cfg(feature = "exochain")]
fn save_local_store(store: &LocalAclStore) -> anyhow::Result<()> {
    let path = local_acl_store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(store)?;
    std::fs::write(&path, json)?;
    Ok(())
}

#[cfg(feature = "exochain")]
fn build_checker_from_store(store: &LocalAclStore) -> anyhow::Result<exo_resource_tree::CapabilityChecker> {
    use exo_resource_tree::{CapabilityChecker, Principal, ResourceId};
    let checker = CapabilityChecker::new();
    for (principal, path, action) in &store.grants {
        checker.grant(
            ResourceId::new(path.as_str()),
            Principal::new(principal.as_str()),
            parse_action(action)?,
        );
    }
    for (principal, path, action) in &store.denies {
        checker.deny(
            ResourceId::new(path.as_str()),
            Principal::new(principal.as_str()),
            parse_action(action)?,
        );
    }
    Ok(checker)
}

#[cfg(feature = "exochain")]
fn run_local_acl(cmd: ResourceCommand) -> anyhow::Result<()> {
    use exo_resource_tree::{Principal, ResourceId};

    match cmd {
        ResourceCommand::Grant {
            principal,
            path,
            action,
        } => {
            let mut store = load_local_store();
            // Drop matching deny so grant wins.
            store
                .denies
                .retain(|(p, r, a)| !(p == &principal && r == &path && a.eq_ignore_ascii_case(&action)));
            store
                .grants
                .retain(|(p, r, a)| !(p == &principal && r == &path && a.eq_ignore_ascii_case(&action)));
            store
                .grants
                .push((principal.clone(), path.clone(), action.clone()));
            save_local_store(&store)?;
            println!(
                "Granted {action} on {path} to {principal} (local store: {})",
                local_acl_store_path().display()
            );
            Ok(())
        }
        ResourceCommand::Revoke {
            principal,
            path,
            action,
        } => {
            let mut store = load_local_store();
            store
                .grants
                .retain(|(p, r, a)| !(p == &principal && r == &path && a.eq_ignore_ascii_case(&action)));
            store
                .denies
                .retain(|(p, r, a)| !(p == &principal && r == &path && a.eq_ignore_ascii_case(&action)));
            store
                .denies
                .push((principal.clone(), path.clone(), action.clone()));
            save_local_store(&store)?;
            println!(
                "Revoked {action} on {path} for {principal} (local store: {})",
                local_acl_store_path().display()
            );
            Ok(())
        }
        ResourceCommand::Check {
            principal,
            path,
            action,
            role,
        } => {
            let store = load_local_store();
            let checker = build_checker_from_store(&store)?;
            let p = Principal::new(principal.as_str());
            let r = ResourceId::new(path.as_str());
            let a = parse_action(&action)?;
            let role = parse_role(&role);
            let decision = checker.check_permission(&p, &role, &a, &r);
            println!(
                "check principal={principal} path={path} action={action} role={role:?} => {decision:?}"
            );
            if !decision.is_allowed() {
                std::process::exit(2);
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

#[cfg(not(feature = "exochain"))]
fn run_local_acl(_cmd: ResourceCommand) -> anyhow::Result<()> {
    anyhow::bail!("resource grant/revoke/check require the exochain feature (default build)")
}
