//! Project key certificates: `project.cert.show`, `project.cert.challenge`,
//! `project.rekey`, `project.revoke`, and [`issue_for_register`] for `mesh.register`
//! (ADR-103 A7, Phase 2 package C).
//!
//! Every route is `Admin` and user-daemon only. A `wft_` token is refused
//! (as in `token_rpc`): the TCP relay strips literal scopes, so a relayed
//! caller can only present a token, and a token must not certify keys.
//! A daemon that is not the `--profile user` daemon refuses too, so a
//! project kernel can never mint certificates with its own key.
//!
//! The user chain (source `user.projects`, reserved: the `chain.append`
//! RPC refuses it) holds `project.register`, `project.rekey` and
//! `project.revoke` events: certificates (public keys and signatures) and
//! metadata, never private bytes. The chain is saved only on clean
//! shutdown, so every operation first appends to the fsynced identity
//! journal (`<manifests>/identity.journal.jsonl`) under an exclusive
//! `flock`, and [`current_view`] merges journal + `<id>.cert.json` files +
//! chain into the one [`RevocationView`] used for every decision. A journal
//! that exists but does not parse stops issuing and verifying (fail closed).
//!
//! Nonces are issued by the daemon ([`issue_challenge`]), single use and
//! short lived; [`claim_nonce`] turns one into a [`DaemonNonce`], the only
//! thing [`issue_for_register`] and `project.rekey` accept, so a replayed or
//! invented nonce cannot reach the PoP check. Package H's `mesh.challenge`
//! must call [`issue_challenge`] and its `mesh.register` [`claim_nonce`]; H
//! also verifies the spawn nonce. TOFU limits are stated in
//! `clawft_kernel::project_identity`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use clawft_kernel::chain::ChainManager;
use clawft_kernel::project_identity::{
    self as ident, IdentityError, IdentityJournal, JournalRecord, KIND_REGISTER, KIND_REKEY,
    KIND_REVOKE, Registration, SOURCE,
};
use clawft_kernel::token_authority::SECRET_PREFIX;
use clawft_rpc::Response;
use clawft_types::project::cert::{PopOp, ProjectCert, key_id};
use clawft_types::project::{CertRequest, ProjectError, find_by_id, validate_id};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::rpc_ext::{ExtCall, ExtCtx, ExtFuture};

#[path = "project_cert_store.rs"]
mod store;
use store::current_view_locked;
#[cfg(test)]
use store::read_cert_files as store_read_certs;
pub use store::{current_view, repair};

#[path = "project_cert_nonce.rs"]
mod nonce;
pub use nonce::{CHALLENGE_TTL, DaemonNonce, claim_nonce, issue_challenge};

const MAX_REASON: usize = 256;

/// What the user daemon verified about the child it spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnInfo {
    /// Child pid.
    pub pid: u32,
    /// SHA-256 of the child executable, hex.
    pub exe_sha: String,
}

/// A request to certify a project key (from `mesh.register`).
#[derive(Debug)]
pub struct RegisterRequest {
    /// Project ULID.
    pub project_id: String,
    /// The project's public key.
    pub project_pubkey: [u8; 32],
    /// [`root_sha256`] of the project root the child claims.
    pub root_sha256: String,
    /// Spawn facts recorded on the chain.
    pub spawn: SpawnInfo,
    /// The daemon-issued challenge nonce, claimed with [`claim_nonce`].
    pub nonce: DaemonNonce,
    /// Proof of possession: [`ident::pop_sign`] with [`PopOp::Register`]
    /// and the user key id over this nonce and project id.
    pub pop_sig: [u8; 64],
}

/// A certificate and whether this call created it.
#[derive(Debug, Clone)]
pub struct Issued {
    /// The certificate in force.
    pub cert: ProjectCert,
    /// False when the key was already certified (idempotent re-register).
    pub new: bool,
}

/// Why an operation was refused; `kind()` is the RPC `error_kind`.
#[derive(Debug, thiserror::Error)]
pub enum IssueError {
    /// Not the user daemon, or it has no chain/key.
    #[error("{0}")]
    Unavailable(String),
    /// Malformed parameter.
    #[error("{0}")]
    Invalid(String),
    /// The id is not in the manifest store.
    #[error("project {0} is not registered")]
    NotRegistered(String),
    /// The claimed root is not the registered root. The child must send the
    /// canonical root path (what `project.register` stored).
    #[error("root does not match the registered project root")]
    RootMismatch,
    /// Proof of possession failed or its nonce was used before.
    #[error("{0}")]
    Pop(String),
    /// Identity rule (TOFU conflict, revoked, not bound, bad cert).
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// Manifest store failure.
    #[error("{0}")]
    Store(String),
}

