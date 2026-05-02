//! `weaver soul` — promote identity-drift observations into `SOUL.md`.
//!
//! `agent-core-v1` Phase F2 — the operator-facing side of the
//! identity-journal flow. F1 stamped a `soul_journal` derived-write
//! grant at boot and seeded `.clawft/SOUL.md`, `.clawft/IDENTITY.md`,
//! and `.clawft/SOUL.journal.md`. F2 lets a human read pending
//! observations from the substrate-backed `soul_journal` topic, see
//! a diff against the current `.clawft/SOUL.md`, and on confirmation
//! write the merged result back to disk plus a witness chain entry.
//!
//! The agent's *write* side of the journal (self-observation during
//! chat turns) is deferred to a future phase. F2 lands the operator
//! side only — with an empty journal the command exits cleanly with
//! "No journal entries to promote." so the rollout doesn't require
//! the agent to start writing first.
//!
//! # Wire shape (current node)
//!
//! Substrate paths:
//! - prefix: `substrate/<daemon-node-id>/derived/soul_journal/`
//! - entry: `substrate/<daemon-node-id>/derived/soul_journal/<entry-ulid>`
//! - entry value: `{ "summary": "...", "content": "...", "ts": "..." }`
//!   (only `content` is required; the rest are best-effort metadata
//!   the agent's future write path will populate)
//!
//! # Witness chain
//!
//! The daemon does not expose a public `chain.append` RPC today (see
//! `crates/clawft-weave/src/daemon.rs::handle_request` — only
//! `chain.{status,local,checkpoint,verify,export}`). For F2 we record
//! the promotion event two ways:
//!
//! 1. `tracing::info!(target = "chain_event", source = "soul",
//!    kind = "soul.promote", ...)` — this is the bridge that
//!    [`clawft_core::chain_event::push_chain_event`] uses; the
//!    daemon's chain-event bridge drains the buffer every 2s and
//!    forwards to `ChainManager::append`. Because `weaver` is a
//!    short-lived CLI that talks to the daemon over a Unix socket
//!    (not embedded), the bridge never sees this event. So we also:
//! 2. Append the same payload as one JSONL line to
//!    `<workspace>/.weftos/audit/soul-promote.log`. Local audit log
//!    is the durable record until a public `chain.append` RPC ships;
//!    follow-up TODO is filed below.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::client::DaemonClient;
use crate::protocol::{
    NodeIdentityResult, Request, SubstrateListChild, SubstrateListParams, SubstrateListResult,
    SubstrateReadParams, SubstrateReadResult,
};

/// `weaver soul` subcommand group.
#[derive(Args)]
pub struct SoulArgs {
    #[command(subcommand)]
    pub cmd: SoulCmd,
}

/// `weaver soul {promote, status}` — operator-facing identity-journal
/// commands. See module docs for wire shape and witness semantics.
#[derive(Subcommand)]
pub enum SoulCmd {
    /// Read pending journal entries, show a diff, prompt for
    /// confirmation, write the merged result to `.clawft/SOUL.md`,
    /// and append a witness audit entry recording the promotion.
    Promote(PromoteArgs),
    /// Show pending journal entries without applying them.
    Status(StatusArgs),
}

/// Arguments to `weaver soul promote`.
#[derive(Args, Default, Clone)]
pub struct PromoteArgs {
    /// Skip the interactive confirmation prompt — apply if non-empty.
    #[arg(long, short)]
    pub yes: bool,

    /// Workspace path. Defaults to cwd. Mirrors `weaver init`'s flag.
    #[arg(long)]
    pub workspace: Option<PathBuf>,
}

/// Arguments to `weaver soul status`.
#[derive(Args, Default, Clone)]
pub struct StatusArgs {
    /// Workspace path. Defaults to cwd.
    #[arg(long)]
    pub workspace: Option<PathBuf>,
}

