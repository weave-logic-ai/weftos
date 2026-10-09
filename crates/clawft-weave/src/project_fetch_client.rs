//! The member's side of `project.fetch` (ADR-108 P3b): the `weftos://` URL,
//! the channel the calls travel on, and the chunked download with checksums.
//!
//! Two channels carry the same request bodies: [`PlaneChannel`] (inside the
//! user daemon, straight onto the signed `workload.ctl` wire) and
//! [`DaemonChannel`] (the `git-remote-weftos` helper, over the daemon's local
//! RPC socket, which forwards to the primary). A download opens a session,
//! pulls every chunk in order, checks each chunk's SHA-256 and the whole
//! file's, and closes the session; a mismatch fails the fetch. Bundles land in
//! the repository through git itself (`git bundle unbundle` for the helper,
//! `git fetch <bundle>` for an install), tars through the checked unpack in
//! [`crate::project_fetch_tar`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use clawft_kernel::workload_ctl::{PlacementControlPlane, msg::method};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::project_fetch_repos::{Refs, git, is_oid, ref_ok};
use crate::project_fetch_tar::{Unpacked, unpack_tar};

/// URL scheme of the remote helper.
pub const SCHEME: &str = "weftos://";
/// Local daemon RPC verb the helper calls (forwarded as the node-admin method).
pub const LOCAL_RPC: &str = "project.fetch";
const FETCH_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// `weftos://<mesh node id>/<ULID>/<dir>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUrl {
    pub node: String,
    pub project: String,
    pub dir: String,
}

fn node_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 128 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
}

impl RemoteUrl {
    /// Parse and validate; `dir` defaults to `.`.
    pub fn parse(url: &str) -> Result<Self, String> {
        let rest = url.trim().strip_prefix(SCHEME).ok_or("url must start with weftos://")?;
        let mut it = rest.trim_end_matches('/').splitn(3, '/');
        let node = it.next().unwrap_or("").to_owned();
        let project = it.next().unwrap_or("").to_owned();
        let dir = it.next().filter(|d| !d.is_empty()).unwrap_or(".").to_owned();
        if !node_ok(&node) {
            return Err("url: bad node id".into());
        }
        clawft_types::project::validate_id(&project).map_err(|_| "url: project must be a ULID".to_string())?;
        if !crate::project_install::dir_ok(&dir) {
            return Err("url: dir must be '.' or one plain path segment".into());
        }
        Ok(Self { node, project, dir })
    }

    /// The URL as git keeps it in `remote.origin.url`.
    pub fn url(&self) -> String {
        format!("{SCHEME}{}/{}/{}", self.node, self.project, self.dir)
    }
}

/// Carries one `project.fetch` body to the primary and returns its result.
#[async_trait]
pub trait FetchChannel: Send + Sync {
    async fn call(&self, body: Value) -> Result<Value, String>;
}

/// Inside the daemon: straight onto the signed mesh wire.
pub struct PlaneChannel {
    pub plane: Arc<PlacementControlPlane>,
    pub node: String,
}

#[async_trait]
impl FetchChannel for PlaneChannel {
    async fn call(&self, body: Value) -> Result<Value, String> {
        self.plane.node_admin(&self.node, method::PROJECT_FETCH, body).await.map_err(|e| e.to_string())
    }
}

/// From a helper process: the local daemon forwards to the primary.
pub struct DaemonChannel {
    pub node: String,
    /// Socket path; `None` is the daemon's default.
    pub socket: Option<PathBuf>,
    client: tokio::sync::Mutex<Option<clawft_rpc::DaemonClient>>,
}

impl DaemonChannel {
    pub fn new(node: String, socket: Option<PathBuf>) -> Self {
        Self { node, socket, client: tokio::sync::Mutex::new(None) }
    }
}

#[async_trait]
impl FetchChannel for DaemonChannel {
    async fn call(&self, body: Value) -> Result<Value, String> {
        let mut guard = self.client.lock().await;
        if guard.is_none() {
            let c = match &self.socket {
                Some(p) => clawft_rpc::DaemonClient::connect_path(p).await,
                None => clawft_rpc::DaemonClient::connect().await,
            };
            *guard = Some(c.ok_or("the WeftOS user daemon is not running (weaver kernel boot)")?);
        }
        let req = clawft_rpc::Request::with_params(LOCAL_RPC, json!({ "node": self.node, "body": body }));
        let resp = match guard.as_mut().unwrap().call(req).await {
            Ok(r) => r,
            Err(e) => {
                *guard = None;
                return Err(format!("daemon: {e}"));
            }
        };
        if resp.ok {
            Ok(resp.result.unwrap_or(Value::Null))
        } else {
            Err(resp.error.unwrap_or_else(|| "daemon refused".into()))
        }
    }
}

/// An opened transfer.
#[derive(Debug, Clone)]
pub struct Opened {
    pub bytes: u64,
    /// The open result minus the session fields (`files`, `archived`, ...).
    pub meta: Value,
}

/// The project's repositories and non-git summary (`op: list`).
pub async fn list(ch: &dyn FetchChannel, project: &str) -> Result<Value, String> {
    ch.call(json!({ "op": "list", "project": project })).await
}

/// Branches, tags and HEAD of one repository (`op: refs`).
pub async fn refs(ch: &dyn FetchChannel, project: &str, dir: &str) -> Result<Refs, String> {
    let v = ch.call(json!({ "op": "refs", "project": project, "dir": dir })).await?;
    let refs = v["refs"]
        .as_array()
        .ok_or("refs: malformed answer")?
        .iter()
        .filter_map(|r| Some((r["oid"].as_str()?.to_owned(), r["name"].as_str()?.to_owned())))
        .filter(|(o, n)| is_oid(o) && ref_ok(n))
        .collect();
    let head = v["head"].as_str().filter(|h| ref_ok(h)).map(str::to_owned);
    Ok(Refs { refs, head })
}

