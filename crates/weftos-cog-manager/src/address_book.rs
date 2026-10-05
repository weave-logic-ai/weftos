//! The user's address book for Weave Manager. It lives in `~/.weave-manager/address-book.json`
//! (or `$WEAVE_MANAGER_BOOK`) and stores names and URLs only. Host tokens never go in this file.
//!
//! A tailnet address (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`) is the kind the mesh will attach
//! the console to. A LAN address is a direct connection from this computer. The two stay distinct
//! so a saved house IP is never treated as a tailnet peer.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sensor_install::is_loopback_host;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    ThisMachine,
    Tailnet,
    Lan,
    Other,
}

impl Reach {
    pub fn label(self) -> &'static str {
        match self {
            Reach::ThisMachine => "this machine",
            Reach::Tailnet => "tailnet",
            Reach::Lan => "lan",
            Reach::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Saved,
    Tailnet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookEntry {
    pub label: String,
    pub url: String,
    pub reach: Reach,
    pub online: Option<bool>,
    pub source: Source,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddressBook {
    pub entries: Vec<BookEntry>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    label: String,
    url: String,
}

#[derive(Serialize, Deserialize, Default)]
struct FileShape {
    entries: Vec<Stored>,
}

/// `http://127.0.0.1:9480`, the cog-host Weave Manager tries before it asks.
pub const LOCAL_HOST: &str = "http://127.0.0.1:9480";

pub fn book_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WEAVE_MANAGER_BOOK") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".weave-manager").join("address-book.json"))
}

impl AddressBook {
    pub fn load() -> Self {
        book_path().map(|p| Self::load_from(&p)).unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else { return Self::default() };
        let Ok(file) = serde_json::from_str::<FileShape>(&text) else { return Self::default() };
        let mut book = Self::default();
        for row in file.entries {
            book.upsert(row.label, &row.url);
        }
        book
    }

    pub fn save(&self) -> Result<(), String> {
        let path = book_path().ok_or_else(|| "no home directory for the address book".to_string())?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        let file = FileShape {
            entries: self.entries.iter().filter(|e| e.source == Source::Saved).map(|e| Stored { label: e.label.clone(), url: e.url.clone() }).collect(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// Insert or rename. The reach is computed from the URL, so a file cannot label a LAN
    /// address as tailnet.
    pub fn upsert(&mut self, label: String, url: &str) {
        let Ok(url) = normalize_target(url) else { return };
        let reach = classify(&url);
        if let Some(existing) = self.entries.iter_mut().find(|e| e.url == url) {
            if !label.trim().is_empty() {
                existing.label = label;
            }
            existing.reach = reach;
            existing.source = Source::Saved;
            return;
        }
        let label = if label.trim().is_empty() { url.clone() } else { label };
        self.entries.push(BookEntry { label, url, reach, online: None, source: Source::Saved });
    }
}

/// Saved rows first, then tailnet peers whose URL is not already saved.
pub fn merged(book: &AddressBook, tailnet: &[BookEntry]) -> Vec<BookEntry> {
    let mut out = book.entries.clone();
    for peer in tailnet {
        if out.iter().any(|e| e.url == peer.url) {
            if let Some(row) = out.iter_mut().find(|e| e.url == peer.url) {
                row.online = peer.online;
            }
            continue;
        }
        out.push(peer.clone());
    }
    out
}

pub fn classify(url: &str) -> Reach {
    if is_loopback_host(url) {
        return Reach::ThisMachine;
    }
    let Some(ip) = host_ip(url) else { return Reach::Other };
    if weftos_cog_market::net::tailnet_ip(&ip).is_some() {
        return Reach::Tailnet;
    }
    if is_lan(&ip) {
        return Reach::Lan;
    }
    Reach::Other
}

/// Turn a typed address into a cog-host URL. A bare host gets `http://` and port 9480.
pub fn normalize_target(input: &str) -> Result<String, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("enter an address".into());
    }
    let with_scheme = if t.starts_with("http://") || t.starts_with("https://") { t.to_string() } else { format!("http://{t}") };
    let rest = with_scheme.trim_end_matches('/');
    let (scheme, hostport) = rest.split_once("://").ok_or_else(|| "address needs a host".to_string())?;
    if hostport.is_empty() || hostport.contains('/') {
        return Err("address needs a host".into());
    }
    let has_port = if let Some(h) = hostport.strip_prefix('[') {
        h.split_once(']').and_then(|(_, after)| after.starts_with(':').then_some(true)).unwrap_or(false)
    } else {
        hostport.matches(':').count() == 1
    };
    let url = if has_port { format!("{scheme}://{hostport}") } else { format!("{scheme}://{hostport}:9480") };
    Ok(url.trim_end_matches('/').to_string())
}

fn host_ip(url: &str) -> Option<String> {
    let rest = url.trim().trim_start_matches("http://").trim_start_matches("https://");
    let host = if let Some(h) = rest.strip_prefix('[') {
        h.split_once(']')?.0.to_string()
    } else {
        rest.split([':', '/']).next()?.to_string()
    };
    if host.parse::<IpAddr>().is_ok() { Some(host) } else { None }
}

fn is_lan(ip: &str) -> bool {
    let Ok(addr) = ip.parse::<IpAddr>() else { return false };
    match addr {
        IpAddr::V4(v) => {
            let o = v.octets();
            o[0] == 10 || (o[0] == 192 && o[1] == 168) || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 169 && o[1] == 254)
        }
        IpAddr::V6(_) => false,
    }
}

/// Peers from `tailscale status --json`. Self is not in `Peer`. A non-tailnet IP in that
/// document is dropped. Prefers the IPv4 tailnet address for the cog-host URL.
pub fn peers_from_tailscale_status(body: &str) -> Result<Vec<BookEntry>, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let peers = v.get("Peer").and_then(|p| p.as_object()).ok_or_else(|| "tailscale status has no Peer map".to_string())?;
    let mut out = Vec::new();
    for peer in peers.values() {
        let online = peer.get("Online").and_then(|x| x.as_bool());
        let ip = peer.get("TailscaleIPs").and_then(|x| x.as_array()).and_then(|ips| {
            ips.iter().filter_map(|i| i.as_str()).find(|s| s.contains('.')).or_else(|| ips.iter().find_map(|i| i.as_str()))
        });
        let Some(ip) = ip else { continue };
        if weftos_cog_market::net::tailnet_ip(ip).is_none() {
            continue;
        }
        let name = peer.get("HostName").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).unwrap_or(ip);
        let url = normalize_target(ip).unwrap_or_else(|_| format!("http://{ip}:9480"));
        out.push(BookEntry { label: name.to_string(), url, reach: Reach::Tailnet, online, source: Source::Tailnet });
    }
    out.sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.label.cmp(&b.label)));
    Ok(out)
}