impl IssueError {
    /// Stable snake_case discriminator for clients.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "cert_unavailable",
            Self::Invalid(_) => "invalid_params",
            Self::NotRegistered(_) => "project_not_found",
            Self::RootMismatch => "root_mismatch",
            Self::Pop(_) => "pop_failed",
            Self::Identity(IdentityError::KeyConflict { .. }) => "key_conflict",
            Self::Identity(IdentityError::KeyRevoked { .. }) => "key_revoked",
            Self::Identity(IdentityError::KeyReuse { .. }) => "key_reuse",
            Self::Identity(IdentityError::JournalCorrupt { .. }) => "journal_corrupt",
            Self::Identity(IdentityError::NotBound(_)) => "not_certified",
            Self::Identity(_) => "cert_error",
            Self::Store(_) => "project_store_error",
        }
    }

    fn response(&self) -> Response {
        Response::error_with_kind(self.kind(), self.to_string())
    }
}

/// Everything the operations need, injectable for tests.
pub struct CertEnv {
    /// The user chain.
    pub chain: Arc<ChainManager>,
    /// The user key (the user chain's signing key).
    pub user_key: SigningKey,
    /// `~/.weftos/projects`.
    pub manifests_dir: PathBuf,
}

/// Hex SHA-256 of the root path's raw bytes (`OsStr` bytes on unix, so two
/// distinct non-UTF-8 paths never collide): how a project root is named in
/// `project.register` and in [`RegisterRequest::root_sha256`]. The child
/// must hash the canonical root, exactly as stored in the manifest.
pub fn root_sha256(root: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        root.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = root.to_string_lossy().into_owned().into_bytes();
    ident::hex(&Sha256::digest(bytes))
}

fn cert_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.cert.json"))
}

fn write_cert_file(dir: &Path, cert: &ProjectCert) -> Result<(), IssueError> {
    let path = cert_path(dir, &cert.project_id);
    let text = serde_json::to_vec_pretty(cert).map_err(|e| IssueError::Store(e.to_string()))?;
    if std::fs::read(&path).is_ok_and(|b| b == text) {
        return Ok(());
    }
    ident::write_private_atomic(&path, &text, false)?;
    Ok(())
}

fn clean_reason(v: Option<&Value>) -> String {
    v.and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_REASON)
        .collect()
}

fn registered_manifest(
    env: &CertEnv,
    id: &str,
) -> Result<clawft_types::project::ProjectManifest, IssueError> {
    validate_id(id).map_err(|_| IssueError::Invalid("project id is not a canonical ULID".into()))?;
    find_by_id(&env.manifests_dir, id)
        .map_err(|e: ProjectError| IssueError::Store(e.to_string()))?
        .ok_or_else(|| IssueError::NotRegistered(id.to_owned()))
}

fn verify_pop(
    env: &CertEnv,
    op: PopOp,
    pubkey: &[u8; 32],
    nonce: &DaemonNonce,
    project_id: &str,
    sig: &[u8; 64],
) -> Result<(), IssueError> {
    if nonce.project_id() != project_id {
        return Err(IssueError::Pop("nonce was issued for another project".into()));
    }
    let uk = key_id(&user_pubkey(env));
    ident::pop_verify(pubkey, op, &uk, nonce.as_str(), project_id, sig)
        .map_err(|e| IssueError::Pop(e.to_string()))
}

fn user_pubkey(env: &CertEnv) -> [u8; 32] {
    env.user_key.verifying_key().to_bytes()
}