/// Open a transfer with `open` and stream it to `out`. `None` when the primary
/// had nothing to send (`empty`).
pub async fn download(ch: &dyn FetchChannel, open: Value, out: &Path) -> Result<Option<Opened>, String> {
    let mut v = ch.call(open).await?;
    if v["empty"] == true {
        return Ok(None);
    }
    let session = v["session"].as_str().filter(|s| s.len() == 32).ok_or("open: no session")?.to_owned();
    let bytes = v["bytes"].as_u64().ok_or("open: no size")?;
    let sha256 = v["sha256"].as_str().ok_or("open: no checksum")?.to_owned();
    let chunk_bytes = v["chunk_bytes"].as_u64().filter(|c| (1..=200 * 1024).contains(c)).ok_or("open: bad chunk size")?;
    let chunks = v["chunks"].as_u64().ok_or("open: no chunk count")?;
    if chunks != bytes.div_ceil(chunk_bytes) {
        return Err("open: chunk count disagrees with size".into());
    }
    let result = pull(ch, &session, bytes, chunk_bytes, chunks, &sha256, out).await;
    let _ = ch.call(json!({ "op": "close", "session": session })).await;
    result?;
    if let Some(o) = v.as_object_mut() {
        for k in ["session", "sha256", "chunk_bytes", "chunks"] {
            o.remove(k);
        }
    }
    Ok(Some(Opened { bytes, meta: v }))
}

async fn pull(ch: &dyn FetchChannel, session: &str, bytes: u64, chunk_bytes: u64, chunks: u64, sha256: &str, out: &Path) -> Result<(), String> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| format!("write: {e}"))?);
    let mut total = Sha256::new();
    let mut got = 0u64;
    for i in 0..chunks {
        let c = ch.call(json!({ "op": "chunk", "session": session, "index": i })).await?;
        if c["index"].as_u64() != Some(i) {
            return Err("chunk: answer for another index".into());
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(c["data"].as_str().ok_or("chunk: no data")?)
            .map_err(|_| "chunk: bad encoding")?;
        let expect = (bytes - got).min(chunk_bytes) as usize;
        if data.len() != expect || hex::encode(Sha256::digest(&data)) != c["sha256"].as_str().unwrap_or("") {
            return Err(format!("chunk {i}: size or checksum mismatch"));
        }
        total.update(&data);
        f.write_all(&data).map_err(|e| format!("write: {e}"))?;
        got += data.len() as u64;
    }
    f.flush().map_err(|e| format!("write: {e}"))?;
    if got != bytes || hex::encode(total.finalize()) != sha256 {
        return Err("transfer checksum mismatch".into());
    }
    Ok(())
}

/// How a downloaded bundle enters the local repository.
pub enum BundleMode {
    /// Objects only; git updates refs itself (remote-helper `fetch`).
    Unbundle,
    /// `git fetch <bundle> <refspecs>`: objects and refs (an install).
    Fetch(Vec<String>),
}

/// Fetch `want` refnames of `project`/`dir` minus `have` ids into the
/// repository at `repo`. `Ok(None)` when nothing was missing.
pub async fn fetch_bundle(ch: &dyn FetchChannel, project: &str, dir: &str, want: &[String], have: &[String], repo: &Path, mode: BundleMode) -> Result<Option<u64>, String> {
    let tmp = tempfile::Builder::new().prefix("weftos-fetch-").suffix(".bundle").tempfile_in(repo).map_err(|e| format!("tmp: {e}"))?;
    let open = json!({ "op": "bundle.open", "project": project, "dir": dir, "want": want, "have": have });
    let Some(opened) = download(ch, open, tmp.path()).await? else { return Ok(None) };
    let path = tmp.path().to_str().ok_or("tmp path is not UTF-8")?;
    match mode {
        BundleMode::Unbundle => git(repo, &["bundle", "unbundle", path], None, FETCH_TIMEOUT)?,
        BundleMode::Fetch(specs) => {
            let mut args = vec!["fetch", "--quiet", "--no-write-fetch-head", path];
            args.extend(specs.iter().map(String::as_str));
            git(repo, &args, None, FETCH_TIMEOUT)?
        }
    };
    Ok(Some(opened.bytes))
}

/// What a non-git fetch did.
#[derive(Debug, Clone, Default)]
pub struct TarReport {
    /// Bytes received on the wire.
    pub bytes: u64,
    pub unpacked: Unpacked,
    pub archived: Vec<String>,
    pub excluded: u64,
    pub truncated: bool,
}

/// Fetch the project's non-git content under `dest`.
pub async fn fetch_tar(ch: &dyn FetchChannel, project: &str, dest: &Path) -> Result<Option<TarReport>, String> {
    let tmp = tempfile::Builder::new().prefix("weftos-fetch-").suffix(".tar").tempfile().map_err(|e| format!("tmp: {e}"))?;
    let Some(opened) = download(ch, json!({ "op": "tar.open", "project": project }), tmp.path()).await? else {
        return Ok(None);
    };
    let unpacked = unpack_tar(tmp.path(), dest)?;
    Ok(Some(TarReport {
        bytes: opened.bytes,
        unpacked,
        archived: opened.meta["archived"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
        excluded: opened.meta["excluded"].as_u64().unwrap_or(0),
        truncated: opened.meta["truncated"].as_bool().unwrap_or(false),
    }))
}
