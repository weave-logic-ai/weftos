//! Session keys for a console that connected over the tailnet, or from this machine.
//!
//! `GET /mesh/enroll` issues one nonce bound to the socket address. `POST /mesh/enroll`
//! accepts `{pub, nonce, sig}` when that signature proves the caller holds the Ed25519
//! key. The host stores a session bearer in `<root>/mesh-keys.json` (mode 0600). That
//! bearer is not the contents of `host.token`. A LAN address is refused. Knowing the
//! public key is not enough to read the session back.
//!
//! The signed bytes are `weftos.mesh.enroll.v1\n` plus the nonce hex. Weave Manager
//! signs the same bytes.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::auth::{self, ct_eq};

/// Domain string both sides sign. Weave Manager duplicates these exact bytes.
pub const DOMAIN: &[u8] = b"weftos.mesh.enroll.v1\n";

const CHALLENGE_TTL: Duration = Duration::from_secs(60);
const MAX_CHALLENGES: usize = 64;

pub fn path(root: &Path) -> PathBuf {
    root.join("mesh-keys.json")
}

pub fn enroll_message(nonce: &str) -> Vec<u8> {
    let mut msg = Vec::with_capacity(DOMAIN.len() + nonce.len());
    msg.extend_from_slice(DOMAIN);
    msg.extend_from_slice(nonce.as_bytes());
    msg
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i < b.len() {
        out.push((hex_nibble(b[i])? << 4) | hex_nibble(b[i + 1])?);
        i += 2;
    }
    Some(out)
}

/// Loopback or a tailnet address. Anything else, including a missing peer, is refused.
pub fn may_enroll(peer: Option<&str>) -> bool {
    let Some(ip) = canonical_peer(peer) else { return false };
    ip.is_loopback() || weftos_cog_market::net::is_tailnet_ip(&ip)
}

fn canonical_peer(peer: Option<&str>) -> Option<IpAddr> {
    peer?.parse::<IpAddr>().ok().map(|ip| ip.to_canonical())
}

fn peer_key(peer: Option<&str>) -> Option<String> {
    canonical_peer(peer).map(|ip| ip.to_string())
}

#[derive(Clone)]
struct Key {
    pub_hex: String,
    token: String,
    seen: u64,
    peer: String,
}

struct Challenge {
    nonce: String,
    peer: String,
    exp: Instant,
}

struct Inner {
    keys: Vec<Key>,
    challenges: Vec<Challenge>,
}

pub struct MeshKeys {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for MeshKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.inner.lock().map(|g| g.keys.len()).unwrap_or(0);
        f.debug_struct("MeshKeys").field("keys", &n).finish()
    }
}