/// Certify the key in `req` for `project.register`. Refuses an id that is
/// not in the manifest store, a root that is not that project's registered
/// root, a key the caller cannot prove (PoP over a daemon-issued nonce), a
/// second key for a certified id, a key used by another project, and a
/// revoked key. Idempotent for the certified key.
pub fn register(env: &CertEnv, req: RegisterRequest, now: DateTime<Utc>) -> Result<Issued, IssueError> {
    let manifest = registered_manifest(env, &req.project_id)?;
    if req.root_sha256 != root_sha256(&manifest.root) {
        return Err(IssueError::RootMismatch);
    }
    verify_pop(env, PopOp::Register, &req.project_pubkey, &req.nonce, &req.project_id, &req.pop_sig)?;
    let journal = IdentityJournal::new(&env.manifests_dir);
    let lock = journal.lock()?;
    match current_view_locked(env, &lock)?.plan_registration(&req.project_id, &req.project_pubkey)? {
        Registration::Existing(cert) => {
            ident::verify_cert_at(&cert, &user_pubkey(env), now).map_err(IdentityError::from)?;
            write_cert_file(&env.manifests_dir, &cert)?;
            Ok(Issued { cert: *cert, new: false })
        }
        Registration::New { serial } => {
            let cert = ident::sign_cert(
                &env.user_key,
                &CertRequest {
                    project_id: req.project_id.clone(),
                    project_pubkey: req.project_pubkey,
                    serial,
                    issued_at: now,
                    expires_at: None,
                },
            )?;
            journal.append(&lock, &JournalRecord::Register { cert: cert.clone() })?;
            env.chain.append(
                SOURCE,
                KIND_REGISTER,
                Some(json!({
                    "cert": cert,
                    "name": manifest.name,
                    "root_sha256": req.root_sha256,
                    "manifest_schema": manifest.schema_version,
                    "spawn": { "pid": req.spawn.pid, "exe_sha": req.spawn.exe_sha },
                })),
            );
            write_cert_file(&env.manifests_dir, &cert)?;
            Ok(Issued { cert, new: true })
        }
    }
}

/// `project.rekey`: replace the certified key with `new_pubkey` (proved by
/// PoP over a nonce from `project.cert.challenge`). The old key is revoked
/// for good.
pub fn rekey(env: &CertEnv, params: &Value, now: DateTime<Utc>) -> Result<Issued, IssueError> {
    let id = str_param(params, "id")?;
    registered_manifest(env, id)?;
    let new_pk = pubkey_param(params, "new_pubkey")?;
    let nonce = claim_nonce(str_param(params, "nonce")?, id)?;
    let sig = sig_param(params, "pop_sig")?;
    verify_pop(env, PopOp::Rekey, &new_pk, &nonce, id, &sig)?;
    let journal = IdentityJournal::new(&env.manifests_dir);
    let lock = journal.lock()?;
    let (old_key_id, serial) = current_view_locked(env, &lock)?.plan_rekey(id, &new_pk)?;
    let cert = ident::sign_cert(
        &env.user_key,
        &CertRequest {
            project_id: id.to_owned(),
            project_pubkey: new_pk,
            serial,
            issued_at: now,
            expires_at: None,
        },
    )?;
    journal.append(&lock, &JournalRecord::Rekey { old_key_id: old_key_id.clone(), cert: cert.clone() })?;
    env.chain.append(
        SOURCE,
        KIND_REKEY,
        Some(json!({
            "project_id": id,
            "old_key_id": old_key_id,
            "new_cert": cert,
            "reason": clean_reason(params.get("reason")),
        })),
    );
    write_cert_file(&env.manifests_dir, &cert)?;
    Ok(Issued { cert, new: true })
}

/// `project.revoke`: revoke the certified key. The project has no key
/// until a later register (the revoked key can never return).
pub fn revoke(env: &CertEnv, params: &Value) -> Result<Value, IssueError> {
    let id = str_param(params, "id")?;
    validate_id(id).map_err(|_| IssueError::Invalid("project id is not a canonical ULID".into()))?;
    let journal = IdentityJournal::new(&env.manifests_dir);
    let lock = journal.lock()?;
    let old = current_view_locked(env, &lock)?
        .bound_key_id(id)
        .ok_or_else(|| IdentityError::NotBound(id.to_owned()))?
        .to_owned();
    journal.append(&lock, &JournalRecord::Revoke { project_id: id.to_owned(), key_id: old.clone() })?;
    env.chain.append(
        SOURCE,
        KIND_REVOKE,
        Some(json!({
            "project_id": id,
            "old_key_id": old,
            "reason": clean_reason(params.get("reason")),
        })),
    );
    match std::fs::remove_file(cert_path(&env.manifests_dir, id)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(IssueError::Store(format!(
                "key {old} is revoked, but removing {id}.cert.json failed: {e}"
            )));
        }
    }
    Ok(json!({ "project_id": id, "revoked_key_id": old }))
}

