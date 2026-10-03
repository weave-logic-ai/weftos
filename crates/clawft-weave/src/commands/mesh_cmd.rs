//! `weaver mesh ...`: run the machine mesh service and administer it
//! (ADR-103, Phase 3 package S).
//!
//! `serve` runs the service in the foreground and refuses to run as root.
//! Every other verb is a mesh-local client: it connects to the service socket,
//! verifies the server (uid, machine key against `service.json` and the pin),
//! and sends one verb. Admin verbs connect as `role: "admin"` and hold no
//! keys; the service authorises them by peer credential (root or `admin_uids`).

use std::io::Write;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use clawft_mesh_local::hexser;
use clawft_mesh_local::proto::{ErrorKind, Message, Role, ServiceRecord};
use clawft_mesh_service::admin_client::{fingerprint, write_pin, AdminClient, AdminError, ConnectConfig};
use clawft_mesh_service::config::{DEFAULT_SOCKET, ENV_SOCKET};
use clawft_mesh_service::{MeshServiceConfig, Overrides};
use serde_json::Value;

const BUILD_VERSION: &str = env!("BUILD_VERSION");

/// `weaver mesh`.
#[derive(Args, Debug)]
pub struct MeshArgs {
    #[command(subcommand)]
    pub cmd: MeshCmd,
}

/// Where the service is and how to present the result.
#[derive(Args, Debug, Clone, Default)]
pub struct ConnArgs {
    /// Mesh-local socket (default: $WEFTOS_MESH_SOCKET, else /var/run/weftos/mesh.sock).
    #[arg(long)]
    pub socket: Option<PathBuf>,
    /// Machine-key pin to compare against (default: ~/.weftos/mesh/machine.pub when present).
    #[arg(long)]
    pub pin: Option<PathBuf>,
    /// Print the raw JSON reply.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// mesh.toml (production: /etc/weftos/mesh.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// State directory (default /var/lib/weftos/mesh; tests: a tempdir).
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    /// mesh-local socket path.
    #[arg(long)]
    pub socket: Option<PathBuf>,
    /// Mesh listener address (default 127.0.0.1:9489; 0.0.0.0 exposes it to the LAN).
    #[arg(long)]
    pub listen: Option<String>,
    /// Loopback health address, or `off` (default 127.0.0.1:9490).
    #[arg(long)]
    pub health_listen: Option<String>,
}

#[derive(Args, Debug)]
pub struct TrustArgs {
    #[command(flatten)]
    pub conn: ConnArgs,
    /// Replace an existing pin that holds a different key.
    #[arg(long)]
    pub replace: bool,
}

#[derive(Subcommand, Debug)]
pub enum MeshCmd {
    /// Run the machine mesh service in the foreground (never as root).
    Serve(ServeArgs),
    /// Service status: identity, policy, registrations, peers, journal.
    Status(ConnArgs),
    /// List bound, pending and revoked users (admin).
    Bindings(ConnArgs),
    /// Approve, revoke or rebind a user binding (admin).
    Bind {
        #[command(subcommand)]
        cmd: BindCmd,
    },
    /// Revoke or restore a mesh peer (admin).
    Peer {
        #[command(subcommand)]
        cmd: PeerCmd,
    },
    /// Verify the machine journal; acknowledge a quarantined tail (admin).
    Journal {
        #[command(subcommand)]
        cmd: JournalCmd,
    },
    /// Pin the service's machine key after verifying its fingerprint out of band.
    Trust(TrustArgs),
    /// Mesh nonce for the Seed licence mesh id (ADR-106); runs locally.
    Nonce {
        #[command(subcommand)]
        cmd: super::mesh_nonce::NonceCmd,
    },
    /// PRINT a reviewed shell script that installs the service (runs nothing).
    InstallService(super::mesh_install::InstallArgs),
    /// PRINT the inverse script (keeps node.key unless --purge-key; runs nothing).
    UninstallService(super::mesh_install::InstallArgs),
}