impl MeshKeys {
    pub fn open(root: &Path) -> Self {
        let path = path(root);
        let stored = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<FileShape>(&b).ok()).map(|f| f.keys).unwrap_or_default();
        let keys = stored
            .into_iter()
            .filter(|k| k.token.len() == 64 && k.pub_hex.len() == 64 && hex_decode(&k.token).is_some() && hex_decode(&k.pub_hex).is_some())
            .map(|k| Key { pub_hex: k.pub_hex, token: k.token, seen: k.seen, peer: k.peer })
            .collect();
        Self { path, inner: Mutex::new(Inner { keys, challenges: Vec::new() }) }
    }

    /// A fresh nonce for this socket. One live challenge per peer; a new one replaces the last.
    pub fn issue(&self, peer: Option<&str>) -> Result<String, String> {
        let Some(peer) = peer_key(peer) else {
            return Err("the mesh registers a key for a tailnet or local connection".into());
        };
        if !may_enroll(Some(&peer)) {
            return Err("the mesh registers a key for a tailnet or local connection".into());
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        g.challenges.retain(|c| c.exp > now && c.peer != peer);
        if g.challenges.len() >= MAX_CHALLENGES {
            return Err("too many enrollments in flight".into());
        }
        let nonce = auth::random_hex(32).map_err(|e| e.to_string())?;
        g.challenges.push(Challenge { nonce: nonce.clone(), peer, exp: now + CHALLENGE_TTL });
        Ok(nonce)
    }

    /// Prove possession, then mint or rotate the session for this public key.
    pub fn enroll(&self, peer: Option<&str>, pub_hex: &str, nonce: &str, sig_hex: &str) -> Result<String, String> {
        let Some(peer) = peer_key(peer) else {
            return Err("the mesh registers a key for a tailnet or local connection".into());
        };
        if !may_enroll(Some(&peer)) {
            return Err("the mesh registers a key for a tailnet or local connection".into());
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let mut found = None;
        for (i, c) in g.challenges.iter().enumerate() {
            if ct_eq(&c.nonce, nonce) && found.is_none() {
                found = Some(i);
            }
        }
        let Some(idx) = found else { return Err("unknown or used challenge".into()) };
        let ch = g.challenges.remove(idx);
        if ch.exp <= now {
            return Err("challenge expired".into());
        }
        if ch.peer != peer {
            return Err("challenge was issued to a different address".into());
        }
        let pub_bytes = hex_decode(pub_hex).filter(|b| b.len() == 32).ok_or("bad public key")?;
        let sig_bytes = hex_decode(sig_hex).filter(|b| b.len() == 64).ok_or("bad signature")?;
        let mut pub_arr = [0u8; 32];
        pub_arr.copy_from_slice(&pub_bytes);
        let vk = VerifyingKey::from_bytes(&pub_arr).map_err(|_| "bad public key")?;
        let sig = Signature::from_slice(&sig_bytes).map_err(|_| "bad signature")?;
        vk.verify(&enroll_message(nonce), &sig).map_err(|_| "signature does not prove this key")?;
        let token = auth::random_hex(32).map_err(|e| e.to_string())?;
        let seen = unix_now();
        let mut keys = g.keys.clone();
        if let Some(row) = keys.iter_mut().find(|k| ct_eq(&k.pub_hex, pub_hex)) {
            row.token = token.clone();
            row.seen = seen;
            row.peer = peer;
        } else {
            keys.push(Key { pub_hex: pub_hex.to_string(), token: token.clone(), seen, peer });
        }
        persist(&self.path, &keys)?;
        g.keys = keys;
        Ok(token)
    }

    pub fn accepts(&self, token: &str) -> bool {
        if token.is_empty() {
            return false;
        }
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut ok = false;
        for k in &g.keys {
            ok |= ct_eq(&k.token, token);
        }
        ok
    }

    /// Refresh `seen` for a session bearer. The static host token is not a session.
    pub fn maintain(&self, token: &str) -> bool {
        if token.is_empty() {
            return false;
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut hit = false;
        let seen = unix_now();
        let mut keys = g.keys.clone();
        for k in &mut keys {
            if ct_eq(&k.token, token) {
                k.seen = seen;
                hit = true;
            }
        }
        if !hit {
            return false;
        }
        if persist(&self.path, &keys).is_err() {
            return false;
        }
        g.keys = keys;
        true
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredKey {
    pub_hex: String,
    token: String,
    seen: u64,
    peer: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FileShape {
    keys: Vec<StoredKey>,
}

fn persist(path: &Path, keys: &[Key]) -> Result<(), String> {
    let file = FileShape {
        keys: keys.iter().map(|k| StoredKey { pub_hex: k.pub_hex.clone(), token: k.token.clone(), seen: k.seen, peer: k.peer.clone() }).collect(),
    };
    let text = serde_json::to_string(&file).map_err(|e| e.to_string())?;
    auth::write_private(path, text.as_bytes()).map_err(|e| format!("could not store the mesh key: {e}"))
}

fn refuse(code: &'static str, error: &str) -> (&'static str, String) {
    (code, serde_json::json!({"ok": false, "error": error}).to_string())
}

pub fn http_challenge(keys: &MeshKeys, peer: Option<&str>) -> (&'static str, String) {
    match keys.issue(peer) {
        Ok(nonce) => ("200 OK", serde_json::json!({"ok": true, "nonce": nonce}).to_string()),
        Err(e) => refuse("403 Forbidden", &e),
    }
}

pub fn http_enroll(keys: &MeshKeys, peer: Option<&str>, body: &[u8]) -> (&'static str, String) {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return refuse("400 Bad Request", "bad enroll body"),
    };
    let (Some(pub_hex), Some(nonce), Some(sig)) = (v["pub"].as_str(), v["nonce"].as_str(), v["sig"].as_str()) else {
        return refuse("400 Bad Request", "enroll needs pub, nonce and sig");
    };
    match keys.enroll(peer, pub_hex, nonce, sig) {
        Ok(token) => ("200 OK", serde_json::json!({"ok": true, "token": token}).to_string()),
        Err(e) if e.contains("tailnet or local") => refuse("403 Forbidden", &e),
        Err(e) => refuse("400 Bad Request", &e),
    }
}

pub fn http_maintain(keys: &MeshKeys, token: &str) -> (&'static str, String) {
    let held = keys.maintain(token);
    ("200 OK", serde_json::json!({"ok": true, "session": held}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn sign(sk: &SigningKey, nonce: &str) -> (String, String) {
        let pub_hex = hex_encode(sk.verifying_key().as_bytes());
        let sig = sk.sign(&enroll_message(nonce));
        (pub_hex, hex_encode(&sig.to_bytes()))
    }

    #[test]
    fn loopback_enrolls_and_a_lan_address_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let keys = MeshKeys::open(dir.path());
        assert!(may_enroll(Some("127.0.0.1")));
        assert!(may_enroll(Some("::1")));
        assert!(may_enroll(Some("100.109.125.18")));
        assert!(!may_enroll(Some("192.168.1.235")));
        assert!(!may_enroll(Some("10.1.2.3")));
        assert!(!may_enroll(None));
        assert!(keys.issue(Some("192.168.1.20")).is_err());
        let nonce = keys.issue(Some("127.0.0.1")).unwrap();
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let (pub_hex, sig) = sign(&sk, &nonce);
        let token = keys.enroll(Some("127.0.0.1"), &pub_hex, &nonce, &sig).unwrap();
        assert!(keys.accepts(&token));
        assert!(!keys.accepts("sekrit-host-token"));
        let stored = std::fs::read_to_string(path(dir.path())).unwrap();
        assert!(!stored.contains("sekrit-host-token"));
        assert!(stored.contains(&token));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path(dir.path())).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn a_known_public_key_without_the_signature_does_not_return_the_session() {
        let keys = MeshKeys::open(tempfile::tempdir().unwrap().path());
        let sk = SigningKey::from_bytes(&[4u8; 32]);
        let nonce = keys.issue(Some("100.64.0.8")).unwrap();
        let (pub_hex, sig) = sign(&sk, &nonce);
        let token = keys.enroll(Some("100.64.0.8"), &pub_hex, &nonce, &sig).unwrap();
        let again = keys.issue(Some("100.64.0.8")).unwrap();
        let err = keys.enroll(Some("100.64.0.8"), &pub_hex, &again, &"00".repeat(64)).unwrap_err();
        assert!(err.contains("signature") || err.contains("bad signature"));
        assert!(keys.accepts(&token), "a failed enroll leaves the session in place");
        assert!(keys.enroll(Some("100.64.0.8"), &pub_hex, &nonce, &sig).is_err(), "a nonce is single use");
    }

    #[test]
    fn a_challenge_from_one_address_fails_on_another_and_rotate_retires_the_old_token() {
        let keys = MeshKeys::open(tempfile::tempdir().unwrap().path());
        let sk = SigningKey::from_bytes(&[5u8; 32]);
        let nonce = keys.issue(Some("127.0.0.1")).unwrap();
        let (pub_hex, sig) = sign(&sk, &nonce);
        assert!(keys.enroll(Some("100.87.159.20"), &pub_hex, &nonce, &sig).is_err());
        assert!(keys.enroll(Some("127.0.0.1"), &pub_hex, &nonce, &sig).is_err(), "the mismatched attempt burns the nonce");

        let nonce = keys.issue(Some("127.0.0.1")).unwrap();
        let (pub_hex, sig) = sign(&sk, &nonce);
        let first = keys.enroll(Some("127.0.0.1"), &pub_hex, &nonce, &sig).unwrap();
        assert!(keys.maintain(&first));
        let nonce = keys.issue(Some("127.0.0.1")).unwrap();
        let (pub_hex, sig) = sign(&sk, &nonce);
        let second = keys.enroll(Some("127.0.0.1"), &pub_hex, &nonce, &sig).unwrap();
        assert_ne!(first, second);
        assert!(!keys.accepts(&first));
        assert!(keys.accepts(&second));
        assert!(!keys.maintain(&first));
    }

    #[test]
    fn the_signed_domain_is_the_bytes_the_console_signs() {
        assert_eq!(DOMAIN, b"weftos.mesh.enroll.v1\n");
    }
}
