//! `weaver cog checkout ...` (ADR-106 phase 3): ask the steward for a
//! Cognitum cog, approve its hashes as the operator, and list what this node
//! holds.
//!
//! - `weaver cog checkout <cog>@<version> [--arch <arch>]`: the daemon asks
//!   the bound steward over `mesh.cog.checkout`, or its own relay when it is
//!   the steward. The grant lands in the node's store and floods to the mesh.
//! - `weaver cog checkout approve <cog>@<version> --operator-key <file>`: the
//!   daemon names the content (mesh id, cog, version, the sha256 set of the
//!   held grant, or `--sha256` values); this command prints it and, with
//!   `--confirm`, signs it under `weft-licence-v1/approval` with the operator
//!   key (which never leaves the CLI) and sends it back to be verified,
//!   stored and flooded. `--reapprove-orphaned` re-signs every approval a
//!   `mesh_nonce` change orphaned.
//! - `weaver cog checkout status [--explain]`: binding, steward, relay,
//!   grants and approvals; `--explain` adds the run gate's verdict per
//!   artifact.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use clawft_kernel::licence::{Approval, sign_approval};
use clawft_kernel::workload_pkg::signing_key_from_hex;
use clawft_rpc::DaemonClient;
use serde_json::{Value, json};

use crate::protocol::Request;

/// `weaver cog checkout` arguments.
#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct CheckoutArgs {
    /// `approve` or `status`; without one, check out `<cog>@<version>`.
    #[command(subcommand)]
    pub command: Option<CheckoutCmd>,
    /// `<cog>@<version>` (`latest` is allowed) to check out.
    pub reference: Option<String>,
    /// Target arch (default: this node's).
    #[arg(long)]
    pub arch: Option<String>,
    /// Print raw JSON.
    #[arg(long)]
    pub json: bool,
}