#[derive(Subcommand, Debug)]
pub enum BindCmd {
    /// Approve a bind waiting for approval. The key is named by its user id
    /// (fingerprint): by default the one pending now, shown before approving;
    /// the service refuses if the pending key is a different one.
    Approve {
        uid: u32,
        /// The pending key's user id, as shown by `weaver mesh bindings`.
        #[arg(long)]
        user_id: Option<String>,
        #[command(flatten)]
        conn: ConnArgs,
    },
    /// Revoke the user's binding and every certificate issued to its key.
    Revoke {
        uid: u32,
        #[arg(long, default_value = "")]
        reason: String,
        #[command(flatten)]
        conn: ConnArgs,
    },
    /// Replace the user's key (default: the key the uid last offered in conflict).
    Rebind {
        uid: u32,
        /// New user public key (64 hex).
        #[arg(long)]
        pubkey: Option<String>,
        #[command(flatten)]
        conn: ConnArgs,
    },
}

#[derive(Subcommand, Debug)]
pub enum PeerCmd {
    /// Refuse and disconnect a peer node id.
    Revoke {
        node_id: String,
        #[arg(long, default_value = "")]
        reason: String,
        #[command(flatten)]
        conn: ConnArgs,
    },
    /// Remove a peer revocation.
    Unrevoke {
        node_id: String,
        #[command(flatten)]
        conn: ConnArgs,
    },
}

#[derive(Subcommand, Debug)]
pub enum JournalCmd {
    /// Re-verify the hash chain and signatures; fails when it does not verify.
    Verify {
        /// Acknowledge a quarantined tail (journals what was lost; lifts read-only).
        #[arg(long)]
        accept_truncate: bool,
        /// Quarantine record to accept (default: the newest pending one).
        #[arg(long)]
        seq: Option<u64>,
        /// Raise the certificate serial floor (never lowers it).
        #[arg(long)]
        floor: Option<u64>,
        #[command(flatten)]
        conn: ConnArgs,
    },
}

/// Entry point from `main`.
pub async fn run(args: MeshArgs) -> Result<()> {
    execute(args.cmd, &mut std::io::stdout()).await
}

/// Run one command, writing its output to `w` (tests capture it).
pub async fn execute(cmd: MeshCmd, w: &mut dyn Write) -> Result<()> {
    match cmd {
        MeshCmd::Serve(a) => serve(a).await,
        MeshCmd::Status(c) => {
            let data = call_best(&c, Message::Status {}).await?;
            show(w, &c, &data, render_status)
        }
        MeshCmd::Bindings(c) => {
            let data = call(&c, Role::Admin, Message::BindingsList {}).await?;
            show(w, &c, &data, render_bindings)
        }
        MeshCmd::Bind { cmd } => match cmd {
            BindCmd::Approve { uid, user_id, conn } => {
                let user_id = match user_id {
                    Some(u) => u,
                    None => pending_user_id(&conn, uid).await?,
                };
                writeln!(w, "approving uid {uid} with the key whose user id is {user_id}")?;
                ack(w, &conn, Message::BindApprove { uid, user_id: Some(user_id) }, &format!("approved uid {uid}")).await
            }
            BindCmd::Revoke { uid, reason, conn } => {
                ack(w, &conn, Message::BindRevoke { uid, reason }, &format!("revoked uid {uid}")).await
            }
            BindCmd::Rebind { uid, pubkey, conn } => {
                if let Some(k) = &pubkey
                    && hexser::decode::<32>(k).is_none()
                {
                    bail!("--pubkey must be 64 lowercase hex characters");
                }
                ack(w, &conn, Message::BindRebind { uid, user_pubkey: pubkey }, &format!("rebound uid {uid}")).await
            }
        },
        MeshCmd::Peer { cmd } => match cmd {
            PeerCmd::Revoke { node_id, reason, conn } => {
                let done = format!(
                    "revoked peer {node_id}; its live connection is closed. Under admission = enforce it is refused \
                     when it reconnects; under observe (the default) the revocation is only recorded and it can reconnect"
                );
                ack(w, &conn, Message::PeerRevoke { node_id, reason }, &done).await
            }
            PeerCmd::Unrevoke { node_id, conn } => {
                let done = format!("restored peer {node_id}");
                ack(w, &conn, Message::PeerUnrevoke { node_id }, &done).await
            }
        },
        MeshCmd::Journal { cmd: JournalCmd::Verify { accept_truncate, seq, floor, conn } } => {
            journal_verify(w, &conn, accept_truncate, seq, floor).await
        }
        MeshCmd::Trust(t) => trust(w, t).await,
        MeshCmd::Nonce { cmd } => super::mesh_nonce::run(cmd, w),
        MeshCmd::InstallService(a) => super::mesh_install::run_install(&a, w),
        MeshCmd::UninstallService(a) => super::mesh_install::run_uninstall(&a, w),
    }
}

