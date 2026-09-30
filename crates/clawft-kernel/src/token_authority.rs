//! Token authority (ADR-102 D3/D4, ADR-103 user role).
//!
//! The daemon is the single issuer and validator of bearer tokens. A
//! token is a 256-bit random secret shown once to the issuer; only its
//! SHA-256 is kept. Every issue and revoke is a chain event, so the table
//! is rebuilt at boot as `issued - revoked - expired`.
//!
//! The chain carries the SHA-256 of the secret (needed to validate after a
//! restart) but never the secret itself. The 16-hex token id is the first
//! 16 characters of that hash, so the hash reveals nothing that helps an
//! attacker: a 256-bit preimage is not searchable.
//!
//! Scope is [`TokenScope::Owner`] (full surface, ADR-102 D4); TTL is the
//! limit: default 15 minutes, maximum 24 hours.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::chain::ChainManager;

/// Chain event source for token events.
pub const SOURCE: &str = "auth.token";
/// Chain event kind for an issue.
pub const KIND_ISSUED: &str = "auth.token.issued";
/// Chain event kind for a revoke.
pub const KIND_REVOKED: &str = "auth.token.revoked";
/// Default token lifetime.
pub const DEFAULT_TTL: Duration = Duration::minutes(15);
/// Longest token lifetime accepted.
pub const MAX_TTL: Duration = Duration::hours(24);
/// Longest accepted label, in characters.
pub const MAX_LABEL_LEN: usize = 64;
/// Prefix on issued secrets, so a leaked one is recognisable.
pub const SECRET_PREFIX: &str = "wft_";

/// What a token grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenScope {
    /// Full surface (ADR-102 D4).
    Owner,
}

/// Public metadata of a live token. Never contains the secret or hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenInfo {
    pub id: String,
    pub label: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub scope: TokenScope,
    /// Optional project claim (ADR-103), a ULID string.
    pub project: Option<String>,
}

/// Why an issue was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("ttl must be positive")]
    TtlNotPositive,
    #[error("ttl exceeds the 24 h maximum")]
    TtlTooLong,
    #[error("label must be 1-{MAX_LABEL_LEN} printable characters")]
    BadLabel,
}

/// Who asked for the token (recorded on the chain, unverified in Phase 1).
#[derive(Debug, Clone, Default)]
pub struct Issuer {
    pub uid: Option<u32>,
}

struct Entry {
    hash: [u8; 32],
    info: TokenInfo,
}

/// The token table, backed by chain events.
pub struct TokenAuthority {
    chain: Arc<ChainManager>,
    node_id: String,
    table: Mutex<HashMap<String, Entry>>,
}

