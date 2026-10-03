//! Host pairing store: base URL + capability token + optional host node id.
//!
//! Pairing QR / manual form (ADR-077):
//! `weftos://pair?host=…&port=…&token=…&node=…&v=1`
//! or plain `https://host:7860` + bearer/token.

use crate::error::EdgeError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const PAIR_FILE: &str = "host_pair.json";

/// Trusted host + capability-scoped token for splatd / gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct HostPair {
    /// Base URL without trailing slash, e.g. `http://192.168.1.20:7860`.
    pub base_url: String,
    /// Capability token (Bearer or query). Empty string means unauthenticated LAN.
    pub token: String,
    /// Optional host Ed25519 node id (hex) from QR.
    pub host_node_id: Option<String>,
    /// Pairing scheme version from QR (`v=1`).
    pub pair_version: u32,
    /// Wall-clock ms when the pair was saved (device clock).
    pub paired_at_ms: u64,
}

impl HostPair {
    pub fn jobs_url(&self) -> String {
        format!("{}/v1/jobs", self.base_url.trim_end_matches('/'))
    }

    pub fn job_url(&self, job_id: &str) -> String {
        format!("{}/v1/jobs/{job_id}", self.base_url.trim_end_matches('/'))
    }

    pub fn healthz_url(&self) -> String {
        format!("{}/healthz", self.base_url.trim_end_matches('/'))
    }
}

/// Persist a host pair under `<data_dir>/host_pair.json`.
pub fn save_pair(data_dir: &Path, pair: &HostPair) -> Result<(), EdgeError> {
    validate_pair(pair)?;
    fs::create_dir_all(data_dir).map_err(EdgeError::io)?;
    let path = pair_path(data_dir);
    let json = serde_json::to_string_pretty(pair)
        .map_err(|e| EdgeError::pair(format!("serialize: {e}")))?;
    fs::write(&path, json).map_err(EdgeError::io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Load the current host pair, if any.
pub fn load_pair(data_dir: &Path) -> Result<Option<HostPair>, EdgeError> {
    let path = pair_path(data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(EdgeError::io)?;
    let pair: HostPair =
        serde_json::from_str(&raw).map_err(|e| EdgeError::pair(format!("parse: {e}")))?;
    validate_pair(&pair)?;
    Ok(Some(pair))
}

/// Clear stored pair (unpair).
pub fn clear_pair(data_dir: &Path) -> Result<(), EdgeError> {
    let path = pair_path(data_dir);
    if path.exists() {
        fs::remove_file(&path).map_err(EdgeError::io)?;
    }
    Ok(())
}

/// Parse a pairing deep link or plain URL into a [`HostPair`].
///
/// Accepted forms:
/// - `weftos://pair?host=192.168.1.2&port=7860&token=abc&node=<hex>&v=1`
/// - `http(s)://host:port` (token optional via `?token=` or separate arg)
/// - bare `host:port` → `http://host:port`
pub fn parse_pair_input(input: &str, token_override: Option<&str>) -> Result<HostPair, EdgeError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(EdgeError::invalid("empty pair input"));
    }

    let now_ms = now_millis();

    if input.starts_with("weftos://pair") || input.starts_with("weftos:pair") {
        return parse_weftos_qr(input, token_override, now_ms);
    }

    let mut base = if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        format!("http://{input}")
    };

    // Strip query for base_url but harvest token.
    let mut token = token_override.unwrap_or("").to_string();
    if let Some((head, query)) = base.split_once('?') {
        for part in query.split('&') {
            if let Some((k, v)) = part.split_once('=')
                && k == "token" && token.is_empty() {
                    token = urlencoding_decode(v);
                }
        }
        base = head.to_string();
    }
    base = base.trim_end_matches('/').to_string();

    let pair = HostPair {
        base_url: base,
        token,
        host_node_id: None,
        pair_version: 1,
        paired_at_ms: now_ms,
    };
    validate_pair(&pair)?;
    Ok(pair)
}

fn parse_weftos_qr(
    input: &str,
    token_override: Option<&str>,
    now_ms: u64,
) -> Result<HostPair, EdgeError> {
    let query = input
        .split_once('?')
        .map(|(_, q)| q)
        .ok_or_else(|| EdgeError::pair("weftos pair URL missing query"))?;

    let mut host = None::<String>;
    let mut port = None::<String>;
    let mut token = token_override.unwrap_or("").to_string();
    let mut node = None::<String>;
    let mut version = 1u32;
    let mut scheme = "http".to_string();

    for part in query.split('&') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let v = urlencoding_decode(v);
        match k {
            "host" => host = Some(v),
            "port" => port = Some(v),
            "token" if token.is_empty() => token = v,
            "node" => node = Some(v),
            "v" => {
                version = v.parse().unwrap_or(1);
            }
            "scheme" | "proto" => scheme = v,
            "url" | "base" => {
                // Full base URL override
                let base = v.trim_end_matches('/').to_string();
                return Ok(HostPair {
                    base_url: base,
                    token,
                    host_node_id: node,
                    pair_version: version,
                    paired_at_ms: now_ms,
                });
            }
            _ => {}
        }
    }

    let host = host.ok_or_else(|| EdgeError::pair("pair QR missing host"))?;
    let port = port.unwrap_or_else(|| "7860".into());
    let base_url = format!("{scheme}://{host}:{port}");
    let pair = HostPair {
        base_url,
        token,
        host_node_id: node,
        pair_version: version,
        paired_at_ms: now_ms,
    };
    validate_pair(&pair)?;
    Ok(pair)
}

fn validate_pair(pair: &HostPair) -> Result<(), EdgeError> {
    if pair.base_url.is_empty() {
        return Err(EdgeError::pair("base_url empty"));
    }
    if !(pair.base_url.starts_with("http://") || pair.base_url.starts_with("https://")) {
        return Err(EdgeError::pair("base_url must be http(s)"));
    }
    Ok(())
}

pub fn pair_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PAIR_FILE)
}

fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Minimal percent-decode (token/host only; not a full URL parser).
fn urlencoding_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h = hex_nibble(bytes[i + 1]);
                let l = hex_nibble(bytes[i + 2]);
                if let (Some(h), Some(l)) = (h, l) {
                    out.push((h << 4) | l);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_plain_url_and_persist() {
        let pair = parse_pair_input("http://10.0.0.5:7860", Some("tok")).unwrap();
        assert_eq!(pair.base_url, "http://10.0.0.5:7860");
        assert_eq!(pair.token, "tok");
        let dir = tempdir().unwrap();
        save_pair(dir.path(), &pair).unwrap();
        let loaded = load_pair(dir.path()).unwrap().unwrap();
        assert_eq!(loaded, pair);
        clear_pair(dir.path()).unwrap();
        assert!(load_pair(dir.path()).unwrap().is_none());
    }

    #[test]
    fn parse_weftos_qr() {
        let pair = parse_pair_input(
            "weftos://pair?host=192.168.1.9&port=7860&token=abc%201&node=deadbeef&v=1",
            None,
        )
        .unwrap();
        assert_eq!(pair.base_url, "http://192.168.1.9:7860");
        assert_eq!(pair.token, "abc 1");
        assert_eq!(pair.host_node_id.as_deref(), Some("deadbeef"));
        assert_eq!(pair.pair_version, 1);
    }
}
