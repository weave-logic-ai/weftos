//! `project.anchor.submit`: the user daemon accepts a project's signed
//! chain-head statement (ADR-103 A7, Phase 2 package D).
//!
//! Authenticated by the project key through the statement signature, not by
//! a capability token: a project kernel holds no token, and the claim the
//! daemon records is exactly "this certified key signed this statement".
//! User daemon only. Checks, in order: the certificate in force for the id
//! (from [`current_view`], never called under a journal lock) is unrevoked,
//! names the signing key, matches `cert_serial` and verifies; the signature;
//! then under one lock `seq` is last + 1, `prev_anchor` is the hash of the
//! last accepted statement, `at` is at most five minutes ahead, and
//! `head_seq` does not go backwards. An identical resubmission of the last
//! accepted statement is answered with its original acknowledgement, so a
//! project that lost the answer can replay safely. `at` has no lower bound:
//! a statement replayed after a long outage is old by design.
//!
//! Key history: statements are verified under the project's CURRENT key.
//! A rejection that carries `last` also carries `key_history` (public keys of
//! the certificate history: current and rekeyed-out keys, never a key revoked
//! for compromise), so a project restarted under a new key can verify and
//! adopt the parent's last statement and continue at N + 1. The daemon's own
//! records (the file, its chain events) were verified when accepted and are
//! re-verified against ALL certificates ever issued for the project,
//! compromised keys included: that check is tamper detection, not
//! authorisation. After a compromise revoke the old statements stay the
//! accepted history up to the revoke; nothing signed by that key is ever
//! accepted as new (the view refuses it).
//!
//! Recovery: at user-daemon startup ([`reconcile_startup`], and lazily on the
//! first use of a project) a `<id>.anchor.json` that is ahead of the chain
//! gets its `project.anchor` event re-appended (`recovered: true`), so the
//! chain is the record again. A chain event at the file's `user_seq` with a
//! different hash is a post-crash sequence collision, not corruption: the
//! event is re-appended under a new seq and the file rewritten (logged).
//!
//! Cost: structural checks (id, hex lengths, canonical `at`) run before the
//! identity view. Verification and the accept happen under one lock, so a
//! revoke or rekey that landed before the lock is taken is seen. The last
//! accepted statement per project is kept in memory (no chain scan per
//! call). After three consecutive AUTHENTICATED refusals (seq, prev, head)
//! a project is answered `anchor_backoff` for 1 s doubling to 60 s; refusals
//! of unsigned garbage never count, so nobody can lock a project out.
//!
//! Acceptance appends a `project.anchor` event (source `project.anchor`,
//! reserved) to the user chain and writes `<manifests>/<id>.anchor.json`
//! (the chain is saved only on clean shutdown, so the file is the durable
//! record; [`last_accepted`] takes the further of the two).
//!
//! Honest limit: the user daemon attests "this key claimed head X at time
//! T"; it cannot verify X without subscribing to the project chain.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use clawft_kernel::chain_anchor::{ANCHOR_SOURCE, AnchorAck};
use clawft_kernel::project_identity::{self as ident, IdentityError, RevocationView};
use clawft_rpc::Response;
use clawft_types::project::canon::hex_decode;
use clawft_types::project::cert::{CertError, ProjectAnchorStmt, ts};
use clawft_types::project::validate_id;
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::project_cert_rpc::{CertEnv, current_view};
use crate::rpc_ext::{ExtCall, ExtFuture, KernelRef};

/// Kind of the user-chain event.
pub use clawft_kernel::chain_anchor::KIND_ANCHOR;
/// How far ahead of the daemon clock `at` may be, seconds.
pub const MAX_AHEAD_SECS: i64 = 300;
/// Authenticated refusals in a row before `anchor_backoff` starts.
const BACKOFF_AFTER: u32 = 3;
const BACKOFF_MAX_SECS: i64 = 60;

type Key = (usize, String);

#[derive(Default)]
struct Inner {
    /// Last accepted statement per (chain, project). `None` = loaded, none yet.
    index: HashMap<Key, Option<Accepted>>,
    /// Consecutive authenticated refusals and the end of the quiet period.
    backoff: HashMap<Key, (u32, Option<DateTime<Utc>>)>,
}

/// Serialises verify-then-append (and owns the in-memory index).
static ACCEPT: Mutex<Option<Inner>> = Mutex::new(None);

#[path = "anchor_error.rs"]
mod error;
pub use error::{Accepted, AnchorError, Resync};