fn hash_secret(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

fn id_of(hash: &[u8; 32]) -> String {
    hex(hash)[..16].to_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Constant-time equality over two hashes.
fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn parse_time(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.get(key)?.as_str()?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

impl TokenAuthority {
    /// Build the authority over `chain` and replay its token events.
    pub fn new(chain: Arc<ChainManager>, node_id: impl Into<String>) -> Self {
        let a = Self {
            chain,
            node_id: node_id.into(),
            table: Mutex::new(HashMap::new()),
        };
        a.rebuild_at(Utc::now());
        a
    }

    /// Replay the chain: issued minus revoked minus expired at `now`.
    pub fn rebuild_at(&self, now: DateTime<Utc>) {
        let mut table: HashMap<String, Entry> = HashMap::new();
        for ev in self.chain.tail_from(0) {
            let Some(p) = ev.payload.as_ref() else {
                continue;
            };
            match ev.kind.as_str() {
                KIND_ISSUED => {
                    let (Some(id), Some(hash), Some(issued_at), Some(expires_at)) = (
                        p.get("id").and_then(Value::as_str),
                        p.get("sha256").and_then(Value::as_str).and_then(unhex32),
                        parse_time(p, "issued_at"),
                        parse_time(p, "expires_at"),
                    ) else {
                        continue;
                    };
                    if id_of(&hash) != id {
                        continue;
                    }
                    table.insert(
                        id.to_owned(),
                        Entry {
                            hash,
                            info: TokenInfo {
                                id: id.to_owned(),
                                label: p
                                    .get("label")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                issued_at,
                                expires_at,
                                scope: TokenScope::Owner,
                                project: p
                                    .get("project")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            },
                        },
                    );
                }
                KIND_REVOKED => {
                    if let Some(id) = p.get("id").and_then(Value::as_str) {
                        table.remove(id);
                    }
                }
                _ => {}
            }
        }
        table.retain(|_, e| e.info.expires_at > now);
        *self.table.lock().unwrap_or_else(|e| e.into_inner()) = table;
    }

    /// Issue a token. Returns the secret (shown once) and its metadata.
    pub fn issue(
        &self,
        label: &str,
        ttl: Option<Duration>,
        project: Option<String>,
        issuer: &Issuer,
    ) -> Result<(String, TokenInfo), TokenError> {
        self.issue_at(Utc::now(), label, ttl, project, issuer)
    }

    /// [`Self::issue`] at an explicit clock.
    pub fn issue_at(
        &self,
        now: DateTime<Utc>,
        label: &str,
        ttl: Option<Duration>,
        project: Option<String>,
        issuer: &Issuer,
    ) -> Result<(String, TokenInfo), TokenError> {
        let ttl = ttl.unwrap_or(DEFAULT_TTL);
        if ttl <= Duration::zero() {
            return Err(TokenError::TtlNotPositive);
        }
        if ttl > MAX_TTL {
            return Err(TokenError::TtlTooLong);
        }
        if label.is_empty()
            || label.chars().count() > MAX_LABEL_LEN
            || label.chars().any(char::is_control)
        {
            return Err(TokenError::BadLabel);
        }
        let mut raw = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut raw);
        let secret = format!("{SECRET_PREFIX}{}", hex(&raw));
        let hash = hash_secret(&secret);
        let info = TokenInfo {
            id: id_of(&hash),
            label: label.to_owned(),
            issued_at: now,
            expires_at: now + ttl,
            scope: TokenScope::Owner,
            project,
        };
        let mut payload = json!({
            "id": info.id,
            "sha256": hex(&hash),
            "label": info.label,
            "issued_at": info.issued_at.to_rfc3339(),
            "expires_at": info.expires_at.to_rfc3339(),
            "issuer": { "node_id": self.node_id, "uid": issuer.uid, "uid_verified": false },
        });
        if let Some(p) = &info.project {
            payload["project"] = json!(p);
        }
        self.chain.append(SOURCE, KIND_ISSUED, Some(payload));
        self.lock().insert(
            info.id.clone(),
            Entry {
                hash,
                info: info.clone(),
            },
        );
        Ok((secret, info))
    }

    /// Revoke by id. Returns whether a live token was revoked.
    pub fn revoke(&self, id: &str) -> bool {
        if !self.lock().contains_key(id) {
            return false;
        }
        self.chain.append(
            SOURCE,
            KIND_REVOKED,
            Some(json!({ "id": id, "revoked_at": Utc::now().to_rfc3339() })),
        );
        self.lock().remove(id).is_some()
    }

    /// Validate a presented secret. Compares hashes in constant time.
    pub fn validate(&self, secret: &str) -> Option<TokenInfo> {
        self.validate_at(Utc::now(), secret)
    }

    /// [`Self::validate`] at an explicit clock.
    pub fn validate_at(&self, now: DateTime<Utc>, secret: &str) -> Option<TokenInfo> {
        let hash = hash_secret(secret);
        let table = self.lock();
        let e = table.get(&id_of(&hash))?;
        (ct_eq(&e.hash, &hash) && e.info.expires_at > now).then(|| e.info.clone())
    }

    /// Live tokens, oldest first.
    pub fn list(&self) -> Vec<TokenInfo> {
        self.list_at(Utc::now())
    }

    /// [`Self::list`] at an explicit clock.
    pub fn list_at(&self, now: DateTime<Utc>) -> Vec<TokenInfo> {
        let mut v: Vec<_> = self
            .lock()
            .values()
            .filter(|e| e.info.expires_at > now)
            .map(|e| e.info.clone())
            .collect();
        v.sort_by(|a, b| a.issued_at.cmp(&b.issued_at).then_with(|| a.id.cmp(&b.id)));
        v
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.table.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests;
