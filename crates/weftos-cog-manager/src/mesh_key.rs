//! Operator key and per-host session bearers for Weave Manager.
//!
//! `~/.weave-manager/operator.key` (or `$WEAVE_MANAGER_KEY`) is the Ed25519 seed, mode 0600.
//! `~/.weave-manager/sessions.json` (or `$WEAVE_MANAGER_SESSIONS`) maps a cog-host URL to the
//! session that host issued. The address book is a different file and stores neither.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};

/// Same bytes `weftos-cog-host` `mesh_keys::DOMAIN` signs.
pub const DOMAIN: &[u8] = b"weftos.mesh.enroll.v1\n";

#[derive(Clone)]
pub struct Operator {
    signing: SigningKey,
}

impl Operator {
    pub fn pub_hex(&self) -> String {
        hex_encode(self.signing.verifying_key().as_bytes())
    }

    pub fn sign_nonce(&self, nonce: &str) -> String {
        let mut msg = Vec::with_capacity(DOMAIN.len() + nonce.len());
        msg.extend_from_slice(DOMAIN);
        msg.extend_from_slice(nonce.as_bytes());
        hex_encode(&self.signing.sign(&msg).to_bytes())
    }
}

pub fn load_or_create() -> Result<Operator, String> {
    let path = key_path().ok_or_else(|| "no home directory for the mesh key".to_string())?;
    load_or_create_at(&path)
}

fn load_or_create_at(path: &Path) -> Result<Operator, String> {
    if let Ok(mut f) = std::fs::File::open(path) {
        let mut seed = [0u8; 32];
        f.read_exact(&mut seed).map_err(|e| format!("mesh key: {e}"))?;
        return Ok(Operator { signing: SigningKey::from_bytes(&seed) });
    }
    let mut seed = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut seed)).map_err(|e| format!("mesh key: {e}"))?;
    write_private(path, &seed)?;
    Ok(Operator { signing: SigningKey::from_bytes(&seed) })
}

pub fn token_for(host: &str) -> Option<String> {
    let path = session_path()?;
    let map = load_sessions(&path);
    map.get(&canon(host)).cloned()
}

pub fn remember_session(host: &str, token: &str) -> Result<(), String> {
    let path = session_path().ok_or_else(|| "no home directory for mesh sessions".to_string())?;
    let mut map = load_sessions(&path);
    map.insert(canon(host), token.to_string());
    save_sessions(&path, &map)
}

pub fn forget_session(host: &str) {
    let Some(path) = session_path() else { return };
    let mut map = load_sessions(&path);
    map.remove(&canon(host));
    let _ = save_sessions(&path, &map);
}

fn canon(host: &str) -> String {
    host.trim().trim_end_matches('/').to_string()
}

fn home_file(env: &str, name: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env) {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".weave-manager").join(name))
}

fn key_path() -> Option<PathBuf> {
    home_file("WEAVE_MANAGER_KEY", "operator.key")
}

fn session_path() -> Option<PathBuf> {
    home_file("WEAVE_MANAGER_SESSIONS", "sessions.json")
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct SessionsFile {
    sessions: Vec<SessionRow>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SessionRow {
    host: String,
    token: String,
}

fn load_sessions(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else { return BTreeMap::new() };
    let Ok(file) = serde_json::from_str::<SessionsFile>(&text) else { return BTreeMap::new() };
    file.sessions.into_iter().filter(|r| !r.host.is_empty() && !r.token.is_empty()).map(|r| (canon(&r.host), r.token)).collect()
}

fn save_sessions(path: &Path, map: &BTreeMap<String, String>) -> Result<(), String> {
    let file = SessionsFile {
        sessions: map.iter().map(|(host, token)| SessionRow { host: host.clone(), token: token.clone() }).collect(),
    };
    let text = serde_json::to_string(&file).map_err(|e| e.to_string())?;
    write_private(path, text.as_bytes())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signed_domain_matches_the_host() {
        assert_eq!(DOMAIN, b"weftos.mesh.enroll.v1\n");
    }

    #[test]
    fn a_session_file_round_trips_and_is_not_the_address_book() {
        let dir = std::env::temp_dir().join(format!("weave-sess-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.json");
        let mut map = BTreeMap::new();
        map.insert("http://127.0.0.1:9480".into(), "abc123session".into());
        save_sessions(&path, &map).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("abc123session"));
        assert_eq!(load_sessions(&path).get("http://127.0.0.1:9480").map(String::as_str), Some("abc123session"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let book = dir.join("address-book.json");
        std::fs::write(&book, r#"{"entries":[{"label":"this machine","url":"http://127.0.0.1:9480"}]}"#).unwrap();
        assert!(!std::fs::read_to_string(&book).unwrap().contains("abc123session"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_seed_signs_again_after_it_is_stored() {
        let dir = std::env::temp_dir().join(format!("weave-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("operator.key");
        let first = load_or_create_at(&path).unwrap();
        let again = load_or_create_at(&path).unwrap();
        assert_eq!(first.pub_hex(), again.pub_hex());
        assert_eq!(first.sign_nonce("aa"), again.sign_nonce("aa"));
        assert_ne!(first.sign_nonce("aa"), first.sign_nonce("bb"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
