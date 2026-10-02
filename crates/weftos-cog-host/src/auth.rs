//! Request policy for the host's HTTP API: CSRF guard, CORS origin allowlist and the per-host bearer
//! token. The host binds all interfaces, so browsers on the LAN (or a malicious page in one) must not
//! be able to drive it:
//!
//! - every `POST` (except edge heartbeats) needs `Content-Type: application/json` and
//!   `X-Weft-Host: 1`; a custom header forces a CORS preflight, which only allowlisted origins pass;
//! - the `/hw/*` mutating and agent routes also need `Authorization: Bearer <token>`, where the token
//!   is `$WEFT_COG_HOST_TOKEN` or the 0600 file `<root>/host.token` created on first start.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

pub type Headers = HashMap<String, String>;
pub type Refusal = (&'static str, String);

pub const TOKEN_ENV: &str = "WEFT_COG_HOST_TOKEN";
pub const ORIGINS_ENV: &str = "WEFT_COG_HOST_ORIGINS";
const DEFAULT_ORIGINS: [&str; 3] = ["http://127.0.0.1:*", "http://localhost:*", "http://[::1]:*"];

pub fn token_path(root: &Path) -> PathBuf {
    root.join("host.token")
}

/// `n` random bytes from the OS as lowercase hex.
pub fn random_hex(n: usize) -> std::io::Result<String> {
    let mut buf = vec![0u8; n];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Write `bytes` to `path` atomically (unique temp name, then rename), mode 0600 on unix.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("tmp{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

#[derive(Clone, Debug)]
pub struct Policy {
    token: String,
    origins: Vec<String>,
}

fn refuse(code: &'static str, msg: &str, hint: &str) -> Refusal {
    (code, serde_json::json!({ "ok": false, "error": msg, "hint": hint }).to_string())
}

impl Policy {
    pub fn new(token: String, origins: Vec<String>) -> Self {
        Self { token, origins }
    }

    /// Token from `$WEFT_COG_HOST_TOKEN`, else `<root>/host.token` (created on first start);
    /// origins from `$WEFT_COG_HOST_ORIGINS` (comma separated) plus the loopback defaults.
    pub fn load(root: &Path) -> std::io::Result<Self> {
        let token = match std::env::var(TOKEN_ENV).ok().filter(|t| !t.trim().is_empty()) {
            Some(t) => t.trim().to_string(),
            None => match std::fs::read_to_string(token_path(root)).map(|t| t.trim().to_string()).ok().filter(|t| !t.is_empty()) {
                Some(t) => t,
                None => {
                    let t = random_hex(32)?;
                    write_private(&token_path(root), t.as_bytes())?;
                    t
                }
            },
        };
        let mut origins: Vec<String> = DEFAULT_ORIGINS.iter().map(|s| s.to_string()).collect();
        if let Ok(extra) = std::env::var(ORIGINS_ENV) {
            origins.extend(extra.split(',').map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty()));
        }
        Ok(Self { token, origins })
    }

    /// Is `origin` allowed to call the API cross-origin? Allowlist entries may end in `:*` (any
    /// port); the host's own origin (from the request's Host header) is always allowed.
    pub fn origin_allowed(&self, origin: &str, host_header: Option<&str>) -> bool {
        if host_header.is_some_and(|h| origin == format!("http://{h}")) {
            return true;
        }
        self.origins.iter().any(|p| match p.strip_suffix(":*") {
            Some(prefix) => origin.strip_prefix(prefix).and_then(|r| r.strip_prefix(':')).is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())),
            None => p == origin,
        })
    }

    /// CSRF guard for a POST: JSON content type, `X-Weft-Host: 1`, and (if the browser sent an
    /// Origin) an allowlisted one.
    pub fn check_post(&self, h: &Headers) -> Result<(), Refusal> {
        if let Some(o) = h.get("origin")
            && !self.origin_allowed(o, h.get("host").map(String::as_str))
        {
            return Err(refuse("403 Forbidden", "origin not allowed", &format!("set {ORIGINS_ENV} to allow it")));
        }
        let json = h.get("content-type").is_some_and(|c| c.trim().to_ascii_lowercase().starts_with("application/json"));
        if !json || h.get("x-weft-host").map(String::as_str) != Some("1") {
            return Err(refuse("400 Bad Request", "POST needs Content-Type: application/json and X-Weft-Host: 1", "send both headers"));
        }
        Ok(())
    }

    /// Bearer-token check for the mutating `/hw/*` routes.
    pub fn check_token(&self, h: &Headers) -> Result<(), Refusal> {
        let given = h.get("authorization").and_then(|a| a.strip_prefix("Bearer ")).map(str::trim).unwrap_or("");
        if ct_eq(given, &self.token) {
            Ok(())
        } else {
            Err(refuse("401 Unauthorized", "host token required", &format!("send Authorization: Bearer <token>; the token is in <root>/host.token or ${TOKEN_ENV}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol() -> Policy {
        Policy::new("sekrit".into(), DEFAULT_ORIGINS.iter().map(|s| s.to_string()).chain(["https://console.example".to_string()]).collect())
    }
    fn hs(v: &[(&str, &str)]) -> Headers {
        v.iter().map(|(k, x)| (k.to_string(), x.to_string())).collect()
    }

    #[test]
    fn origin_allowlist() {
        let p = pol();
        assert!(p.origin_allowed("http://localhost:8080", None));
        assert!(p.origin_allowed("http://127.0.0.1:9", None));
        assert!(p.origin_allowed("https://console.example", None));
        assert!(p.origin_allowed("http://10.0.0.5:9480", Some("10.0.0.5:9480"))); // own origin
        assert!(!p.origin_allowed("http://evil.example", Some("10.0.0.5:9480")));
        assert!(!p.origin_allowed("http://localhost.evil.example:80", None));
        assert!(!p.origin_allowed("http://localhost:80abc", None));
        assert!(!p.origin_allowed("http://localhost:", None));
    }

    #[test]
    fn post_needs_json_and_custom_header() {
        let p = pol();
        let ok = hs(&[("content-type", "application/json"), ("x-weft-host", "1")]);
        assert!(p.check_post(&ok).is_ok());
        let ct = hs(&[("content-type", "application/json; charset=utf-8"), ("x-weft-host", "1")]);
        assert!(p.check_post(&ct).is_ok());
        for bad in [
            hs(&[("content-type", "text/plain"), ("x-weft-host", "1")]),
            hs(&[("content-type", "application/json")]),
            hs(&[("x-weft-host", "1")]),
            hs(&[("content-type", "application/json"), ("x-weft-host", "0")]),
        ] {
            assert_eq!(p.check_post(&bad).unwrap_err().0, "400 Bad Request");
        }
        let evil = hs(&[("content-type", "application/json"), ("x-weft-host", "1"), ("origin", "http://evil.example"), ("host", "h:1")]);
        assert_eq!(p.check_post(&evil).unwrap_err().0, "403 Forbidden");
    }

    #[test]
    fn token_is_required_and_compared() {
        let p = pol();
        assert!(p.check_token(&hs(&[("authorization", "Bearer sekrit")])).is_ok());
        for bad in [hs(&[]), hs(&[("authorization", "Bearer nope")]), hs(&[("authorization", "sekrit")]), hs(&[("authorization", "Bearer sekri")])] {
            assert_eq!(p.check_token(&bad).unwrap_err().0, "401 Unauthorized");
        }
        assert!(ct_eq("a", "a") && !ct_eq("a", "b") && !ct_eq("a", "ab"));
    }

    #[test]
    fn token_file_is_created_private_and_reused() {
        let d = tempfile::tempdir().unwrap();
        let hex = random_hex(32).unwrap();
        assert_eq!(hex.len(), 64);
        write_private(&token_path(d.path()), hex.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(token_path(d.path())).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(std::fs::read_to_string(token_path(d.path())).unwrap(), hex);
    }
}
