//! `weaver workload node bind | unbind | status` (ADR-106 phase 1d).
//!
//! The CLI is where the operator key lives. `bind` first asks the daemon for
//! the Seed's live identity and the mesh facts (`workload.node.bind` with
//! `prepare`), checks the grant key fingerprint the operator typed against
//! the key it was given, signs a v2 binding record, and sends it back for the
//! steward checks. The daemon never sees the operator key.
//!
//! Naming: the verbs follow the RPC (`workload.node.bind` is
//! `weaver workload node bind`), beside `weaver workload place`.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use clawft_kernel::licence::{BindState, BindingRecord, key_id, sign_binding};
use clawft_kernel::workload_pkg::codec::hex_decode_exact;
use clawft_kernel::workload_pkg::signing_key_from_hex;
use clawft_rpc::DaemonClient;
use serde_json::{Value, json};

use crate::protocol::Request;

/// `weaver workload node` subcommands.
#[derive(Subcommand, Debug)]
pub enum NodeCmd {
    /// Bind a Seed to this mesh (operator-signed, steward-verified).
    Bind(BindArgs),
    /// Withdraw the Seed binding (operator-signed; needs no Seed).
    Unbind(UnbindArgs),
    /// Forget the grant clock high-water mark and restart it from now (undoes
    /// a forward clock jump). Shows the floor and the grants it would revive;
    /// changes nothing without `--confirm <floor>`.
    ResetFloor {
        /// Apply the reset (chained), pinned to the floor the dry run printed;
        /// refused when the floor has moved since.
        #[arg(long, value_name = "FLOOR", num_args = 0..=1)]
        confirm: Option<Option<String>>,
    },
    /// Mesh id and the held binding.
    Status {
        /// Print raw JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `weaver workload node bind`.
#[derive(Args, Debug)]
pub struct BindArgs {
    /// Operator-assigned Seed node id (from the daemon's workload-seeds.json).
    pub seed_node_id: String,
    /// Operator key file (hex seed; `weaver workload keygen`), pinned on the daemon.
    #[arg(long)]
    pub operator_key: PathBuf,
    /// The grant public key `weft-licence init` printed (64 hex).
    #[arg(long)]
    pub grant_pubkey: String,
    /// The grant key fingerprint `weft-licence init` printed (`ed25519:` + 16 hex).
    /// Compared with the key above before anything is signed.
    #[arg(long)]
    pub grant_fingerprint: String,
}

/// `weaver workload node unbind`.
#[derive(Args, Debug)]
pub struct UnbindArgs {
    /// Operator key file (hex seed), pinned on the daemon.
    #[arg(long)]
    pub operator_key: PathBuf,
}

fn load_key(path: &Path) -> Result<ed25519_dalek::SigningKey, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    signing_key_from_hex(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The grant key's fingerprint, checked against what the operator typed.
/// The operator compared it over the USB link; this is the second comparison.
pub fn confirm_fingerprint(grant_pubkey: &str, typed: &str) -> Result<String, String> {
    let key = hex_decode_exact::<32>(grant_pubkey).ok_or("--grant-pubkey must be 64 hex characters")?;
    let computed = key_id(&key);
    if computed != typed.trim() {
        return Err(format!(
            "the grant key's fingerprint is {computed}, not the {} you gave; they must match \
             what `weft-licence init` printed. Nothing was signed",
            typed.trim()
        ));
    }
    Ok(computed)
}

fn s<'a>(v: &'a Value, k: &str) -> Result<&'a str, String> {
    v.get(k).and_then(Value::as_str).ok_or_else(|| format!("the daemon's reply has no {k}"))
}

/// The record the operator signs for a bind, from the `prepare` reply.
pub fn bind_record(prep: &Value, grant_pubkey: &str) -> Result<BindingRecord, String> {
    Ok(BindingRecord {
        v: 2,
        device_id: s(prep, "device_id")?.into(),
        device_pubkey: s(prep, "device_pubkey")?.into(),
        mesh_id: s(prep, "mesh_id")?.into(),
        grant_pubkey: grant_pubkey.into(),
        steward_node_id: s(prep, "steward_node_id")?.into(),
        steward_pubkey: s(prep, "steward_pubkey")?.into(),
        state: BindState::Bound,
        seq: prep.get("next_seq").and_then(Value::as_u64).ok_or("the daemon's reply has no next_seq")?,
        bound_at: prep.get("now").and_then(Value::as_u64).ok_or("the daemon's reply has no now")?,
    })
}

/// The record that withdraws the held binding, from `workload.node.binding`.
/// It names the current local mesh id and the next `seq`.
pub fn unbind_record(status: &Value, now: u64) -> Result<BindingRecord, String> {
    let held = status
        .get("binding")
        .and_then(|b| b.get("record"))
        .filter(|r| !r.is_null())
        .ok_or("no Seed binding is held on this node")?;
    let mut rec: BindingRecord =
        serde_json::from_value(held.clone()).map_err(|e| format!("held binding: {e}"))?;
    rec.mesh_id = status
        .get("mesh_id")
        .and_then(Value::as_str)
        .ok_or("this node has no mesh id (set kernel.mesh.mesh_nonce and genesis_hash)")?
        .into();
    rec.state = BindState::Unbound;
    rec.seq = status.get("next_seq").and_then(Value::as_u64).ok_or("the daemon's reply has no next_seq")?;
    rec.bound_at = now;
    Ok(rec)
}

async fn call(client: &mut DaemonClient, method: &str, params: Value) -> anyhow::Result<Value> {
    let resp = client.call(Request::with_params(method, params)).await?;
    if !resp.ok {
        anyhow::bail!("{}", resp.error.unwrap_or_default());
    }
    Ok(resp.result.unwrap_or_default())
}

/// Run one `node` verb against the daemon.
pub async fn run(cmd: NodeCmd, client: &mut DaemonClient) -> anyhow::Result<()> {
    match cmd {
        NodeCmd::Status { json } => {
            let st = call(client, "workload.node.binding", json!({})).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&st)?);
            } else {
                print!("{}", render_status(&st));
            }
        }
        NodeCmd::ResetFloor { confirm } => reset_floor(client, confirm).await?,
        NodeCmd::Bind(a) => {
            let key = load_key(&a.operator_key).map_err(anyhow::Error::msg)?;
            let fp = confirm_fingerprint(&a.grant_pubkey, &a.grant_fingerprint).map_err(anyhow::Error::msg)?;
            let prep =
                call(client, "workload.node.bind", json!({ "seed_node_id": a.seed_node_id, "prepare": true })).await?;
            let rec = bind_record(&prep, &a.grant_pubkey).map_err(anyhow::Error::msg)?;
            println!("Seed        {} (identity read live over the pinned link)", rec.device_id);
            println!("mesh id     {}", rec.mesh_id);
            println!("grant key   {fp} (matches what you confirmed)");
            println!("steward     {}   seq {}", rec.steward_node_id, rec.seq);
            let signed = sign_binding(&rec, &key).map_err(|e| anyhow::anyhow!("{e}"))?;
            let done = call(
                client,
                "workload.node.bind",
                json!({ "seed_node_id": a.seed_node_id, "signed": signed, "grant_fingerprint": fp }),
            )
            .await?;
            println!("bound: {}", done["bound"]["device_id"].as_str().unwrap_or("?"));
        }
        NodeCmd::Unbind(a) => {
            let key = load_key(&a.operator_key).map_err(anyhow::Error::msg)?;
            let st = call(client, "workload.node.binding", json!({})).await?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let rec = unbind_record(&st, now).map_err(anyhow::Error::msg)?;
            let signed = sign_binding(&rec, &key).map_err(|e| anyhow::anyhow!("{e}"))?;
            call(client, "workload.node.unbind", json!({ "signed": signed })).await?;
            println!("unbound: {} (seq {})", rec.device_id, rec.seq);
        }
    }
    Ok(())
}