fn anchor_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.anchor.json"))
}

fn key_of(env: &CertEnv, id: &str) -> Key {
    (std::sync::Arc::as_ptr(&env.chain) as *const () as usize, id.to_owned())
}

fn lower_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Cheap checks that need no identity view: run first.
fn structural(stmt: &ProjectAnchorStmt, now: DateTime<Utc>) -> Result<(), AnchorError> {
    let bad = |m: &str| AnchorError::BadStatement(m.to_owned());
    validate_id(&stmt.project_id).map_err(|_| bad("project_id is not a canonical ULID"))?;
    for (name, v, n) in [
        ("project_key_id", stmt.project_key_id.as_str(), 32),
        ("head_hash", &stmt.head_hash, 64),
        ("rule_hash", &stmt.rule_hash, 64),
        ("sig", &stmt.sig, 128),
    ] {
        if !lower_hex(v, n) {
            return Err(AnchorError::BadStatement(format!("`{name}` is not {n} lowercase hex characters")));
        }
    }
    if stmt.prev_anchor.as_deref().is_some_and(|p| !lower_hex(p, 64)) {
        return Err(bad("`prev_anchor` is not 64 lowercase hex characters"));
    }
    let at = DateTime::parse_from_rfc3339(&stmt.at)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .filter(|t| ts(*t) == stmt.at)
        .ok_or_else(|| bad("`at` is not canonical RFC 3339 (YYYY-MM-DDTHH:MM:SSZ)"))?;
    if at > now + Duration::seconds(MAX_AHEAD_SECS) {
        return Err(AnchorError::Future);
    }
    Ok(())
}

fn history_keys(view: &RevocationView, id: &str) -> Vec<String> {
    view.key_history(id).iter().map(|c| c.project_pubkey.clone()).collect()
}

/// A statement the daemon accepted earlier still verifies under some
/// certificate ever issued for the project (tamper detection).
fn recorded_ok(view: &RevocationView, a: &Accepted) -> bool {
    let s = &a.statement;
    view.all_certs(&s.project_id).iter().any(|c| {
        c.project_key_id == s.project_key_id
            && hex_decode::<32>(&c.project_pubkey).is_some_and(|pk| s.verify(&pk).is_ok())
    })
}

fn read_file(env: &CertEnv, id: &str) -> Option<Accepted> {
    std::fs::read(anchor_file(&env.manifests_dir, id))
        .ok()
        .and_then(|b| serde_json::from_slice::<Accepted>(&b).ok())
        .filter(|a| a.statement.project_id == id)
}

fn write_file(env: &CertEnv, a: &Accepted) -> Result<(), AnchorError> {
    let bytes = serde_json::to_vec_pretty(a).map_err(|e| AnchorError::Store(e.to_string()))?;
    ident::write_private_atomic(&anchor_file(&env.manifests_dir, &a.statement.project_id), &bytes, false)
        .map_err(|e: IdentityError| AnchorError::Store(format!("record accepted anchor: {e}")))
}

fn append_event(env: &CertEnv, stmt: &ProjectAnchorStmt, recovered: Option<&Accepted>) -> Accepted {
    let mut payload = json!({
        "project_id": stmt.project_id,
        "statement": stmt,
        "statement_hash": stmt.hash(),
    });
    if let Some(old) = recovered {
        payload["recovered"] = json!(true);
        payload["original_user_seq"] = json!(old.user_seq);
        payload["original_user_event_hash"] = json!(old.user_event_hash);
    }
    let ev = env.chain.append(ANCHOR_SOURCE, KIND_ANCHOR, Some(payload));
    Accepted { statement: stmt.clone(), user_seq: ev.sequence, user_event_hash: ident::hex(&ev.hash) }
}