/// `project.cert.show`: the certificate in force, if any.
pub fn show(env: &CertEnv, params: &Value) -> Result<Value, IssueError> {
    let id = str_param(params, "id")?;
    validate_id(id).map_err(|_| IssueError::Invalid("project id is not a canonical ULID".into()))?;
    let view = current_view(env)?;
    Ok(json!({
        "project_id": id,
        "certified": view.bound_key_id(id).is_some(),
        "cert": view.current_cert(id),
        "last_serial": view.last_serial(id),
    }))
}

/// `project.cert.challenge`: a nonce for a `project.rekey` proof.
pub fn challenge(env: &CertEnv, params: &Value) -> Result<Value, IssueError> {
    let id = str_param(params, "id")?;
    registered_manifest(env, id)?;
    Ok(json!({
        "nonce": issue_challenge(id)?,
        "ttl_secs": CHALLENGE_TTL.as_secs(),
        "user_key_id": key_id(&user_pubkey(env)),
        "op": PopOp::Rekey.as_str(),
    }))
}

fn str_param<'a>(p: &'a Value, k: &str) -> Result<&'a str, IssueError> {
    p.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| IssueError::Invalid(format!("missing string param `{k}`")))
}

fn pubkey_param(p: &Value, k: &str) -> Result<[u8; 32], IssueError> {
    ident::parse_pubkey(str_param(p, k)?)
        .ok_or_else(|| IssueError::Invalid(format!("`{k}` must be 64 lowercase hex characters")))
}

fn sig_param(p: &Value, k: &str) -> Result<[u8; 64], IssueError> {
    clawft_types::project::canon::hex_decode::<64>(str_param(p, k)?)
        .ok_or_else(|| IssueError::Invalid(format!("`{k}` must be 128 lowercase hex characters")))
}

/// Build the environment from the daemon: only the user daemon, which
/// signs with the user key and keeps the manifests.
pub(crate) async fn env_from(ctx: &ExtCtx) -> Result<CertEnv, IssueError> {
    if !crate::user_daemon::is_active() {
        return Err(IssueError::Unavailable(
            "project certificates are issued by the user daemon only".into(),
        ));
    }
    let k = ctx.kernel.read().await;
    let chain = k.chain_manager().cloned();
    let user_key = chain.as_ref().and_then(|c| c.signing_key_clone());
    let (Some(chain), Some(user_key)) = (chain, user_key) else {
        return Err(IssueError::Unavailable("the user chain has no signing key".into()));
    };
    let manifests_dir = crate::project_rpc::configured_dir()
        .ok_or_else(|| IssueError::Unavailable("no manifest store (no home directory)".into()))?;
    Ok(CertEnv { chain, user_key, manifests_dir })
}

/// Certify a registering child's key. Shared by package H's
/// `mesh.register`; H verifies the spawn nonce and issued the PoP nonce.
pub async fn issue_for_register(ctx: &ExtCtx, req: RegisterRequest) -> Result<Issued, IssueError> {
    let env = env_from(ctx).await?;
    tokio::task::spawn_blocking(move || register(&env, req, Utc::now()))
        .await
        .map_err(|e| IssueError::Store(format!("task failed: {e}")))?
}

/// Handler for `project.cert.show`, `project.cert.challenge`,
/// `project.rekey`, `project.revoke` and `project.identity.repair`.
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        if call
            .ctx
            .auth
            .as_deref()
            .is_some_and(|a| a.trim().starts_with(SECRET_PREFIX))
        {
            return Response::error_with_kind(
                crate::token_rpc::TOKEN_CANNOT_MINT_KIND,
                "a token cannot manage project certificates; use the local socket as the owner",
            );
        }
        let env = match env_from(&call.ctx).await {
            Ok(e) => e,
            Err(e) => return e.response(),
        };
        let (method, params) = (call.method, call.params);
        let out = tokio::task::spawn_blocking(move || match method.as_str() {
            "project.cert.show" => show(&env, &params),
            "project.cert.challenge" => challenge(&env, &params),
            "project.identity.repair" => repair(&env),
            "project.rekey" => rekey(&env, &params, Utc::now())
                .map(|i| json!({ "cert": i.cert })),
            "project.revoke" => revoke(&env, &params),
            other => Err(IssueError::Invalid(format!("unknown method: {other}"))),
        })
        .await;
        match out {
            Ok(Ok(v)) => Response::success(v),
            Ok(Err(e)) => e.response(),
            Err(e) => Response::error(format!("project cert task failed: {e}")),
        }
    })
}

#[cfg(test)]
#[path = "project_cert_rpc_tests.rs"]
mod tests;