/// The floor `--confirm` pins: `None` for a dry run; bare `--confirm` is
/// refused (the operator must name the floor the dry run showed).
pub fn confirm_floor(confirm: Option<Option<String>>) -> Result<Option<u64>, String> {
    match confirm {
        None => Ok(None),
        Some(None) => Err("--confirm needs the floor the dry run printed: --confirm <floor>".into()),
        Some(Some(v)) => v.trim().parse::<u64>().map(Some).map_err(|_| format!("--confirm {v}: the floor is a number of unix seconds")),
    }
}

/// `reset-floor`, shared by `weaver workload node` and `weaver cog checkout`:
/// a dry run prints the floor; `--confirm <floor>` resets only if the floor is
/// still that value.
pub async fn reset_floor(client: &mut DaemonClient, confirm: Option<Option<String>>) -> anyhow::Result<()> {
    let pinned = confirm_floor(confirm).map_err(anyhow::Error::msg)?;
    let r = match pinned {
        None => call(client, "workload.node.reset-floor", json!({})).await?,
        Some(floor) => call(client, "workload.node.reset-floor", json!({ "confirm": true, "floor": floor })).await?,
    };
    print!("{}", render_floor_preview(&r["preview"]));
    if r["applied"].as_bool() == Some(true) {
        println!("floor reset (chained as licence.floor_reset_requested and licence.floor_reset)");
    } else {
        println!("nothing changed; to apply: --confirm {}", r["preview"]["floor"]);
    }
    Ok(())
}