/// Read the local tailnet once. Missing `tailscale` is a note, not a failure of the book.
/// Self is not returned: the connected node is the root, and peers are the nodes under it.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_local_tailnet() -> Result<Vec<BookEntry>, String> {
    let out = std::process::Command::new("tailscale").args(["status", "--json"]).output().map_err(|e| format!("tailscale status: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("tailscale status exited {}: {err}", out.status));
    }
    peers_from_tailscale_status(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(target_arch = "wasm32")]
pub fn read_local_tailnet() -> Result<Vec<BookEntry>, String> {
    Err("tailscale status runs on this computer, not in the browser".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_keeps_tailnet_lan_and_this_machine_apart() {
        assert_eq!(classify("http://127.0.0.1:9480"), Reach::ThisMachine);
        assert_eq!(classify("http://100.109.125.18:9480"), Reach::Tailnet);
        assert_eq!(classify("http://[fd7a:115c:a1e0::1]:9480"), Reach::Tailnet);
        assert_eq!(classify("http://192.168.1.235:9480"), Reach::Lan);
        assert_eq!(classify("http://10.1.2.3:9480"), Reach::Lan);
        assert_eq!(classify("http://cog0.local:9480"), Reach::Other);
    }

    #[test]
    fn bare_tailnet_ip_gets_the_cog_host_port() {
        assert_eq!(normalize_target("100.109.125.18").as_deref(), Ok("http://100.109.125.18:9480"));
        assert_eq!(normalize_target("http://100.64.0.2:9480").as_deref(), Ok("http://100.64.0.2:9480"));
        assert!(normalize_target("  ").is_err());
    }

    #[test]
    fn tailscale_status_drops_non_tailnet_and_prefers_ipv4() {
        let body = r#"{"Peer":{
            "a":{"HostName":"cognitum-cog0","Online":true,"TailscaleIPs":["100.109.125.18","fd7a:115c:a1e0::8"]},
            "b":{"HostName":"weird","Online":true,"TailscaleIPs":["192.168.1.9"]},
            "c":{"HostName":"weave","Online":false,"TailscaleIPs":["100.113.204.47"]}
        }}"#;
        let peers = peers_from_tailscale_status(body).unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].label, "cognitum-cog0");
        assert_eq!(peers[0].url, "http://100.109.125.18:9480");
        assert_eq!(peers[0].online, Some(true));
        assert_eq!(peers[1].label, "weave");
        assert_eq!(peers[1].online, Some(false));
    }

    #[test]
    fn saved_book_recomputes_reach_and_does_not_store_a_token() {
        let dir = std::env::temp_dir().join(format!("weave-book-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("address-book.json");
        let mut book = AddressBook::default();
        book.upsert("cog0".into(), "100.109.125.18");
        book.upsert("bench".into(), "192.168.1.20");
        book.save_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("token"));
        assert!(text.contains("100.109.125.18"));
        let loaded = AddressBook::load_from(&path);
        assert_eq!(loaded.entries[0].reach, Reach::Tailnet);
        assert_eq!(loaded.entries[1].reach, Reach::Lan);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_marks_a_saved_tailnet_row_online_without_duplicating_it() {
        let mut book = AddressBook::default();
        book.upsert("cog0".into(), "100.109.125.18");
        let live = vec![BookEntry {
            label: "cognitum-cog0".into(),
            url: "http://100.109.125.18:9480".into(),
            reach: Reach::Tailnet,
            online: Some(true),
            source: Source::Tailnet,
        }, BookEntry {
            label: "other".into(),
            url: "http://100.64.0.9:9480".into(),
            reach: Reach::Tailnet,
            online: Some(true),
            source: Source::Tailnet,
        }];
        let rows = merged(&book, &live);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "cog0");
        assert_eq!(rows[0].online, Some(true));
        assert_eq!(rows[1].label, "other");
    }
}
