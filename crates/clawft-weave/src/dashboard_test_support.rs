//! Test support for the dashboard reporter: a local fake dashboard (plain
//! HTTP on loopback, never the real one), a token directory, a child source
//! and a log capture.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::dashboard_cfg::DashboardConfig;
use crate::dashboard_report::{Ambient, ChildSource, Dashboard};

/// A request the fake dashboard saw.
#[derive(Debug, Clone)]
pub struct Seen {
    pub path: String,
    pub authorization: Option<String>,
    pub content_type: Option<String>,
    pub body: String,
}

type Hook = Box<dyn Fn(usize) + Send + Sync>;
type Answers = Arc<Mutex<HashMap<String, Vec<(u16, String)>>>>;

/// A fake dashboard: canned responses per path (the last one repeats), every
/// request recorded, an optional hook run before each answer.
pub struct FakeDash {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    answers: Answers,
    hook: Arc<Mutex<Option<Hook>>>,
}

impl FakeDash {
    pub async fn start() -> Self {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let answers: Answers = Arc::default();
        let hook: Arc<Mutex<Option<Hook>>> = Arc::default();
        let (s2, a2, h2) = (seen.clone(), answers.clone(), hook.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = l.accept().await else { return };
                let (s3, a3, h3) = (s2.clone(), a2.clone(), h2.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let head_end = loop {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                    let header = |name: &str| {
                        head.lines().find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
                        })
                    };
                    let len: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                    while buf.len() < head_end + len {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
                    let idx = {
                        let mut g = s3.lock().unwrap();
                        g.push(Seen {
                            path: path.clone(),
                            authorization: header("authorization"),
                            content_type: header("content-type"),
                            body: String::from_utf8_lossy(&buf[head_end..]).into_owned(),
                        });
                        g.len()
                    };
                    if let Some(h) = h3.lock().unwrap().as_ref() {
                        h(idx);
                    }
                    let (status, body) = {
                        let g = a3.lock().unwrap();
                        let q = g.get(&path);
                        let nth = s3.lock().unwrap().iter().filter(|r| r.path == path).count().saturating_sub(1);
                        q.and_then(|v| v.get(nth).or(v.last()).cloned()).unwrap_or((404, "{}".into()))
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Self { url, seen, answers, hook }
    }

    /// Answers for `path`, in order (the last repeats).
    pub fn answer(&self, path: &str, answers: &[(u16, &str)]) {
        self.answers
            .lock()
            .unwrap()
            .insert(path.into(), answers.iter().map(|(s, b)| (*s, (*b).to_owned())).collect());
    }

    /// Run `f(total requests so far)` before answering each request.
    pub fn on_request(&self, f: impl Fn(usize) + Send + Sync + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(f));
    }

    pub fn requests(&self, path: &str) -> Vec<Seen> {
        self.seen.lock().unwrap().iter().filter(|r| r.path == path).cloned().collect()
    }

    pub fn total(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

pub const NODE_ID: &str = "a5e70b99-a919-4ff1-9f44-5fcba69d439c";
pub const ULID: &str = "01K00000000000000000000000";

/// A token as the dashboard issues them (`wft_` + 64 hex).
pub fn token(c: char) -> String {
    format!("wft_{}", c.to_string().repeat(64))
}

/// A private directory holding `node.token` (0600) with `tok`.
pub fn token_dir(tok: &str) -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let p = dir.path().join("node.token");
    std::fs::write(&p, format!("{tok}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    (dir, p)
}

pub struct Kids(pub Vec<(String, String)>);

#[async_trait]
impl ChildSource for Kids {
    async fn children(&self) -> Vec<(String, String)> {
        self.0.clone()
    }
}

pub fn config(url: &str, token_file: &std::path::Path) -> DashboardConfig {
    DashboardConfig {
        enabled: true,
        url: url.into(),
        node_id: NODE_ID.into(),
        installation_id: "photo-gallery".into(),
        token_file: Some(token_file.to_path_buf()),
        units: vec![],
        ..Default::default()
    }
}

pub fn dashboard(cfg: DashboardConfig) -> Arc<Dashboard> {
    let ambient = Ambient {
        mesh_listen: Some("0.0.0.0:9470".into()),
        gateway_url: Some("http://127.0.0.1:8080".into()),
    };
    Dashboard::new(cfg, ambient, Arc::new(Kids(vec![(ULID.into(), "running".into())]))).unwrap()
}

/// Captures tracing output (all levels) written on this thread while the guard lives.
#[derive(Clone, Default)]
pub struct LogCapture(pub Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogCapture {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogCapture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl LogCapture {
    pub fn install(&self) -> tracing::subscriber::DefaultGuard {
        tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(self.clone())
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .finish(),
        )
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}