/// The floor now, after a reset, and the grants a reset would revive.
pub fn render_floor_preview(p: &Value) -> String {
    let mut out = format!(
        "now          {}\nfloor now    {}\nfloor after  {}\n",
        p["now"], p["floor"], p["floor_after"]
    );
    match p["revived"].as_array().filter(|a| !a.is_empty()) {
        None => out.push_str("revives      no grant\n"),
        Some(list) => {
            out.push_str("revives      these grants (expired only by the floor):\n");
            for g in list {
                out.push_str(&format!(
                    "  {} {} (expires_at {})\n",
                    g["cog_id"].as_str().unwrap_or("?"),
                    g["version"].as_str().unwrap_or("?"),
                    g["expires_at"]
                ));
            }
        }
    }
    out
}

/// Plain-text `status`.
pub fn render_status(st: &Value) -> String {
    let mut out = String::new();
    if st["installed"].as_bool() == Some(false) || st["reason"].is_string() {
        out.push_str(&format!("licence   not running here: {}\n", st["reason"].as_str().unwrap_or("?")));
    }
    match st["reserved_holder"].as_bool() {
        Some(true) => out.push_str("reserved  this daemon holds the service's licence topics\n"),
        Some(false) => out.push_str("reserved  NOT the holder (another tenant's daemon; the owner's runs the licence path)\n"),
        None if st["holder_state"] == "unknown" => out.push_str("reserved  UNKNOWN (the mesh service could not be asked)\n"),
        None => {}
    }
    if let Some(k) = st["steward"]["pubkey"].as_str() {
        out.push_str(&format!("steward   key {k} (the binding's steward_pubkey)\n"));
    }
    let id = st["mesh_id"].as_str();
    out.push_str(&format!("mesh id   {}\n", id.unwrap_or("none (no kernel.mesh.mesh_nonce)")));
    match st.get("binding").filter(|b| !b.is_null()) {
        None => out.push_str("binding   none\n"),
        Some(b) => {
            out.push_str(&format!(
                "binding   {} Seed {} seq {} grant key {}{}\n",
                b["state"].as_str().unwrap_or("?"),
                b["device_id"].as_str().unwrap_or("?"),
                b["seq"],
                b["grant_fingerprint"].as_str().unwrap_or("?"),
                if b["orphaned"].as_bool() == Some(true) { "  ORPHANED (mesh id changed)" } else { "" },
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRANT: &str = "0202020202020202020202020202020202020202020202020202020202020202";

    #[test]
    fn the_typed_fingerprint_must_match_the_key() {
        let fp = key_id(&[2u8; 32]);
        assert_eq!(confirm_fingerprint(GRANT, &fp).unwrap(), fp);
        assert_eq!(confirm_fingerprint(GRANT, &format!(" {fp}\n")).unwrap(), fp);
        let e = confirm_fingerprint(GRANT, "ed25519:0000000000000000").unwrap_err();
        assert!(e.contains(&fp) && e.contains("Nothing was signed"), "{e}");
        assert!(confirm_fingerprint("zz", &fp).is_err());
    }

    #[test]
    fn the_bind_record_is_built_from_the_prepare_reply() {
        let prep = json!({"device_id": "seed-1", "device_pubkey": "aa", "mesh_id": "bb",
            "steward_node_id": "n", "steward_pubkey": "cc", "next_seq": 4, "now": 99});
        let r = bind_record(&prep, GRANT).unwrap();
        assert_eq!((r.v, r.seq, r.bound_at, r.state), (2, 4, 99, BindState::Bound));
        assert_eq!(r.grant_pubkey, GRANT);
        assert!(bind_record(&json!({}), GRANT).is_err());
    }

    #[test]
    fn the_unbind_record_names_the_current_mesh_and_the_next_seq() {
        let held = BindingRecord {
            v: 2, device_id: "seed-1".into(), device_pubkey: "aa".into(), mesh_id: "old".into(),
            grant_pubkey: GRANT.into(), steward_node_id: "n".into(), steward_pubkey: "cc".into(),
            state: BindState::Bound, seq: 3, bound_at: 1,
        };
        let st = json!({"mesh_id": "new", "next_seq": 4, "binding": {"record": held}});
        let r = unbind_record(&st, 50).unwrap();
        assert_eq!((r.mesh_id.as_str(), r.seq, r.state, r.bound_at), ("new", 4, BindState::Unbound, 50));
        assert_eq!(r.device_id, "seed-1");
        assert!(unbind_record(&json!({"binding": null}), 1).is_err());
        assert!(unbind_record(&json!({"mesh_id": null, "next_seq": 1, "binding": {"record": held}}), 1).is_err());
    }

    #[test]
    fn reset_floor_confirm_must_name_the_floor() {
        assert_eq!(confirm_floor(None), Ok(None));
        assert_eq!(confirm_floor(Some(Some("1790000000".into()))), Ok(Some(1_790_000_000)));
        assert!(confirm_floor(Some(None)).unwrap_err().contains("--confirm <floor>"));
        assert!(confirm_floor(Some(Some("soon".into()))).is_err());
        #[derive(clap::Parser, Debug)]
        struct W {
            #[command(subcommand)]
            c: NodeCmd,
        }
        use clap::Parser;
        let c = |a: &[&str]| W::try_parse_from(a).map(|w| w.c);
        assert!(matches!(c(&["w", "reset-floor"]).unwrap(), NodeCmd::ResetFloor { confirm: None }));
        assert!(matches!(c(&["w", "reset-floor", "--confirm"]).unwrap(), NodeCmd::ResetFloor { confirm: Some(None) }));
        assert!(matches!(c(&["w", "reset-floor", "--confirm", "12"]).unwrap(), NodeCmd::ResetFloor { confirm: Some(Some(_)) }));
    }

    #[test]
    fn the_floor_preview_names_the_grants_it_would_revive() {
        let p = json!({"now": 10, "floor": 99, "floor_after": 10,
            "revived": [{"cog_id": "fall-detect", "version": "1.0.0", "expires_at": 50}]});
        let t = render_floor_preview(&p);
        assert!(t.contains("fall-detect 1.0.0") && t.contains("floor after  10"), "{t}");
        assert!(render_floor_preview(&json!({"now": 1, "floor": 1, "floor_after": 1, "revived": []}))
            .contains("revives      no grant"));
    }

    #[test]
    fn status_marks_an_orphaned_binding() {
        let st = json!({"mesh_id": "b", "binding": {"state": "bound", "device_id": "seed-1", "seq": 2,
            "grant_fingerprint": "ed25519:x", "orphaned": true}});
        assert!(render_status(&st).contains("ORPHANED"));
        assert!(render_status(&json!({"mesh_id": null, "binding": null})).contains("binding   none"));
    }
}
