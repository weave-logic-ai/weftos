//! `project.fetch` on the primary's `workload-host` (ADR-108 P3b): the node
//! holding a project's primary installation serves its repositories and
//! non-git content to a paired peer with a fetch grant.
//!
//! The node-admin channel is one signed request, one signed response, each at
//! most 256 KiB. A transfer is therefore a *spooled session*: an `open` call
//! writes a bundle or tar to a temp file under the runtime dir and answers with
//! its size, SHA-256 and chunk count; `chunk` calls read it back in
//! [`CHUNK_BYTES`] pieces (base64, each with its own SHA-256); `close` (or the
//! TTL) deletes it. Sessions are bound to the requesting node and re-checked
//! against the grant on every chunk, so a revocation stops a transfer in
//! flight.
//!
//! Operations (`body.op`):
//! - `list` — the project's repositories (`dir`, `head`, `branch`), the size of
//!   its non-git content and what is archived;
//! - `refs` — branches and tags of one repository (`dir`), and its HEAD;
//! - `bundle.open` — a `git bundle` of `want` refnames minus `have` ids;
//! - `tar.open` — the non-git content as a tar;
//! - `chunk` / `close` — session reads and release.
//!
//! Each open and every refusal is chained (`project.fetch`); chunks are not.
//! Results carry names, ids, sizes and hashes: never a file's content outside
//! the spooled stream, never anything under `.weftos/`.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use clawft_kernel::chain::ChainManager;
use clawft_kernel::workload_ctl::HOST_CHAIN_SOURCE;
use clawft_types::project::ProjectManifest;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::project_fetch_grants::authorize;
use crate::project_fetch_repos::{self as repos, Bundle, RepoEntry};
use crate::project_fetch_tar::{self as tarc, Exclusions};

/// Raw bytes per chunk: 128 KiB is 171 KiB of base64, two thirds of the
/// 256 KiB signed-payload limit with room for the envelope.
pub const CHUNK_BYTES: usize = 128 * 1024;
/// Chain kind of a fetch on either node.
pub const EVENT_PROJECT_FETCH: &str = "project.fetch";
/// Spool directory under the runtime dir.
pub const SPOOL_DIR: &str = "project-fetch-spool";
const SESSION_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_SESSIONS: usize = 32;
const MAX_PER_PEER: usize = 4;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    op: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    dir: Option<String>,
    #[serde(default)]
    want: Vec<String>,
    #[serde(default)]
    have: Vec<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    index: Option<u64>,
}

struct Session {
    requester: String,
    project: String,
    /// Deleted when the session is dropped.
    _file: tempfile::NamedTempFile,
    /// Opened once after the bundle or tar was written (git renames its
    /// output into place, so the temp file's own handle is stale).
    reader: std::fs::File,
    bytes: u64,
    chunks: u64,
    touched: Instant,
}

/// The primary's fetch server.
pub struct FetchHost {
    runtime_dir: PathBuf,
    manifests_dir: PathBuf,
    chain: Option<Arc<ChainManager>>,
    sessions: Mutex<HashMap<String, Session>>,
}

static GLOBAL: OnceLock<Arc<FetchHost>> = OnceLock::new();

/// Install the daemon's server (first call wins).
pub fn install_global(h: Arc<FetchHost>) -> bool {
    GLOBAL.set(h).is_ok()
}

/// The daemon's server, once placement is built.
pub fn global() -> Option<Arc<FetchHost>> {
    GLOBAL.get().cloned()
}

impl FetchHost {
    /// `runtime_dir` holds the peer and grant files and the spool;
    /// `manifests_dir` is the project index (`~/.weftos/projects`).
    pub fn new(runtime_dir: PathBuf, manifests_dir: PathBuf) -> Self {
        Self { runtime_dir, manifests_dir, chain: None, sessions: Mutex::new(HashMap::new()) }
    }

    /// Chain fetches and refusals here.
    pub fn with_chain(mut self, chain: Arc<ChainManager>) -> Self {
        self.chain = Some(chain);
        self
    }

    fn record(&self, payload: Value) {
        if let Some(c) = &self.chain {
            c.append(HOST_CHAIN_SOURCE, EVENT_PROJECT_FETCH, Some(payload));
        }
    }

    /// Answer one `project.fetch` request from the verified node `requester`.
    pub async fn serve(self: &Arc<Self>, requester: &str, body: &Value) -> Result<Value, String> {
        let body: Body = serde_json::from_value(body.clone()).map_err(|e| format!("project.fetch: {e}"))?;
        let (me, who) = (self.clone(), requester.to_owned());
        tokio::task::spawn_blocking(move || me.handle(&who, body)).await.map_err(|e| e.to_string())?
    }