async fn serve(a: ServeArgs) -> Result<()> {
    let overrides = Overrides { state_dir: a.state_dir, socket: a.socket, listen: a.listen };
    let mut cfg = MeshServiceConfig::load(a.config.as_deref(), &overrides).context("mesh service configuration")?;
    cfg.build_sha = BUILD_VERSION.to_string();
    if let Some(h) = a.health_listen {
        cfg.health_listen = if h == "off" { None } else { Some(h) };
        cfg.validate()?;
    }
    clawft_mesh_service::run(cfg).await.map_err(|e| anyhow!(e))
}

pub(crate) fn socket_of(c: &ConnArgs) -> PathBuf {
    c.socket
        .clone()
        .or_else(|| std::env::var_os(ENV_SOCKET).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET))
}

fn default_pin() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".weftos/mesh/machine.pub"))
}

fn record_of(socket: &std::path::Path) -> Result<ServiceRecord> {
    let path = socket.parent().map(|d| d.join("service.json")).context("socket has no directory")?;
    ServiceRecord::load(&path).with_context(|| {
        format!("cannot read {} (is the mesh service running? `weaver mesh serve` or the installed unit)", path.display())
    })
}

async fn connect(c: &ConnArgs, role: Role, pin: bool) -> Result<AdminClient, AdminError> {
    let socket = socket_of(c);
    let service = record_of(&socket).map_err(|e| AdminError::Pin(format!("{e:#}")))?;
    let mut cc = ConnectConfig::new(socket, service, role);
    cc.build_sha = BUILD_VERSION.to_string();
    if pin {
        cc.pin = c.pin.clone().or_else(default_pin);
    }
    AdminClient::connect(&cc).await
}

async fn call(c: &ConnArgs, role: Role, m: Message) -> Result<Value> {
    let mut cl = connect(c, role, true).await?;
    let reply = cl.request(m).await?;
    cl.bye().await;
    match reply {
        Message::Reply { data } => Ok(data),
        Message::Ack {} => Ok(Value::Null),
        other => bail!("unexpected reply: {other:?}"),
    }
}

/// Admin when the caller is one (full detail), else the plain user view.
async fn call_best(c: &ConnArgs, m: Message) -> Result<Value> {
    match call(c, Role::Admin, m.clone()).await {
        Err(e) if matches!(e.downcast_ref::<AdminError>(), Some(AdminError::Server(b)) if b.kind == ErrorKind::Forbidden) => {
            call(c, Role::User, m).await
        }
        other => other,
    }
}

/// Status (best role) and, when this caller is an admin, the journal check;
/// read-only, for `weaver doctor`.
pub(crate) async fn status_for_doctor(c: &ConnArgs) -> Result<(Value, Option<Value>)> {
    let status = call_best(c, Message::Status {}).await?;
    let journal = call(c, Role::Admin, Message::JournalVerify {}).await.ok();
    Ok((status, journal))
}