/// One pending journal entry, decoded from substrate.
///
/// Built from the value side of `substrate/<node>/derived/soul_journal/<ulid>`.
/// The agent's future write path will populate `summary` and `ts`;
/// today both are best-effort and the diff body is `content`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalEntry {
    /// ULID portion of the substrate path — the entry's stable id.
    pub ulid: String,
    /// One-line human-readable summary. Falls back to the first 80
    /// characters of `content` when the writer didn't supply one.
    pub summary: String,
    /// Full body of the observation. Appended verbatim under the
    /// "## Drift Observations" section in the candidate SOUL.md.
    pub content: String,
    /// Best-effort write timestamp. Empty string when missing.
    #[serde(default)]
    pub ts: String,
}

/// Witness audit-log record written for every applied promotion.
///
/// Mirrors what a future public `chain.append` RPC payload would
/// carry; persisted to `.weftos/audit/soul-promote.log` as one JSONL
/// line per promote and (when running embedded inside the daemon)
/// pushed onto the chain-event bridge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WitnessRecord {
    /// `"soul.promote"`.
    pub kind: String,
    /// ULID of every journal entry that contributed to the merge.
    pub entries: Vec<String>,
    /// SHA-256 hex of the SOUL.md contents *before* the merge.
    pub hash_before: String,
    /// SHA-256 hex of the SOUL.md contents *after* the merge.
    pub hash_after: String,
    /// ISO-8601 (UTC) wall-clock at promotion time.
    pub ts: String,
}

/// Abstraction over the daemon RPC surface this command uses.
///
/// Production wires through [`DaemonRpc`]; tests hand a mock impl
/// to [`run_promote_with`] / [`run_status_with`] so they exercise
/// the merge + diff + witness paths without spinning a daemon.
#[async_trait::async_trait]
pub trait SoulRpc: Send + Sync {
    /// Resolve the daemon's own node-id (used to scope the
    /// `derived/soul_journal/` prefix).
    async fn node_id(&mut self) -> anyhow::Result<String>;

    /// Enumerate journal entries under the given prefix. Returns
    /// only **value-bearing** children (we filter on `has_value` so
    /// pure structural nodes don't appear as ghost ulids).
    async fn list_journal(
        &mut self,
        prefix: &str,
    ) -> anyhow::Result<Vec<SubstrateListChild>>;

    /// Read one journal entry's payload by full substrate path.
    async fn read_entry(&mut self, path: &str) -> anyhow::Result<Option<serde_json::Value>>;

    /// Append a witness record. Today this is a local audit-log
    /// write (no public `chain.append` RPC exists); the trait shape
    /// is forward-compatible with switching to RPC when one ships.
    async fn append_chain(
        &mut self,
        workspace: &Path,
        record: &WitnessRecord,
    ) -> anyhow::Result<()>;
}

/// Production [`SoulRpc`] over [`DaemonClient`].
pub struct DaemonRpc {
    client: DaemonClient,
}

impl DaemonRpc {
    /// Connect to the running daemon over the Unix socket. Returns an
    /// error (rather than `Option`) so the calling command can bail
    /// with a clear "no daemon running" message.
    pub async fn connect() -> anyhow::Result<Self> {
        let client = DaemonClient::connect().await.ok_or_else(|| {
            anyhow::anyhow!("no daemon running — start with 'weaver kernel start'")
        })?;
        Ok(Self { client })
    }
}

#[async_trait::async_trait]
impl SoulRpc for DaemonRpc {
    async fn node_id(&mut self) -> anyhow::Result<String> {
        let resp = self.client.simple_call("node.identity").await?;
        if !resp.ok {
            anyhow::bail!("node.identity failed: {}", resp.error.unwrap_or_default());
        }
        let r: NodeIdentityResult = serde_json::from_value(resp.result.unwrap_or_default())?;
        Ok(r.node_id)
    }