/// Load the last accepted statement from the file and the chain, and
/// re-append the chain event when the file is ahead (or its event is gone).
fn load_last(env: &CertEnv, id: &str, view: &RevocationView) -> Result<Option<Accepted>, AnchorError> {
    let file = read_file(env, id).filter(|a| {
        let ok = recorded_ok(view, a);
        if !ok {
            warn!(project = id, "anchor record does not verify under the certificate history; ignored");
        }
        ok
    });
    let events = env.chain.tail(0);
    let chain_best = events
        .iter()
        .filter(|e| e.source == ANCHOR_SOURCE && e.kind == KIND_ANCHOR)
        .filter_map(|e| {
            let statement: ProjectAnchorStmt =
                serde_json::from_value(e.payload.as_ref()?.get("statement")?.clone()).ok()?;
            (statement.project_id == id).then(|| Accepted {
                statement,
                user_seq: e.sequence,
                user_event_hash: ident::hex(&e.hash),
            })
        })
        .max_by_key(|a| a.statement.seq);
    let Some(f) = file else { return Ok(chain_best) };
    if chain_best.as_ref().is_some_and(|c| c.statement.seq >= f.statement.seq) {
        return Ok(chain_best);
    }
    // The file is ahead of the chain (the chain was not saved before a crash).
    let colliding = events.iter().any(|e| e.sequence == f.user_seq && ident::hex(&e.hash) != f.user_event_hash);
    if colliding {
        warn!(project = id, user_seq = f.user_seq, "user-chain event at the recorded seq differs; re-appending");
    }
    let fixed = append_event(env, &f.statement, Some(&f));
    write_file(env, &fixed)?;
    info!(project = id, seq = f.statement.seq, user_seq = fixed.user_seq, "recovered project.anchor event from the anchor record");
    Ok(Some(fixed))
}

/// The last statement accepted for `project_id` (index, else file + chain).
pub fn last_accepted(env: &CertEnv, project_id: &str) -> Result<Option<Accepted>, AnchorError> {
    let view = current_view(env)?;
    let mut g = ACCEPT.lock().unwrap_or_else(|p| p.into_inner());
    cached_last(&mut g, env, project_id, &view)
}

fn cached_last(
    g: &mut Option<Inner>,
    env: &CertEnv,
    id: &str,
    view: &RevocationView,
) -> Result<Option<Accepted>, AnchorError> {
    let inner = g.get_or_insert_with(Inner::default);
    let key = key_of(env, id);
    if let Some(v) = inner.index.get(&key) {
        return Ok(v.clone());
    }
    let last = load_last(env, id, view)?;
    inner.index.insert(key, last.clone());
    Ok(last)
}

fn cert_error(e: CertError) -> AnchorError {
    match e {
        CertError::BadSignature => AnchorError::BadSignature,
        other => AnchorError::BadStatement(other.to_string()),
    }
}

/// Verify `stmt` and, when it is the next statement, record it. Pure of the
/// daemon: tests and an in-process transport call this directly.
pub fn submit(
    env: &CertEnv,
    stmt: &ProjectAnchorStmt,
    now: DateTime<Utc>,
) -> Result<Accepted, AnchorError> {
    structural(stmt, now)?;
    let key = key_of(env, &stmt.project_id);
    let mut guard = ACCEPT.lock().unwrap_or_else(|p| p.into_inner());
    // Everything below sees revocations and rekeys that landed before this point.
    let view = current_view(env)?;
    let revoked = || AnchorError::KeyRevoked {
        project_id: stmt.project_id.clone(),
        key_id: stmt.project_key_id.clone(),
    };
    if view.is_revoked(&stmt.project_id, &stmt.project_key_id) {
        return Err(revoked());
    }
    let cert = view
        .current_cert(&stmt.project_id)
        .ok_or_else(|| AnchorError::NotCertified(stmt.project_id.clone()))?;
    if cert.project_key_id != stmt.project_key_id {
        return Err(revoked());
    }
    cert.verify(&env.user_key.verifying_key().to_bytes(), now)
        .map_err(|e| AnchorError::CertInvalid(e.to_string()))?;
    if stmt.cert_serial != cert.serial {
        return Err(AnchorError::CertInvalid(format!(
            "cert_serial {} is not the certificate in force ({})",
            stmt.cert_serial, cert.serial
        )));
    }
    let project_pk: [u8; 32] = hex_decode(&cert.project_pubkey)
        .ok_or_else(|| AnchorError::CertInvalid("certificate public key is malformed".into()))?;
    stmt.verify(&project_pk).map_err(cert_error)?;

    // Authenticated from here on: refusals count towards the backoff.
    let inner = guard.get_or_insert_with(Inner::default);
    if let Some((_, Some(until))) = inner.backoff.get(&key)
        && now < *until
    {
        return Err(AnchorError::Backoff((*until - now).num_seconds().max(1)));
    }
    let last = cached_last(&mut guard, env, &stmt.project_id, &view)?;
    let inner = guard.as_mut().expect("inner exists");
    let outcome = accept(env, &view, stmt, last);
    match &outcome {
        Ok(a) => {
            inner.index.insert(key.clone(), Some(a.clone()));
            inner.backoff.remove(&key);
        }
        Err(AnchorError::Store(_)) => {}
        Err(_) => {
            let e = inner.backoff.entry(key).or_insert((0, None));
            e.0 += 1;
            if e.0 >= BACKOFF_AFTER {
                let secs = (1i64 << (e.0 - BACKOFF_AFTER).min(6)).min(BACKOFF_MAX_SECS);
                e.1 = Some(now + Duration::seconds(secs));
            }
        }
    }
    outcome
}

