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
//! Scope is [`TokenScope::Owner`] (full surface, ADR-102 D4, an owner
//! decision): a token is owner-equivalent, including lifecycle verbs such
//! as `kernel.shutdown`, with one exception enforced in the daemon: a
//! token cannot issue, revoke or list tokens. TTL is the limit: default
//! 15 minutes, maximum 24 hours.
//!
//! Durability: the chain is saved only on clean shutdown, so a revocation
//! held only on the chain would fail open after a crash. Each revoke is
//! therefore also appended to a small fsynced journal
//! ([`TokenAuthority::with_journal`]) before it is acknowledged. The
//! journal holds revocations ONLY and rebuild accepts `issued` events
//! only from the chain: anyone who can write the journal file could
//! otherwise mint an owner token for a secret of their choosing. A forged
//! revoke is at worst a denial of service. Losing an issue in a crash
//! fails closed (the token does not validate), which is acceptable.

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
    /// A per-project child kernel's credential (ADR-103 A6, Phase 2 G):
    /// read and write only, never Admin, bound to one project by
    /// [`TokenInfo::project`]. It cannot mint, revoke or list tokens and
    /// reaches no Admin-gated method.
    Project,
}

impl TokenScope {
    /// The literal capability scopes this token grants (see
    /// `capability::CallerCapabilities::from_scopes` in the weave crate).
    pub fn capability_scopes(self) -> &'static [&'static str] {
        match self {
            TokenScope::Owner => &["admin"],
            TokenScope::Project => &["write"],
        }
    }

    fn parse(s: Option<&str>) -> Self {
        match s {
            Some("project") => TokenScope::Project,
            _ => TokenScope::Owner,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            TokenScope::Owner => "owner",
            TokenScope::Project => "project",
        }
    }
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
    #[error("could not persist the token record: {0}")]
    Persist(String),
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
    journal: Option<std::path::PathBuf>,
    /// Serialises journal appends (open + write + fsync).
    journal_lock: Mutex<()>,
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

/// WARN when the directory holding the journal is group/world-writable.
fn warn_if_dir_writable(path: &std::path::Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(m) = std::fs::metadata(dir)
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o022 != 0 {
            tracing::warn!(dir = %dir.display(), "token journal directory is group/world-writable");
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// One token event, from the chain or the journal.
struct Record<'a> {
    kind: &'a str,
    payload: &'a Value,
}

/// Parse an `issued` payload; `None` for anything malformed, forged
/// (id not the hash prefix) or longer-lived than [`MAX_TTL`].
fn parse_issued(p: &Value) -> Option<Entry> {
    let id = p.get("id").and_then(Value::as_str)?;
    let hash = p.get("sha256").and_then(Value::as_str).and_then(unhex32)?;
    let issued_at = parse_time(p, "issued_at")?;
    let expires_at = parse_time(p, "expires_at")?;
    let life = expires_at - issued_at;
    if id_of(&hash) != id || life <= Duration::zero() || life > MAX_TTL {
        return None;
    }
    Some(Entry {
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
            scope: TokenScope::parse(p.get("scope").and_then(Value::as_str)),
            project: p.get("project").and_then(Value::as_str).map(str::to_owned),
        },
    })
}