    async fn list_journal(
        &mut self,
        prefix: &str,
    ) -> anyhow::Result<Vec<SubstrateListChild>> {
        let params = SubstrateListParams {
            prefix: prefix.to_string(),
            depth: 1,
            actor_id: None,
        };
        let resp = self
            .client
            .call(Request::with_params(
                "substrate.list",
                serde_json::to_value(params)?,
            ))
            .await?;
        if !resp.ok {
            anyhow::bail!("substrate.list failed: {}", resp.error.unwrap_or_default());
        }
        let r: SubstrateListResult = serde_json::from_value(resp.result.unwrap_or_default())?;
        Ok(r.children.into_iter().filter(|c| c.has_value).collect())
    }

    async fn read_entry(&mut self, path: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let params = SubstrateReadParams {
            path: path.to_string(),
            actor_id: None,
        };
        let resp = self
            .client
            .call(Request::with_params(
                "substrate.read",
                serde_json::to_value(params)?,
            ))
            .await?;
        if !resp.ok {
            anyhow::bail!("substrate.read failed: {}", resp.error.unwrap_or_default());
        }
        let r: SubstrateReadResult = serde_json::from_value(resp.result.unwrap_or_default())?;
        Ok(r.value)
    }

    async fn append_chain(
        &mut self,
        workspace: &Path,
        record: &WitnessRecord,
    ) -> anyhow::Result<()> {
        // No `chain.append` RPC is exposed by the daemon today; until
        // one ships, write the same payload to a local audit log.
        // TODO(agent-core-v1.1): replace with `chain.append` RPC once
        // the daemon's public chain surface gains an append handler;
        // the trait shape is already forward-compatible.
        write_audit_log(workspace, record)?;
        // Best-effort: also emit a tracing event so any in-process
        // chain-event bridge (i.e. when this code is later embedded)
        // forwards through `clawft_core::chain_event`. For the CLI
        // process this is a no-op past the subscriber.
        tracing::info!(
            target: "chain_event",
            source = "soul",
            kind = %record.kind,
            entries = %record.entries.join(","),
            hash_before = %record.hash_before,
            hash_after = %record.hash_after,
            ts = %record.ts,
            "chain"
        );
        Ok(())
    }
}

// ── Public entry points ──────────────────────────────────────────

/// Dispatch `weaver soul <subcommand>` from `main.rs`.
pub async fn run(args: SoulArgs) -> anyhow::Result<()> {
    match args.cmd {
        SoulCmd::Promote(p) => {
            let mut rpc = DaemonRpc::connect().await?;
            run_promote_with(p, &mut rpc, &mut StdinPrompt).await
        }
        SoulCmd::Status(s) => {
            let mut rpc = DaemonRpc::connect().await?;
            run_status_with(s, &mut rpc).await
        }
    }
}

/// Trait abstracting the y/N confirmation prompt — production reads
/// stdin, tests hand back a canned answer.
pub trait Prompter: Send {
    /// Print `question` and read one line; return `true` for an
    /// affirmative answer (`y` / `yes`, case-insensitive).
    fn confirm(&mut self, question: &str) -> std::io::Result<bool>;
}

/// Stdin-backed [`Prompter`] used in production.
pub struct StdinPrompt;