/// The user id of the key pending approval for `uid`.
async fn pending_user_id(c: &ConnArgs, uid: u32) -> Result<String> {
    let data = call(c, Role::Admin, Message::BindingsList {}).await?;
    data["pending"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["principal"]["uid"].as_u64() == Some(u64::from(uid)))
        .and_then(|p| p["user_id"].as_str().map(str::to_string))
        .with_context(|| format!("nothing is pending approval for uid {uid}"))
}

async fn ack(w: &mut dyn Write, c: &ConnArgs, m: Message, done: &str) -> Result<()> {
    let data = call(c, Role::Admin, m).await?;
    writeln!(w, "{done}")?;
    if let Some(warning) = data["warning"].as_str() {
        writeln!(w, "WARNING: {warning}")?;
    }
    Ok(())
}

fn show(w: &mut dyn Write, c: &ConnArgs, data: &Value, render: fn(&mut dyn Write, &Value) -> Result<()>) -> Result<()> {
    if c.json {
        writeln!(w, "{}", serde_json::to_string_pretty(data)?)?;
        Ok(())
    } else {
        render(w, data)
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("-")
}

fn render_status(w: &mut dyn Write, d: &Value) -> Result<()> {
    let key = hexser::decode::<32>(s(d, "machine_pubkey"));
    writeln!(w, "node        {}", s(d, "node_id"))?;
    if let Some(k) = key {
        writeln!(w, "machine key {}  ({})", fingerprint(&k), s(d, "machine_pubkey"))?;
    }
    writeln!(w, "build       {}   proto {}..={}", s(d, "build_sha"), d["proto"]["min"], d["proto"]["max"])?;
    writeln!(w, "listen      {}   socket {}", s(d, "listen"), s(d, "socket"))?;
    writeln!(
        w,
        "policy      admission {}, bind {}, cluster owner uid {}",
        s(d, "admission"),
        s(d, "bind_policy"),
        d["cluster_owner_uid"].as_u64().map_or("-".to_string(), |u| u.to_string())
    )?;
    writeln!(w, "registered  {}", d["registered"])?;
    for p in d["force_revoked"].as_array().into_iter().flatten() {
        writeln!(
            w,
            "FORCE-REVOKED uid {} (enforced from force-revoked.json; the journal could not record it)",
            p["id"]
        )?;
    }
    for r in d["registrations"].as_array().into_iter().flatten() {
        writeln!(
            w,
            "  - uid {} user {} pid {} {}  delivered {} dropped {}",
            r["principal"]["id"], s(r, "user_id"), r["pid"], s(r, "exe"), r["delivered"], r["dropped_full"]
        )?;
    }
    let peers: Vec<&str> = d["peers"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    writeln!(w, "peers       {}", if peers.is_empty() { "none".to_string() } else { peers.join(", ") })?;
    let j = &d["journal"];
    writeln!(
        w,
        "journal     seq {} head {}{}{}",
        j["seq"],
        j["hash"].as_str().map_or("-", |h| &h[..h.len().min(16)]),
        if j["read_only"] == true { "  READ-ONLY (quarantined tail: `weaver mesh journal verify --accept-truncate`)" } else { "" },
        j["degraded"].as_str().map_or(String::new(), |why| format!("  DEGRADED: {why}")),
    )?;
    if j["last_auto_accept"].is_object() {
        writeln!(
            w,
            "            the service accepted a crash-torn journal tail on its own (seq {}, at {}); see `weaver mesh journal verify`",
            j["last_auto_accept"]["seq"], j["last_auto_accept"]["at"]
        )?;
    }
    let r = &d["router"];
    writeln!(
        w,
        "router      delivered {} sent {}/{} dropped: scope_required {} unknown_scope {} denied_scope {} no_tenant {} queue_full {}",
        r["delivered"], r["sent_remote"], r["sent_local"], r["scope_required"], r["unknown_scope"],
        r["denied_scope"], r["no_tenant"], r["dropped_full"]
    )?;
    Ok(())
}

fn render_bindings(w: &mut dyn Write, d: &Value) -> Result<()> {
    let who = |p: &Value| p["uid"].as_u64().map_or_else(|| p["sid"].to_string(), |u| format!("uid {u}"));
    for b in d["bound"].as_array().into_iter().flatten() {
        writeln!(
            w,
            "bound    {}  user {}  serials {}{}",
            who(&b["principal"]),
            s(b, "user_id"),
            b["serials"],
            if b["registered"] == true { "  (registered)" } else { "" }
        )?;
    }
    for p in d["pending"].as_array().into_iter().flatten() {
        writeln!(w, "pending  {}  user {}  -> `weaver mesh bind approve <uid>`", who(&p["principal"]), s(p, "user_id"))?;
    }
    for r in d["revoked"].as_array().into_iter().flatten() {
        writeln!(w, "revoked  {}", who(r))?;
    }
    for r in d["force_revoked"].as_array().into_iter().flatten() {
        writeln!(w, "FORCE-REVOKED  {}  (journal could not record it; fix the journal, then re-run the revoke)", who(&serde_json::json!({"uid": r["id"]})))?;
    }
    if d["read_only"] == true {
        writeln!(w, "note     the journal is read-only; binds and certificates are refused")?;
    }
    if let Some(why) = d["degraded"].as_str() {
        writeln!(w, "note     bindings are degraded: {why}")?;
    }
    Ok(())
}

async fn journal_verify(w: &mut dyn Write, c: &ConnArgs, accept: bool, seq: Option<u64>, floor: Option<u64>) -> Result<()> {
    let v = call(c, Role::Admin, Message::JournalVerify {}).await?;
    if c.json {
        writeln!(w, "{}", serde_json::to_string_pretty(&v)?)?;
    } else if v["ok"] == true {
        writeln!(w, "journal verifies: {} records, head seq {} hash {}", v["records"], v["head_seq"], s(&v, "head_hash"))?;
    } else {
        writeln!(w, "JOURNAL DOES NOT VERIFY: {} ({})", v["bad"]["reason"], v["bad"]["file"])?;
    }
    if v["last_auto_accept"].is_object() {
        writeln!(
            w,
            "the service accepted a crash-torn tail on its own at seq {} (time {}); nothing readable was lost",
            v["last_auto_accept"]["seq"], v["last_auto_accept"]["at"]
        )?;
    }
    if v["read_only"] == true {
        writeln!(
            w,
            "the journal is read-only after a quarantined tail (pending quarantine seq {})",
            v["latest_pending_quarantine"]
        )?;
        if v["pending_quarantines"].as_array().is_some_and(|a| a.len() > 1) {
            writeln!(
                w,
                "{} quarantines are pending ({}); one acceptance clears them all: review every journal.corrupt.* file first and pass --floor if an older one holds a higher serial",
                v["pending_quarantines"].as_array().map_or(0, Vec::len),
                v["pending_quarantines"]
            )?;
        }
    }
    if accept {
        call(c, Role::Admin, Message::JournalAcceptTruncate { quarantine_seq: seq, floor }).await?;
        writeln!(w, "accepted the quarantined tail; binds and certificates are allowed again")?;
    }
    if v["ok"] != true {
        bail!("the machine journal does not verify");
    }
    Ok(())
}

async fn trust(w: &mut dyn Write, t: TrustArgs) -> Result<()> {
    // No pin comparison here: the point is to show the key and record it.
    let cl = connect(&t.conn, Role::User, false).await?;
    let key = cl.ack.machine_pubkey;
    let (node, sha) = (cl.ack.node_id.clone(), cl.ack.service_build_sha.clone());
    cl.bye().await;
    writeln!(w, "service node {node} (build {sha})")?;
    writeln!(w, "machine key   {}", hexser::encode(&key))?;
    writeln!(w, "fingerprint   {}", fingerprint(&key))?;
    writeln!(w, "Compare this fingerprint out of band (`weaver mesh status` run on the service host as root or an admin shows the same one) before relying on the pin.")?;
    let path = t.conn.pin.clone().or_else(default_pin).context("no pin path: set --pin or HOME")?;
    write_pin(&path, &key, t.replace)?;
    writeln!(w, "pinned in {}", path.display())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct W {
        #[command(subcommand)]
        cmd: MeshCmd,
    }

    fn parse(args: &[&str]) -> MeshCmd {
        W::try_parse_from(std::iter::once("mesh").chain(args.iter().copied())).unwrap().cmd
    }

    #[test]
    fn verbs_parse() {
        assert!(matches!(parse(&["serve", "--state-dir", "/x", "--listen", "127.0.0.1:19489"]), MeshCmd::Serve(_)));
        assert!(matches!(parse(&["status", "--json"]), MeshCmd::Status(c) if c.json));
        assert!(matches!(parse(&["bind", "approve", "501"]), MeshCmd::Bind { cmd: BindCmd::Approve { uid: 501, .. } }));
        assert!(matches!(
            parse(&["bind", "revoke", "501", "--reason", "gone"]),
            MeshCmd::Bind { cmd: BindCmd::Revoke { uid: 501, ref reason, .. } } if reason == "gone"
        ));
        assert!(matches!(parse(&["bind", "rebind", "501", "--pubkey", "ab"]), MeshCmd::Bind { cmd: BindCmd::Rebind { .. } }));
        assert!(matches!(parse(&["peer", "unrevoke", "abc"]), MeshCmd::Peer { cmd: PeerCmd::Unrevoke { .. } }));
        assert!(matches!(
            parse(&["journal", "verify", "--accept-truncate", "--floor", "9"]),
            MeshCmd::Journal { cmd: JournalCmd::Verify { accept_truncate: true, floor: Some(9), .. } }
        ));
        assert!(matches!(parse(&["trust", "--replace"]), MeshCmd::Trust(t) if t.replace));
    }

    #[test]
    fn status_renders_the_important_lines() {
        let d = serde_json::json!({
            "node_id": "n", "machine_pubkey": "ab".repeat(32), "build_sha": "sha", "proto": {"min": 1, "max": 1},
            "listen": "0.0.0.0:9489", "socket": "/s", "admission": "observe", "bind_policy": "tofu",
            "cluster_owner_uid": 501, "registered": 1, "peers": ["p1"],
            "registrations": [{"principal": {"id": 501}, "user_id": "u", "pid": 7, "exe": "/bin/x", "delivered": 2, "dropped_full": 0}],
            "journal": {"seq": 4, "hash": "0123456789abcdef0123", "read_only": true, "degraded": null},
            "router": {"delivered": 1, "sent_remote": 0, "sent_local": 0, "scope_required": 0, "unknown_scope": 0, "denied_scope": 0, "no_tenant": 0, "dropped_full": 0},
        });
        let mut out = Vec::new();
        render_status(&mut out, &d).unwrap();
        let t = String::from_utf8(out).unwrap();
        assert!(t.contains("admission observe") && t.contains("cluster owner uid 501"), "{t}");
        assert!(t.contains("READ-ONLY") && t.contains("p1") && t.contains("uid 501 user u pid 7"), "{t}");
        assert!(t.contains("0123456789abcdef"), "{t}");
    }

    #[test]
    fn bindings_render_pending_with_the_approve_hint() {
        let d = serde_json::json!({
            "bound": [{"principal": {"uid": 501}, "user_id": "u1", "serials": [1, 2], "registered": true}],
            "pending": [{"principal": {"uid": 502}, "user_id": "u2"}],
            "revoked": [{"uid": 503}], "force_revoked": [{"kind": "uid", "id": 504}], "degraded": null, "read_only": false,
        });
        let mut out = Vec::new();
        render_bindings(&mut out, &d).unwrap();
        let t = String::from_utf8(out).unwrap();
        assert!(t.contains("bound    uid 501") && t.contains("(registered)"), "{t}");
        assert!(t.contains("pending  uid 502") && t.contains("bind approve"), "{t}");
        assert!(t.contains("revoked  uid 503"), "{t}");
        assert!(t.contains("FORCE-REVOKED  uid 504"), "{t}");
    }
}