/// Fold records into the live table: issued minus revoked minus expired.
/// The revocation set wins over any `issued` for the same id, whatever
/// the order, so a replayed or forged re-issue cannot revive a token.
fn fold(records: &[Record<'_>], now: DateTime<Utc>) -> HashMap<String, Entry> {
    let mut revoked = std::collections::HashSet::new();
    let mut table = HashMap::new();
    for r in records {
        match r.kind {
            KIND_ISSUED => {
                if let Some(e) = parse_issued(r.payload) {
                    table.insert(e.info.id.clone(), e);
                }
            }
            KIND_REVOKED => {
                if let Some(id) = r.payload.get("id").and_then(Value::as_str) {
                    revoked.insert(id.to_owned());
                }
            }
            _ => {}
        }
    }
    table.retain(|id, e| !revoked.contains(id) && e.info.expires_at > now);
    table
}

impl TokenAuthority {
    /// Build the authority over `chain` (no journal) and replay it.
    pub fn new(chain: Arc<ChainManager>, node_id: impl Into<String>) -> Self {
        Self::with_journal(chain, node_id, None)
    }

    /// Build the authority with a durable revocation journal at `journal`
    /// (see the module docs). `None` means revocations are not crash-durable.
    pub fn with_journal(
        chain: Arc<ChainManager>,
        node_id: impl Into<String>,
        journal: Option<std::path::PathBuf>,
    ) -> Self {
        match &journal {
            None => tracing::warn!(
                "token authority has no journal path: revocations are not durable across a crash"
            ),
            Some(path) => warn_if_dir_writable(path),
        }
        let a = Self {
            chain,
            node_id: node_id.into(),
            journal,
            journal_lock: Mutex::new(()),
            table: Mutex::new(HashMap::new()),
        };
        a.rebuild_at(Utc::now());
        a
    }

    /// Strong references to the underlying chain (the authority holds one).
    pub fn chain_refs(&self) -> usize {
        Arc::strong_count(&self.chain)
    }

    /// Revoked ids recorded in the journal. Lines that cannot be parsed
    /// are logged loudly (a lost revocation is security-relevant); `issued`
    /// lines are ignored by design.
    fn read_journal(&self) -> Vec<Value> {
        let Some(path) = &self.journal else {
            return Vec::new();
        };
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "token journal unreadable: revocations may be lost");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                tracing::warn!(path = %path.display(), line = n + 1, "token journal: unparseable line skipped; a revocation may be lost");
                continue;
            };
            let kind = v.get("kind").and_then(Value::as_str);
            let payload = v.get("payload");
            match (kind, payload) {
                (Some(KIND_REVOKED), Some(p)) if p.get("id").and_then(Value::as_str).is_some() => {
                    out.push(p.clone());
                }
                (Some(KIND_ISSUED), _) => {
                    tracing::warn!(path = %path.display(), line = n + 1, "token journal: ignoring an `issued` entry (the journal is revocations only)");
                }
                _ => {
                    tracing::warn!(path = %path.display(), line = n + 1, "token journal: malformed entry skipped");
                }
            }
        }
        out
    }

    /// Append one revocation to the journal and fsync it. One `write_all`
    /// of a newline-terminated line under a lock; a torn previous tail is
    /// terminated first so it cannot swallow this record.
    fn journal_revoke(&self, payload: &Value) -> Result<(), TokenError> {
        use std::io::{Read, Seek, SeekFrom, Write};
        let Some(path) = &self.journal else {
            return Ok(());
        };
        let mut line =
            json!({ "source": SOURCE, "kind": KIND_REVOKED, "payload": payload }).to_string();
        line.push('\n');
        let _guard = self.journal_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut write = || -> std::io::Result<()> {
            let dir = path.parent();
            if let Some(dir) = dir {
                std::fs::create_dir_all(dir)?;
            }
            let created = !path.exists();
            let mut opts = std::fs::OpenOptions::new();
            opts.create(true).append(true).read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(path)?;
            let len = f.metadata()?.len();
            if len > 0 {
                f.seek(SeekFrom::Start(len - 1))?;
                let mut last = [0u8; 1];
                f.read_exact(&mut last)?;
                if last[0] != b'\n' {
                    line.insert(0, '\n');
                }
            }
            f.write_all(line.as_bytes())?;
            f.sync_all()?;
            if created && let Some(dir) = dir {
                std::fs::File::open(dir)?.sync_all()?;
            }
            Ok(())
        };
        write().map_err(|e| TokenError::Persist(e.to_string()))
    }

    /// Replay the chain and journal: issued minus revoked minus expired at
    /// `now`. Only events from [`SOURCE`] count.
    pub fn rebuild_at(&self, now: DateTime<Utc>) {
        let events = self.chain.tail_from(0);
        let revoked_in_journal = self.read_journal();
        let mut records: Vec<Record<'_>> = Vec::new();
        for ev in &events {
            if ev.source != SOURCE {
                continue;
            }
            if let Some(payload) = ev.payload.as_ref() {
                records.push(Record {
                    kind: ev.kind.as_str(),
                    payload,
                });
            }
        }
        for payload in &revoked_in_journal {
            records.push(Record {
                kind: KIND_REVOKED,
                payload,
            });
        }
        *self.table.lock().unwrap_or_else(|e| e.into_inner()) = fold(&records, now);
        self.prune_journal(&events, &revoked_in_journal, now);
    }

    /// Drop journal revocations whose token can no longer be live
    /// (`issued_at + MAX_TTL <= now`, by the chain's issued event). A
    /// revocation whose issued event is unknown is kept. Rewrites
    /// atomically (temp, fsync, rename, fsync dir, mode 0600), and only
    /// when something is dropped.
    fn prune_journal(
        &self,
        events: &[crate::chain::ChainEvent],
        revoked: &[Value],
        now: DateTime<Utc>,
    ) {
        use std::io::Write;
        let Some(path) = &self.journal else { return };
        let mut issued_at: HashMap<&str, DateTime<Utc>> = HashMap::new();
        for ev in events {
            if ev.source != SOURCE || ev.kind != KIND_ISSUED {
                continue;
            }
            if let Some(p) = ev.payload.as_ref()
                && let (Some(id), Some(t)) = (
                    p.get("id").and_then(Value::as_str),
                    parse_time(p, "issued_at"),
                )
            {
                issued_at.insert(id, t);
            }
        }
        let keep: Vec<&Value> = revoked
            .iter()
            .filter(|p| {
                let id = p.get("id").and_then(Value::as_str).unwrap_or_default();
                issued_at.get(id).is_none_or(|t| *t + MAX_TTL > now)
            })
            .collect();
        if keep.len() == revoked.len() {
            return;
        }
        let _guard = self.journal_lock.lock().unwrap_or_else(|e| e.into_inner());
        let write = || -> std::io::Result<()> {
            let tmp = path.with_extension("jsonl.tmp");
            let mut opts = std::fs::OpenOptions::new();
            opts.create(true).write(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            for p in &keep {
                let mut line =
                    json!({ "source": SOURCE, "kind": KIND_REVOKED, "payload": p }).to_string();
                line.push('\n');
                f.write_all(line.as_bytes())?;
            }
            f.sync_all()?;
            std::fs::rename(&tmp, path)?;
            if let Some(dir) = path.parent() {
                std::fs::File::open(dir)?.sync_all()?;
            }
            Ok(())
        };
        if let Err(e) = write() {
            tracing::warn!(path = %path.display(), error = %e, "token journal prune failed; keeping the old file");
        }
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
        self.issue_scoped_at(now, label, ttl, project, TokenScope::Owner, issuer)
    }

    /// Issue a [`TokenScope::Project`] token for `project_id`: Write only,
    /// never Admin, valid for `ttl`.
    pub fn issue_project(
        &self,
        project_id: &str,
        ttl: Duration,
        issuer: &Issuer,
    ) -> Result<(String, TokenInfo), TokenError> {
        self.issue_scoped_at(
            Utc::now(),
            &format!("project:{project_id}"),
            Some(ttl),
            Some(project_id.to_owned()),
            TokenScope::Project,
            issuer,
        )
    }

    /// Issue with an explicit scope at an explicit clock.
    pub fn issue_scoped_at(
        &self,
        now: DateTime<Utc>,
        label: &str,
        ttl: Option<Duration>,
        project: Option<String>,
        scope: TokenScope,
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
            scope,
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
        if scope != TokenScope::Owner {
            payload["scope"] = json!(scope.as_str());
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

    /// Revoke by id. Returns whether a live token was revoked. The token
    /// stops validating immediately even if journaling then fails (the
    /// error is returned so the caller knows the revocation may not
    /// survive a crash).
    pub fn revoke(&self, id: &str) -> Result<bool, TokenError> {
        if self.lock().remove(id).is_none() {
            return Ok(false);
        }
        let payload = json!({ "id": id, "revoked_at": Utc::now().to_rfc3339() });
        self.chain
            .append(SOURCE, KIND_REVOKED, Some(payload.clone()));
        self.journal_revoke(&payload)?;
        Ok(true)
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