impl Prompter for StdinPrompt {
    fn confirm(&mut self, question: &str) -> std::io::Result<bool> {
        print!("{question}");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(matches!(
            line.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }
}

/// Drive `weaver soul promote` against the supplied RPC + prompter.
///
/// Factored out from [`run`] so unit tests can mock both seams.
pub async fn run_promote_with<R: SoulRpc, P: Prompter>(
    args: PromoteArgs,
    rpc: &mut R,
    prompter: &mut P,
) -> anyhow::Result<()> {
    let workspace = resolve_workspace(args.workspace.as_deref())?;
    let soul_path = workspace.join(".clawft").join("SOUL.md");

    let entries = fetch_journal(rpc).await?;
    if entries.is_empty() {
        println!("No journal entries to promote.");
        return Ok(());
    }

    let current_soul = std::fs::read_to_string(&soul_path).unwrap_or_default();
    let candidate = compose_candidate(&current_soul, &entries);

    print_diff(&current_soul, &candidate);

    let confirmed = if args.yes {
        true
    } else {
        prompter.confirm("Apply this promotion? [y/N] ")?
    };
    if !confirmed {
        println!("Aborted; SOUL.md unchanged.");
        return Ok(());
    }

    if let Some(parent) = soul_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&soul_path, &candidate)?;

    let record = WitnessRecord {
        kind: "soul.promote".to_string(),
        entries: entries.iter().map(|e| e.ulid.clone()).collect(),
        hash_before: sha256_hex(current_soul.as_bytes()),
        hash_after: sha256_hex(candidate.as_bytes()),
        ts: chrono::Utc::now().to_rfc3339(),
    };
    rpc.append_chain(&workspace, &record).await?;

    println!("Promoted {} entries.", entries.len());
    Ok(())
}

/// Drive `weaver soul status` against the supplied RPC. The dry-run
/// sibling of [`run_promote_with`] — no disk writes, no chain entry.
pub async fn run_status_with<R: SoulRpc>(
    args: StatusArgs,
    rpc: &mut R,
) -> anyhow::Result<()> {
    // Resolve workspace just to surface a nicer error when the user
    // calls this from outside an init'd project; the path itself
    // isn't used for status (status reads from substrate).
    let _workspace = resolve_workspace(args.workspace.as_deref())?;
    let entries = fetch_journal(rpc).await?;
    if entries.is_empty() {
        println!("No journal entries pending.");
        return Ok(());
    }
    for entry in &entries {
        println!("{}  {}", entry.ulid, entry.summary);
    }
    Ok(())
}

// ── Internals ────────────────────────────────────────────────────

/// Resolve the workspace path, defaulting to cwd.
fn resolve_workspace(override_path: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(p) = override_path {
        return Ok(p.to_path_buf());
    }
    Ok(std::env::current_dir()?)
}

/// SHA-256 hex digest helper. Matches the shape D1 hashes identity
/// content with so witness payloads stay comparable across crates.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Build the prefix, list, then read each child as a [`JournalEntry`].
async fn fetch_journal<R: SoulRpc>(rpc: &mut R) -> anyhow::Result<Vec<JournalEntry>> {
    let node_id = rpc.node_id().await?;
    let prefix = format!("substrate/{node_id}/derived/soul_journal");
    let children = rpc.list_journal(&prefix).await?;

    let mut entries = Vec::with_capacity(children.len());
    for child in children {
        let ulid = child
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&child.path)
            .to_string();
        let value = rpc.read_entry(&child.path).await?;
        let entry = decode_entry(ulid, value);
        entries.push(entry);
    }
    Ok(entries)
}

/// Decode a substrate value into a [`JournalEntry`], with sensible
/// fallbacks so ill-formed entries don't break the pipeline.
fn decode_entry(ulid: String, value: Option<serde_json::Value>) -> JournalEntry {
    let value = value.unwrap_or(serde_json::Value::Null);
    let content = value
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            // Fall back to whole value as a string if `content`
            // wasn't supplied — keeps malformed entries visible
            // rather than silently dropping them.
            value.as_str().unwrap_or("")
        })
        .to_string();
    let summary = value
        .get("summary")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| summarize(&content));
    let ts = value
        .get("ts")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_default();
    JournalEntry {
        ulid,
        summary,
        content,
        ts,
    }
}

/// First-line, max-80-char default summary.
fn summarize(content: &str) -> String {
    let first_line = content.lines().next().unwrap_or("").trim();
    if first_line.len() > 80 {
        format!("{}…", &first_line[..80])
    } else {
        first_line.to_string()
    }
}

/// Section marker the candidate SOUL.md uses to anchor appended
/// observations. Detecting an existing marker keeps re-promotes
/// idempotent in shape (one section header, growing body).
const DRIFT_HEADING: &str = "## Drift Observations";