/// `weaver cog checkout` subcommands.
#[derive(Subcommand, Debug)]
pub enum CheckoutCmd {
    /// Approve the hashes of a checked-out cog version (operator, Admin).
    Approve(ApproveArgs),
    /// Grants and approvals this node holds.
    Status {
        /// Show the run gate's verdict for every artifact.
        #[arg(long)]
        explain: bool,
        /// Print raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// End a checkout: the steward asks the Seed for a withdrawal, which is
    /// flooded (operator, Admin; run on the steward). The same Seed call also
    /// renews every other active checkout of the mesh.
    Release {
        /// `<cog>@<version>` (an exact version).
        reference: String,
        /// Print raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Renew now instead of at the next 12-hourly pass (Admin; run on the
    /// steward). The Seed renews every active checkout in one call.
    Renew {
        /// `<cog>@<version>` (an exact version) to report on.
        reference: String,
        /// Print raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Held grants, approvals and expiry (read-only).
    List {
        /// Print raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Forget the grant clock high-water mark (alias of `weaver workload
    /// node reset-floor`); changes nothing without `--confirm <floor>`.
    ResetFloor {
        /// Apply the reset (chained), pinned to the floor the dry run printed.
        #[arg(long, value_name = "FLOOR", num_args = 0..=1)]
        confirm: Option<Option<String>>,
    },
}

/// `weaver cog checkout approve`.
#[derive(Args, Debug)]
pub struct ApproveArgs {
    /// `<cog>@<version>` (an exact version).
    #[arg(required_unless_present = "reapprove_orphaned")]
    pub reference: Option<String>,
    /// Operator key file (hex seed), pinned on the daemon.
    #[arg(long)]
    pub operator_key: PathBuf,
    /// Approve exactly this sha256 (repeat); default: every artifact of the held grant.
    #[arg(long = "sha256")]
    pub sha256: Vec<String>,
    /// Re-sign, for the current mesh id, every approval a `mesh_nonce` change orphaned.
    #[arg(long, conflicts_with_all = ["reference", "sha256"])]
    pub reapprove_orphaned: bool,
    /// Sign and send, pinned to what the dry run printed: the approval's
    /// content key, or with `--reapprove-orphaned` the batch digest. Without
    /// it the content is shown and nothing is signed; a value that no longer
    /// matches the freshly prepared content is refused.
    #[arg(long, value_name = "CONTENT_KEY|DIGEST")]
    pub confirm: Option<String>,
}

/// `<cog>@<version>` split, both parts non-empty.
pub fn parse_ref(r: &str) -> Result<(String, String), String> {
    match r.split_once('@') {
        Some((c, v)) if !c.is_empty() && !v.is_empty() && !v.contains('@') => Ok((c.into(), v.into())),
        _ => Err(format!("{r:?}: expected <cog>@<version>")),
    }
}

/// The approval the operator signs, from the daemon's `prepare` reply.
pub fn approval_from(prep: &Value, now: u64) -> Result<Approval, String> {
    let s = |k: &str| prep.get(k).and_then(Value::as_str).map(str::to_owned).ok_or(format!("the daemon's reply has no {k}"));
    let sha256: Vec<String> = prep["sha256"]
        .as_array()
        .ok_or("the daemon's reply has no sha256 set")?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    if sha256.is_empty() {
        return Err("nothing to approve: the sha256 set is empty".into());
    }
    Ok(Approval { v: 1, mesh_id: s("mesh_id")?, cog_id: s("cog_id")?, version: s("version")?, sha256, approved_at: now })
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub(crate) async fn call(client: &mut DaemonClient, method: &str, params: Value) -> anyhow::Result<Value> {
    let resp = client.call(Request::with_params(method, params)).await?;
    if !resp.ok {
        anyhow::bail!("{}", resp.error.unwrap_or_default());
    }
    Ok(resp.result.unwrap_or_default())
}

fn load_key(path: &std::path::Path) -> anyhow::Result<ed25519_dalek::SigningKey> {
    let text = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    signing_key_from_hex(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

/// Run `weaver cog checkout ...` against the daemon.
pub async fn run(a: CheckoutArgs) -> anyhow::Result<()> {
    let mut client = clawft_rpc::connect_or_bail().await?;
    match a.command {
        Some(CheckoutCmd::Status { explain, json }) => {
            let st = call(&mut client, "workload.cog.checkout.status", json!({})).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&st)?);
            } else {
                print!("{}", render_status(&st, explain));
            }
        }
        Some(CheckoutCmd::Approve(p)) => approve(&mut client, p).await?,
        Some(cmd) => super::cog_checkout_verbs::run(cmd, &mut client).await?,
        None => {
            let r = a.reference.ok_or_else(|| anyhow::anyhow!("give <cog>@<version>, or approve | status"))?;
            let (cog, version) = parse_ref(&r).map_err(anyhow::Error::msg)?;
            let v = call(&mut client, "workload.cog.checkout", json!({ "cog_id": cog, "version": version, "arch": a.arch })).await?;
            if a.json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                let g = &v["granted"];
                println!(
                    "granted {}@{} (seq {}, grant {}) via {}; expires_at {}",
                    g["cog_id"].as_str().unwrap_or("?"),
                    g["version"].as_str().unwrap_or("?"),
                    g["seq"],
                    g["grant_id"].as_str().map_or("?", |s| s.get(..12).unwrap_or(s)),
                    v["via"].as_str().unwrap_or("?"),
                    g["expires_at"],
                );
                println!("before it runs: weaver cog checkout approve {cog}@{} --operator-key <file>", g["version"].as_str().unwrap_or(&version));
            }
        }
    }
    Ok(())
}

/// The orphaned approvals this operator signed, re-addressed to `mesh_id`,
/// with how many were skipped (signed by another key, malformed, or not
/// actually for another mesh) and the batch digest `--confirm` must match.
pub fn reapprovals(
    operator: &ed25519_dalek::SigningKey,
    signed: &[clawft_kernel::licence::SignedApproval],
    mesh_id: &str,
    now: u64,
) -> (Vec<Approval>, usize, String) {
    let mut anchors = clawft_kernel::workload_pkg::TrustAnchors::default();
    let pk = clawft_kernel::workload_pkg::codec::hex_encode(&operator.verifying_key().to_bytes());
    let _ = anchors.push_signer("operator", &pk, clawft_kernel::workload_pkg::KeyOrigin::Operator);
    let mut out = Vec::new();
    let mut skipped = 0;
    for s in signed {
        match clawft_kernel::licence::verify_approval_signature(s, &anchors) {
            Ok(a) if a.mesh_id != mesh_id => out.push(Approval { mesh_id: mesh_id.to_owned(), approved_at: now, ..a }),
            _ => skipped += 1,
        }
    }
    let mut keys: Vec<String> = out.iter().map(Approval::content_key).collect();
    keys.sort();
    let digest = clawft_kernel::licence::sha256_hex(format!("{mesh_id}\n{}", keys.join("\n")).as_bytes());
    (out, skipped, digest[..16].to_owned())
}

async fn approve(client: &mut DaemonClient, p: ApproveArgs) -> anyhow::Result<()> {
    let key = load_key(&p.operator_key)?;
    let (approvals, pin): (Vec<Approval>, String) = if p.reapprove_orphaned {
        let prep = call(client, "workload.cog.checkout.approve", json!({ "reapprove_orphaned": true, "prepare": true })).await?;
        let mesh_id = prep["mesh_id"].as_str().ok_or_else(|| anyhow::anyhow!("no mesh id in the reply"))?.to_owned();
        let signed: Vec<clawft_kernel::licence::SignedApproval> = serde_json::from_value(prep["orphaned_signed"].clone())?;
        let (out, skipped, digest) = reapprovals(&key, &signed, &mesh_id, now());
        println!("{} orphaned approval(s) signed by this operator key; {skipped} skipped (another key or not orphaned)", out.len());
        println!("batch digest {digest}");
        (out, digest)
    } else {
        let (cog, version) = parse_ref(p.reference.as_deref().unwrap_or_default()).map_err(anyhow::Error::msg)?;
        let prep = call(
            client,
            "workload.cog.checkout.approve",
            json!({ "cog_id": cog, "version": version, "prepare": true, "sha256": p.sha256 }),
        )
        .await?;
        let a = approval_from(&prep, now()).map_err(anyhow::Error::msg)?;
        let key_now = a.content_key();
        if prep["content_key"].as_str() != Some(key_now.as_str()) {
            anyhow::bail!("the daemon's content key does not match the content it named; nothing signed");
        }
        println!("content key {key_now}");
        (vec![a], key_now)
    };
    if approvals.is_empty() {
        println!("nothing to sign");
        return Ok(());
    }
    for a in &approvals {
        println!("approve {}@{} for mesh {}", a.cog_id, a.version, a.mesh_id.get(..12).unwrap_or(&a.mesh_id));
        for h in &a.sha256 {
            println!("  sha256 {h}");
        }
    }
    match p.confirm.as_deref() {
        None => {
            println!("nothing signed: compare these hashes with the registry or the upstream release, then run again with --confirm {pin}");
            return Ok(());
        }
        Some(c) if c.trim() != pin => {
            anyhow::bail!("--confirm {c} does not match the content prepared now ({pin}): it changed since the dry run; nothing signed");
        }
        Some(_) => {}
    }
    let signed: Vec<_> = approvals.iter().map(|a| sign_approval(a, &key)).collect::<Result<_, _>>().map_err(|e| anyhow::anyhow!("{e}"))?;
    let done = call(client, "workload.cog.checkout.approve", json!({ "signed": signed })).await?;
    for d in done["approved"].as_array().into_iter().flatten() {
        println!("approved {}@{} ({}, chained as cog.checkout.approved)", d["cog_id"].as_str().unwrap_or("?"), d["version"].as_str().unwrap_or("?"), d["receipt"].as_str().unwrap_or("?"));
    }
    Ok(())
}

/// Plain-text `status`.
pub fn render_status(st: &Value, explain: bool) -> String {
    let mut out = String::new();
    let b = st.get("binding").filter(|b| !b.is_null());
    out.push_str(&format!("mesh id   {}\n", st["mesh_id"].as_str().unwrap_or("none")));
    match b {
        None => out.push_str("binding   none in effect\n"),
        Some(b) => out.push_str(&format!("binding   Seed {} seq {}\n", b["device_id"].as_str().unwrap_or("?"), b["seq"])),
    }
    if let Some(s) = st["steward"].as_str() {
        let me = if st["is_steward"].as_bool() == Some(true) { " (this node)" } else { "" };
        let reach = match st["steward_reachable"].as_bool() {
            Some(true) => "reachable",
            Some(false) if !me.is_empty() => "NO RELAY",
            _ => "NOT REACHABLE",
        };
        out.push_str(&format!("steward   {s}{me}  {reach}\n"));
    }
    let grants = st["grants"].as_array().cloned().unwrap_or_default();
    if grants.is_empty() {
        out.push_str("grants    none\n");
    }
    for g in &grants {
        let r = &g["grant"];
        out.push_str(&format!(
            "grant     {}@{} seq {} {} expires_at {}\n",
            r["cog_id"].as_str().unwrap_or("?"),
            r["version"].as_str().unwrap_or("?"),
            r["seq"],
            if r["valid"].as_bool() == Some(true) { "valid" } else if r["withdrawn"].as_bool() == Some(true) { "WITHDRAWN" } else { "LAPSED" },
            r["expires_at"],
        ));
        if explain {
            for a in g["run_gate"].as_array().into_iter().flatten() {
                let v = &a["run_gate"];
                out.push_str(&format!("  {} {}  run gate: {}", a["arch"].as_str().unwrap_or("?"), a["sha256"].as_str().map_or("?", |s| s.get(..16).unwrap_or(s)), v["verdict"].as_str().unwrap_or("?")));
                if let Some(rem) = v["remedy"].as_str() {
                    out.push_str(&format!(" ({}; {rem})", v["reason"].as_str().unwrap_or("")));
                }
                out.push('\n');
            }
        }
    }
    let approvals = st["approvals"].as_array().cloned().unwrap_or_default();
    out.push_str(&format!("approvals {}\n", approvals.len()));
    for a in &approvals {
        out.push_str(&format!(
            "  {}@{} {} hash(es){}\n",
            a["cog_id"].as_str().unwrap_or("?"),
            a["version"].as_str().unwrap_or("?"),
            a["sha256"].as_array().map_or(0, Vec::len),
            if a["active"].as_bool() == Some(true) { "" } else { "  ORPHANED (weaver cog checkout approve --reapprove-orphaned)" },
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct W {
        #[command(subcommand)]
        top: Top,
    }

    #[derive(clap::Subcommand, Debug)]
    enum Top {
        Checkout(CheckoutArgs),
    }

    fn parse(args: &[&str]) -> Result<CheckoutArgs, clap::Error> {
        let mut v = vec!["weaver", "checkout"];
        v.extend_from_slice(args);
        W::try_parse_from(v).map(|w| match w.top {
            Top::Checkout(a) => a,
        })
    }

    #[test]
    fn checkout_approve_and_status_parse() {
        let a = parse(&["fall-detect@1.2.0", "--arch", "aarch64"]).unwrap();
        assert!(a.command.is_none());
        assert_eq!((a.reference.as_deref(), a.arch.as_deref()), (Some("fall-detect@1.2.0"), Some("aarch64")));

        let a = parse(&["approve", "fall-detect@1.2.0", "--operator-key", "/k", "--sha256", "ab", "--confirm", "k1"]).unwrap();
        let Some(CheckoutCmd::Approve(p)) = a.command else { panic!("approve") };
        assert_eq!((p.reference.as_deref(), p.sha256.as_slice(), p.confirm.as_deref()), (Some("fall-detect@1.2.0"), &["ab".to_string()][..], Some("k1")));
        assert!(parse(&["approve", "fall-detect@1.2.0", "--operator-key", "/k", "--confirm"]).is_err(), "--confirm needs the key");

        let a = parse(&["approve", "--reapprove-orphaned", "--operator-key", "/k"]).unwrap();
        assert!(matches!(a.command, Some(CheckoutCmd::Approve(ApproveArgs { reapprove_orphaned: true, .. }))));
        assert!(parse(&["approve", "fall-detect@1", "--reapprove-orphaned", "--operator-key", "/k"]).is_err());
        assert!(parse(&["approve", "--operator-key", "/k"]).is_err(), "a reference or --reapprove-orphaned");
        assert!(parse(&["approve", "fall-detect@1"]).is_err(), "the operator key is required");

        let a = parse(&["status", "--explain"]).unwrap();
        assert!(matches!(a.command, Some(CheckoutCmd::Status { explain: true, json: false })));
    }

    #[test]
    fn references_need_a_cog_and_a_version() {
        assert_eq!(parse_ref("fall-detect@1.2.0").unwrap(), ("fall-detect".into(), "1.2.0".into()));
        assert_eq!(parse_ref("fall-detect@latest").unwrap().1, "latest");
        for bad in ["fall-detect", "@1", "x@", "a@b@c"] {
            assert!(parse_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_signed_approval_is_built_from_the_prepare_reply() {
        let prep = json!({"mesh_id": "m", "cog_id": "fall-detect", "version": "1.2.0", "sha256": ["aa", "bb"]});
        let a = approval_from(&prep, 7).unwrap();
        assert_eq!((a.v, a.approved_at, a.sha256.len()), (1, 7, 2));
        assert!(approval_from(&json!({"mesh_id": "m", "cog_id": "c", "version": "1", "sha256": []}), 1).is_err());
        assert!(approval_from(&json!({}), 1).is_err());
    }

    #[test]
    fn reapproval_keeps_only_this_operators_orphans_and_pins_a_digest() {
        use clawft_kernel::licence::sign_approval;
        let op = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let other = ed25519_dalek::SigningKey::from_bytes(&[2; 32]);
        let a = |mesh: &str| Approval { v: 1, mesh_id: mesh.into(), cog_id: "fall-detect".into(), version: "1.2.0".into(), sha256: vec!["ab".repeat(32)], approved_at: 5 };
        let (old, new) = ("aa".repeat(32), "bb".repeat(32));
        let signed = vec![
            sign_approval(&a(&old), &op).unwrap(),
            sign_approval(&a(&old), &other).unwrap(), // another key: skipped
            sign_approval(&a(&new), &op).unwrap(),    // already for the new mesh: skipped
        ];
        let (out, skipped, digest) = reapprovals(&op, &signed, &new, 9);
        assert_eq!((out.len(), skipped), (1, 2));
        assert_eq!((out[0].mesh_id.as_str(), out[0].approved_at), (new.as_str(), 9));
        assert_eq!(digest.len(), 16);
        // The digest pins the content: another set gives another digest.
        let (_, _, d2) = reapprovals(&op, &signed[..1], &"cc".repeat(32), 9);
        assert_ne!(digest, d2);
        assert_eq!(reapprovals(&op, &signed, &new, 99).2, digest, "approved_at is not part of it");
    }

    #[test]
    fn status_shows_the_steward_the_gate_and_orphaned_approvals() {
        let st = json!({
            "mesh_id": "m", "binding": {"device_id": "seed-1", "seq": 2}, "steward": "node-a", "is_steward": false,
            "steward_reachable": false,
            "grants": [{"grant": {"cog_id": "fall-detect", "version": "1.2.0", "seq": 3, "valid": true, "expires_at": 9},
                        "run_gate": [{"arch": "aarch64", "sha256": "ab".repeat(32),
                                      "run_gate": {"verdict": "no_approval", "reason": "no operator approval covers this binary",
                                                   "remedy": "weaver cog checkout approve fall-detect@1.2.0"}}]}],
            "approvals": [{"cog_id": "fall-detect", "version": "1.1.0", "sha256": ["aa"], "active": false}],
        });
        let t = render_status(&st, true);
        assert!(t.contains("steward   node-a  NOT REACHABLE"), "{t}");
        assert!(t.contains("run gate: no_approval") && t.contains("checkout approve fall-detect@1.2.0"), "{t}");
        assert!(t.contains("ORPHANED"), "{t}");
        assert!(!render_status(&st, false).contains("run gate"));
    }
}