fn accept(
    env: &CertEnv,
    view: &RevocationView,
    stmt: &ProjectAnchorStmt,
    last: Option<Accepted>,
) -> Result<Accepted, AnchorError> {
    if let Some(l) = &last
        && l.statement == *stmt
    {
        return Ok(l.clone());
    }
    let resync = |last: Option<Accepted>| {
        last.map(|l| Box::new(Resync { last: l, key_history: history_keys(view, &stmt.project_id) }))
    };
    let want = last.as_ref().map_or(1, |l| l.statement.seq + 1);
    if stmt.seq != want {
        return Err(AnchorError::Seq { got: stmt.seq, want, last: resync(last) });
    }
    if stmt.prev_anchor != last.as_ref().map(|l| l.statement.hash()) {
        return Err(AnchorError::Prev { last: resync(last) });
    }
    if let Some(l) = &last
        && stmt.head_seq < l.statement.head_seq
    {
        return Err(AnchorError::HeadRegress { got: stmt.head_seq, last: l.statement.head_seq });
    }
    let accepted = append_event(env, stmt, None);
    write_file(env, &accepted)?;
    Ok(accepted)
}

/// User-daemon startup: bring every `<id>.anchor.json` and the user chain
/// back into agreement (see the module docs). Returns the projects whose
/// chain event was re-appended.
pub fn reconcile(env: &CertEnv) -> Result<Vec<String>, AnchorError> {
    let view = current_view(env)?;
    let mut fixed = Vec::new();
    let Ok(rd) = std::fs::read_dir(&env.manifests_dir) else { return Ok(fixed) };
    let mut g = ACCEPT.lock().unwrap_or_else(|p| p.into_inner());
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".anchor.json") else { continue };
        if validate_id(id).is_err() {
            continue;
        }
        let before = env.chain.sequence();
        cached_last(&mut g, env, id, &view)?;
        if env.chain.sequence() != before {
            fixed.push(id.to_owned());
        }
    }
    Ok(fixed)
}

/// Run [`reconcile`] for the user daemon's own chain (call once after boot).
pub async fn reconcile_startup(kernel: &KernelRef) {
    let env = match crate::project_cert_rpc::env_from_kernel(kernel).await {
        Ok(e) => e,
        Err(e) => return warn!(error = %e, "anchor reconcile skipped"),
    };
    match tokio::task::spawn_blocking(move || reconcile(&env)).await {
        Ok(Ok(f)) if !f.is_empty() => info!(projects = ?f, "re-appended project.anchor events from anchor records"),
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn!(error = %e, "anchor reconcile failed"),
        Err(e) => warn!(error = %e, "anchor reconcile task failed"),
    }
}

/// Handler for `project.anchor.submit`. Params are the statement itself.
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        let env = match crate::project_cert_rpc::env_from(&call.ctx).await {
            Ok(e) => e,
            Err(e) => return AnchorError::Unavailable(e.to_string()).response(),
        };
        let stmt: ProjectAnchorStmt = match serde_json::from_value(call.params) {
            Ok(s) => s,
            Err(e) => return AnchorError::BadStatement(format!("not an anchor statement: {e}")).response(),
        };
        match tokio::task::spawn_blocking(move || submit(&env, &stmt, Utc::now())).await {
            Ok(Ok(a)) => Response::success(json!({
                "user_seq": a.user_seq,
                "user_event_hash": a.user_event_hash,
            })),
            Ok(Err(e)) => e.response(),
            Err(e) => Response::error(format!("anchor task failed: {e}")),
        }
    })
}

/// Parse the success result of `project.anchor.submit` (a transport helper).
pub fn parse_ack(result: &Value) -> Option<AnchorAck> {
    Some(AnchorAck {
        user_seq: result.get("user_seq")?.as_u64()?,
        user_event_hash: result.get("user_event_hash")?.as_str()?.to_owned(),
    })
}

#[cfg(test)]
#[path = "anchor_rpc_tests.rs"]
mod tests;