/// Compose the candidate SOUL.md by appending each journal entry
/// under a `## Drift Observations` section. Preserves the existing
/// content verbatim; only appends.
fn compose_candidate(current: &str, entries: &[JournalEntry]) -> String {
    let mut out = String::with_capacity(current.len() + 256);
    out.push_str(current);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !current.contains(DRIFT_HEADING) {
        out.push('\n');
        out.push_str(DRIFT_HEADING);
        out.push('\n');
    }
    for entry in entries {
        out.push('\n');
        out.push_str(&format!("### {}\n", entry.ulid));
        if !entry.ts.is_empty() {
            out.push_str(&format!("_observed: {}_\n\n", entry.ts));
        }
        out.push_str(entry.content.trim_end());
        out.push('\n');
    }
    out
}

/// Print a labeled before/after block. Workspace doesn't pull in
/// `similar` so this is the simplest sensible display — the operator
/// can always run their own diff tool against the seed/candidate
/// pair if they want a unified view.
fn print_diff(current: &str, candidate: &str) {
    println!("--- current SOUL.md ---");
    if current.is_empty() {
        println!("(empty)");
    } else {
        println!("{}", current.trim_end());
    }
    println!("--- candidate SOUL.md ---");
    println!("{}", candidate.trim_end());
    println!("--- end ---");
}