    /// Blocking half. Opens and refusals are chained here; chunks are not.
    fn handle(&self, requester: &str, b: Body) -> Result<Value, String> {
        self.sweep();
        let chained = |ok: Result<Value, String>, project: &str, op: &str, dir: Option<&str>, bytes: u64| {
            self.record(json!({ "requester": requester, "project": project, "op": op, "dir": dir,
                "ok": ok.is_ok(), "bytes": bytes, "error": ok.as_ref().err() }));
            ok
        };
        match b.op.as_str() {
            "chunk" | "close" => self.session_op(requester, &b),
            op @ ("list" | "refs" | "bundle.open" | "tar.open") => {
                let project = b.project.clone().unwrap_or_default();
                let ctx = match self.project(requester, &project) {
                    Ok(c) => c,
                    Err(e) => return chained(Err(e), &project, op, b.dir.as_deref(), 0),
                };
                if op == "list" {
                    return chained(self.list(&ctx), &project, op, None, 0);
                }
                let dir = match b.dir.as_deref() {
                    Some(d) => d,
                    None if op == "tar.open" => ".",
                    None => return chained(Err("dir is required".into()), &project, op, None, 0),
                };
                let out = match op {
                    "refs" => self.refs(&ctx, dir),
                    "bundle.open" => self.bundle_open(requester, &ctx, dir, &b.want, &b.have),
                    _ => self.tar_open(requester, &ctx),
                };
                let bytes = out.as_ref().ok().and_then(|v| v["bytes"].as_u64()).unwrap_or(0);
                chained(out, &project, op, Some(dir), bytes)
            }
            other => chained(Err(format!("unknown op {other:?}")), "", other, None, 0),
        }
    }

    /// Authorise and locate the project.
    fn project(&self, requester: &str, project: &str) -> Result<Ctx, String> {
        clawft_types::project::validate_id(project).map_err(|_| "project must be a ULID".to_string())?;
        authorize(&self.runtime_dir, requester, project)?;
        let m = clawft_types::project::find_by_id(&self.manifests_dir, project)
            .map_err(|e| format!("project index: {e}"))?
            .ok_or("this node does not hold that project")?;
        if m.is_workspace() {
            return Err("this node holds a workspace copy of the project, not its primary".into());
        }
        if !m.root.is_dir() {
            return Err("the project's root is not a directory on this node".into());
        }
        let mut secret_dirs = vec![m.chain_dir()];
        secret_dirs.extend(m.runtime_dir_override().map(Path::to_path_buf));
        let extra = m.workspace_repos();
        let repos = repos::discover(&m.root, &extra);
        Ok(Ctx { manifest: m, repos, secret_dirs })
    }

    fn list(&self, ctx: &Ctx) -> Result<Value, String> {
        let repos: Vec<Value> = ctx
            .repos
            .iter()
            .map(|r| {
                let head = repos::list_refs(&r.path).ok().and_then(|x| x.head);
                json!({ "dir": r.dir, "head": repos::head_short(&r.path),
                        "branch": head.and_then(|h| h.strip_prefix("refs/heads/").map(str::to_owned)) })
            })
            .collect();
        let archive = tarc::read_archive(&ctx.manifest.root)?;
        let plan = tarc::plan(&ctx.exclusions(&archive));
        Ok(json!({
            "project": ctx.manifest.id, "repos": repos,
            "nongit": { "files": plan.files.len(), "bytes": plan.bytes, "large": plan.is_large(),
                        "largest": plan.largest(5), "excluded": plan.excluded, "truncated": plan.truncated },
            "archived": plan.archived,
        }))
    }

    fn refs(&self, ctx: &Ctx, dir: &str) -> Result<Value, String> {
        let repo = ctx.repo(dir)?;
        let r = repos::list_refs(&repo)?;
        let refs: Vec<Value> = r.refs.iter().map(|(oid, name)| json!({ "oid": oid, "name": name })).collect();
        Ok(json!({ "dir": dir, "refs": refs, "head": r.head }))
    }

    fn bundle_open(&self, requester: &str, ctx: &Ctx, dir: &str, want: &[String], have: &[String]) -> Result<Value, String> {
        let repo = ctx.repo(dir)?;
        let file = self.spool_file()?;
        match repos::make_bundle(&repo, file.path(), want, have)? {
            Bundle::Empty => Ok(json!({ "empty": true, "dir": dir })),
            Bundle::Written { .. } => self.open_session(requester, &ctx.manifest.id, file, json!({ "dir": dir })),
        }
    }

    fn tar_open(&self, requester: &str, ctx: &Ctx) -> Result<Value, String> {
        let archive = tarc::read_archive(&ctx.manifest.root)?;
        let plan = tarc::plan(&ctx.exclusions(&archive));
        let file = self.spool_file()?;
        tarc::build_tar(&ctx.manifest.root, &plan, file.path())?;
        let extra = json!({ "files": plan.files.len(), "content_bytes": plan.bytes, "archived": plan.archived,
                            "excluded": plan.excluded, "truncated": plan.truncated, "large": plan.is_large() });
        self.open_session(requester, &ctx.manifest.id, file, extra)
    }

    fn spool_file(&self) -> Result<tempfile::NamedTempFile, String> {
        let dir = self.runtime_dir.join(SPOOL_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| format!("spool: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        tempfile::Builder::new().prefix("fetch-").tempfile_in(&dir).map_err(|e| format!("spool: {e}"))
    }

    fn open_session(&self, requester: &str, project: &str, file: tempfile::NamedTempFile, extra: Value) -> Result<Value, String> {
        let (bytes, sha256) = hash_file(file.path())?;
        let reader = std::fs::File::open(file.path()).map_err(|e| format!("spool: {e}"))?;
        let chunks = bytes.div_ceil(CHUNK_BYTES as u64);
        let id = clawft_kernel::workload_ctl::msg::fresh_nonce();
        let mut s = self.sessions.lock().map_err(|_| "sessions poisoned")?;
        if s.len() >= MAX_SESSIONS || s.values().filter(|x| x.requester == requester).count() >= MAX_PER_PEER {
            return Err("too many open transfers; retry later".into());
        }
        s.insert(id.clone(), Session { requester: requester.into(), project: project.into(), _file: file, reader, bytes, chunks, touched: Instant::now() });
        let mut v = json!({ "session": id, "bytes": bytes, "sha256": sha256, "chunk_bytes": CHUNK_BYTES, "chunks": chunks });
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        Ok(v)
    }

    fn session_op(&self, requester: &str, b: &Body) -> Result<Value, String> {
        let id = b.session.as_deref().filter(|s| s.len() == 32).ok_or("session is required")?;
        let mut s = self.sessions.lock().map_err(|_| "sessions poisoned")?;
        let sess = s.get_mut(id).filter(|x| x.requester == requester).ok_or("no such transfer")?;
        if b.op == "close" {
            s.remove(id);
            return Ok(json!({ "closed": true }));
        }
        // The grant is re-read per chunk: a revocation stops the transfer here.
        if let Err(e) = authorize(&self.runtime_dir, requester, &sess.project) {
            let project = sess.project.clone();
            s.remove(id);
            self.record(json!({ "requester": requester, "project": project, "op": "chunk", "ok": false, "error": e }));
            return Err(e);
        }
        let index = b.index.ok_or("index is required")?;
        if index >= sess.chunks {
            return Err("chunk index out of range".into());
        }
        let offset = index * CHUNK_BYTES as u64;
        let len = (sess.bytes - offset).min(CHUNK_BYTES as u64) as usize;
        let mut buf = vec![0u8; len];
        sess.reader.seek(SeekFrom::Start(offset)).map_err(|e| format!("spool: {e}"))?;
        sess.reader.read_exact(&mut buf).map_err(|e| format!("spool: {e}"))?;
        sess.touched = Instant::now();
        Ok(json!({ "index": index, "sha256": hex::encode(Sha256::digest(&buf)),
                   "data": base64::engine::general_purpose::STANDARD.encode(&buf) }))
    }

    fn sweep(&self) {
        if let Ok(mut s) = self.sessions.lock() {
            s.retain(|_, x| x.touched.elapsed() < SESSION_TTL);
        }
    }

    /// Open transfers (tests and `doctor`).
    pub fn open_sessions(&self) -> usize {
        self.sessions.lock().map(|s| s.len()).unwrap_or(0)
    }
}

struct Ctx {
    manifest: ProjectManifest,
    repos: Vec<RepoEntry>,
    secret_dirs: Vec<PathBuf>,
}

impl Ctx {
    fn repo(&self, dir: &str) -> Result<PathBuf, String> {
        repos::resolve(&self.manifest.root, &self.manifest.workspace_repos(), dir)
    }

    fn exclusions<'a>(&'a self, archive: &'a [tarc::ArchiveEntry]) -> Exclusions<'a> {
        Exclusions { root: &self.manifest.root, archive, repos: &self.repos, secret_dirs: &self.secret_dirs }
    }
}

/// `(size, hex SHA-256)` of a file, streamed.
pub fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("hash: {e}"))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("hash: {e}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        h.update(&buf[..n]);
    }
    Ok((total, hex::encode(h.finalize())))
}