/// Write one JSONL line to `<workspace>/.weftos/audit/soul-promote.log`.
fn write_audit_log(workspace: &Path, record: &WitnessRecord) -> anyhow::Result<()> {
    let dir = workspace.join(".weftos").join("audit");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("soul-promote.log");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let line = serde_json::to_string(record)?;
    writeln!(file, "{line}")?;
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// In-memory mock — drives `run_promote_with` / `run_status_with`
    /// without touching the daemon socket.
    #[derive(Default)]
    struct MockRpc {
        node_id: String,
        children: Vec<SubstrateListChild>,
        values: HashMap<String, serde_json::Value>,
        appended: Vec<WitnessRecord>,
        appended_workspaces: Vec<PathBuf>,
    }

    #[async_trait::async_trait]
    impl SoulRpc for MockRpc {
        async fn node_id(&mut self) -> anyhow::Result<String> {
            Ok(self.node_id.clone())
        }

        async fn list_journal(
            &mut self,
            _prefix: &str,
        ) -> anyhow::Result<Vec<SubstrateListChild>> {
            Ok(self.children.clone())
        }

        async fn read_entry(
            &mut self,
            path: &str,
        ) -> anyhow::Result<Option<serde_json::Value>> {
            Ok(self.values.get(path).cloned())
        }

        async fn append_chain(
            &mut self,
            workspace: &Path,
            record: &WitnessRecord,
        ) -> anyhow::Result<()> {
            self.appended.push(record.clone());
            self.appended_workspaces.push(workspace.to_path_buf());
            Ok(())
        }
    }

    /// Canned [`Prompter`] returning a fixed answer. Reused across
    /// the confirm/abort tests.
    struct FakePrompt(bool);

    impl Prompter for FakePrompt {
        fn confirm(&mut self, _question: &str) -> std::io::Result<bool> {
            Ok(self.0)
        }
    }

    fn write_seed_soul(workspace: &Path, body: &str) {
        let dir = workspace.join(".clawft");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SOUL.md"), body).unwrap();
    }

    fn child(path: &str) -> SubstrateListChild {
        SubstrateListChild {
            path: path.to_string(),
            has_value: true,
            child_count: 0,
        }
    }

    #[tokio::test]
    async fn promote_with_empty_journal_returns_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        write_seed_soul(tmp.path(), "# SOUL\nseed\n");
        let mut rpc = MockRpc {
            node_id: "n-abc123".into(),
            ..Default::default()
        };
        let mut prompt = FakePrompt(true);
        let args = PromoteArgs {
            yes: true,
            workspace: Some(tmp.path().to_path_buf()),
        };
        run_promote_with(args, &mut rpc, &mut prompt)
            .await
            .unwrap();

        // SOUL.md untouched.
        let soul = std::fs::read_to_string(tmp.path().join(".clawft").join("SOUL.md")).unwrap();
        assert_eq!(soul, "# SOUL\nseed\n", "empty journal must not edit SOUL.md");
        assert!(rpc.appended.is_empty(), "no chain entry on empty journal");
    }

    #[tokio::test]
    async fn promote_appends_drift_observations_section() {
        let tmp = tempfile::tempdir().unwrap();
        write_seed_soul(tmp.path(), "# SOUL\nseed body\n");

        let path1 = "substrate/n-abc/derived/soul_journal/01HZX1AAAA";
        let path2 = "substrate/n-abc/derived/soul_journal/01HZX1BBBB";
        let mut values = HashMap::new();
        values.insert(
            path1.to_string(),
            serde_json::json!({
                "summary": "noticed bias toward verbose answers",
                "content": "noticed bias toward verbose answers",
                "ts": "2026-04-27T12:00:00Z"
            }),
        );
        values.insert(
            path2.to_string(),
            serde_json::json!({
                "summary": "added a kindness register",
                "content": "added a kindness register"
            }),
        );

        let mut rpc = MockRpc {
            node_id: "n-abc".into(),
            children: vec![child(path1), child(path2)],
            values,
            ..Default::default()
        };
        let mut prompt = FakePrompt(true);
        let args = PromoteArgs {
            yes: true,
            workspace: Some(tmp.path().to_path_buf()),
        };
        run_promote_with(args, &mut rpc, &mut prompt)
            .await
            .unwrap();

        let soul = std::fs::read_to_string(tmp.path().join(".clawft").join("SOUL.md")).unwrap();
        assert!(
            soul.contains("# SOUL\nseed body"),
            "candidate must preserve the prior SOUL.md prefix"
        );
        assert!(
            soul.contains("## Drift Observations"),
            "candidate must contain the drift section heading"
        );
        assert!(
            soul.contains("01HZX1AAAA"),
            "candidate must contain the first entry's ULID anchor"
        );
        assert!(
            soul.contains("01HZX1BBBB"),
            "candidate must contain the second entry's ULID anchor"
        );
        assert!(
            soul.contains("noticed bias toward verbose answers"),
            "candidate must contain the first entry's content body"
        );
        assert!(
            soul.contains("added a kindness register"),
            "candidate must contain the second entry's content body"
        );
    }

    #[tokio::test]
    async fn promote_writes_witness_chain_append() {
        let tmp = tempfile::tempdir().unwrap();
        let seed = "# SOUL\nseed body\n";
        write_seed_soul(tmp.path(), seed);

        let path1 = "substrate/n-x/derived/soul_journal/01HZX1AAAA";
        let mut values = HashMap::new();
        values.insert(
            path1.to_string(),
            serde_json::json!({
                "summary": "drift",
                "content": "drift body"
            }),
        );

        let mut rpc = MockRpc {
            node_id: "n-x".into(),
            children: vec![child(path1)],
            values,
            ..Default::default()
        };
        let mut prompt = FakePrompt(true);
        let args = PromoteArgs {
            yes: true,
            workspace: Some(tmp.path().to_path_buf()),
        };
        run_promote_with(args, &mut rpc, &mut prompt)
            .await
            .unwrap();

        assert_eq!(rpc.appended.len(), 1, "exactly one witness entry");
        let rec = &rpc.appended[0];
        assert_eq!(rec.kind, "soul.promote");
        assert_eq!(rec.entries, vec!["01HZX1AAAA".to_string()]);
        // Hashes are SHA-256 hex (64 chars).
        assert_eq!(rec.hash_before.len(), 64);
        assert_eq!(rec.hash_after.len(), 64);
        assert_ne!(
            rec.hash_before, rec.hash_after,
            "hash_before and hash_after must differ when SOUL.md changed"
        );
        // hash_before must match the seed content we wrote.
        assert_eq!(rec.hash_before, sha256_hex(seed.as_bytes()));
        // hash_after must match the file we just wrote.
        let after = std::fs::read_to_string(tmp.path().join(".clawft").join("SOUL.md")).unwrap();
        assert_eq!(rec.hash_after, sha256_hex(after.as_bytes()));
    }

    #[tokio::test]
    async fn status_lists_pending_entries() {
        // Capture stdout via a buffered writer would be ideal, but
        // for unit-level coverage we exercise the same fetch path
        // and verify the entries the printer would receive are the
        // ones the RPC returned. (Status is fundamentally a print
        // wrapper around `fetch_journal`; testing the fetch covers
        // the moving parts.)
        let path1 = "substrate/n/derived/soul_journal/01HZA";
        let path2 = "substrate/n/derived/soul_journal/01HZB";
        let mut values = HashMap::new();
        values.insert(
            path1.to_string(),
            serde_json::json!({"summary": "a", "content": "a body"}),
        );
        values.insert(
            path2.to_string(),
            serde_json::json!({"summary": "b", "content": "b body"}),
        );
        let mut rpc = MockRpc {
            node_id: "n".into(),
            children: vec![child(path1), child(path2)],
            values,
            ..Default::default()
        };

        let entries = fetch_journal(&mut rpc).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].ulid, "01HZA");
        assert_eq!(entries[0].summary, "a");
        assert_eq!(entries[1].ulid, "01HZB");
        assert_eq!(entries[1].summary, "b");

        // Drive the public path too — must not error and must not
        // append a chain entry (status is read-only).
        let tmp = tempfile::tempdir().unwrap();
        let args = StatusArgs {
            workspace: Some(tmp.path().to_path_buf()),
        };
        run_status_with(args, &mut rpc).await.unwrap();
        assert!(rpc.appended.is_empty(), "status must never append chain");
    }

    #[tokio::test]
    async fn promote_without_confirm_aborts() {
        let tmp = tempfile::tempdir().unwrap();
        let seed = "# SOUL\nseed\n";
        write_seed_soul(tmp.path(), seed);

        let path1 = "substrate/n/derived/soul_journal/01HZX";
        let mut values = HashMap::new();
        values.insert(
            path1.to_string(),
            serde_json::json!({"summary": "drift", "content": "drift body"}),
        );
        let mut rpc = MockRpc {
            node_id: "n".into(),
            children: vec![child(path1)],
            values,
            ..Default::default()
        };
        let mut prompt = FakePrompt(false);
        let args = PromoteArgs {
            yes: false,
            workspace: Some(tmp.path().to_path_buf()),
        };
        run_promote_with(args, &mut rpc, &mut prompt)
            .await
            .unwrap();

        // SOUL.md unchanged.
        let soul = std::fs::read_to_string(tmp.path().join(".clawft").join("SOUL.md")).unwrap();
        assert_eq!(soul, seed, "abort must not touch SOUL.md");
        // No chain entry.
        assert!(
            rpc.appended.is_empty(),
            "abort must not write a witness record"
        );
    }

    #[test]
    fn audit_log_writes_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let record = WitnessRecord {
            kind: "soul.promote".into(),
            entries: vec!["01HZA".into()],
            hash_before: "0".repeat(64),
            hash_after: "1".repeat(64),
            ts: "2026-04-27T12:00:00Z".into(),
        };
        write_audit_log(tmp.path(), &record).unwrap();
        let path = tmp.path().join(".weftos").join("audit").join("soul-promote.log");
        let body = std::fs::read_to_string(&path).unwrap();
        let line = body.trim();
        let parsed: WitnessRecord = serde_json::from_str(line).unwrap();
        assert_eq!(parsed, record);
    }

    #[test]
    fn compose_candidate_appends_existing_drift_section() {
        // Re-promote behavior: existing `## Drift Observations`
        // header isn't duplicated — entries append under the
        // existing one, keeping the document idempotent in shape.
        let current = "# SOUL\nbody\n\n## Drift Observations\n\n### old\nold body\n";
        let entries = vec![JournalEntry {
            ulid: "new".into(),
            summary: "n".into(),
            content: "new body".into(),
            ts: String::new(),
        }];
        let out = compose_candidate(current, &entries);
        // Only one occurrence of the heading.
        assert_eq!(out.matches(DRIFT_HEADING).count(), 1);
        assert!(out.contains("### old"), "old entry preserved");
        assert!(out.contains("### new"), "new entry appended");
    }
}
