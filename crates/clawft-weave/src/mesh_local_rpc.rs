//! `mesh.challenge | register | heartbeat | unregister`: the user daemon's
//! side of `mesh-local/1` (ADR-103 A6, Phase 2 package H).
//!
//! Authenticated by the spawn nonce and the project key's proof of
//! possession, not by a capability token (a child holds none before it
//! registers). Until Phase 3 binds peer credentials the guards are the 0600
//! socket, the single-use spawn nonce the supervisor filed
//! ([`crate::mesh_local_registry::expect_spawn`]) and the PoP over a
//! daemon-issued challenge. They stop a hand-started or copied-`spawn.json`
//! child; they do not stop a same-uid process that can read the 0600
//! `spawn.json` before the real child does (stated limit, ADR-103 A7).
//!
//! `mesh.register` order (nothing with a side effect runs before the checks
//! that need none):
//!
//! 1. shape: protocol tag, ULID, hex keys, signature;
//! 2. authorisation: an outstanding spawn expectation with the right nonce
//!    and pid (peeked, not consumed), or a re-register of this daemon's own
//!    expired session for the same pid (a parent restart or a missed beat);
//! 3. the root the child hashed equals the root it was spawned in;
//! 4. the challenge nonce is claimed (single use);
//! 5. the certificate is issued or confirmed
//!    ([`issue_for_register`](crate::project_cert_rpc::issue_for_register):
//!    manifest, root, PoP, TOFU, revocation);
//! 6. the spawn nonce is consumed and the session opened (one live session
//!    per project id);
//! 7. the acknowledgement is signed with the user key over a value the child
//!    chose, so a socket squatting at the parent's path cannot forge one.

// `clawft_rpc::Response` is the ready-made refusal returned as the `Err` early-out of these
// handlers; it is built once per refused request, so its size is not on a hot path.
#![allow(clippy::result_large_err)]

use std::time::Instant;

use clawft_kernel::project_identity as ident;
use clawft_rpc::Response;
use clawft_rpc::mesh_local::{
    ChallengeReply, ChallengeRequest, HeartbeatRequest, METHOD_CHALLENGE, METHOD_HEARTBEAT,
    METHOD_REGISTER, METHOD_UNREGISTER, PROTO_MESH_LOCAL, PROTOCOL_TAG, ParentHead, RegisterAck,
    RegisterRequest, UnregisterRequest, ack_signed_bytes, activity_digest, bind_signed_bytes,
};
use clawft_types::project::canon::hex_decode;
use clawft_types::project::cert::key_id;
use clawft_types::project::validate_id;
use ed25519_dalek::Signer;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::mesh_local_registry::{
    HEARTBEAT_SECS, NewSession, RegistryError, SessionProof, SessionState, consume_spawn, now_unix, peek_spawn,
    registry, spawn_expected,
};
use crate::project_cert_rpc::{
    SpawnInfo, claim_nonce, env_from, issue_challenge, issue_for_register, root_sha256,
};
use crate::rpc_ext::{ExtCall, ExtCtx, ExtFuture};

const MAX_FIELD: usize = 4096;

fn err(kind: &str, msg: impl Into<String>) -> Response {
    Response::error_with_kind(kind, msg.into())
}

fn reg_err(e: &RegistryError) -> Response {
    err(e.kind(), e.to_string())
}

fn params<T: DeserializeOwned>(v: Value) -> Result<T, Response> {
    serde_json::from_value(v)
        .map_err(|e| err("invalid_params", format!("bad mesh-local params: {e}")))
}

/// Handler for the four `mesh.*` methods.
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        let ExtCall {
            method,
            params: p,
            ctx,
        } = call;
        if !crate::user_daemon::is_active() {
            return err(
                "mesh_unavailable",
                "mesh-local registration is served by the user daemon only",
            );
        }
        match method.as_str() {
            METHOD_CHALLENGE => challenge(&ctx, p).await,
            METHOD_REGISTER => register(&ctx, p).await,
            METHOD_HEARTBEAT => heartbeat(p),
            METHOD_UNREGISTER => unregister(p),
            other => err("invalid_params", format!("unknown mesh method {other}")),
        }
    })
}

async fn challenge(ctx: &ExtCtx, p: Value) -> Response {
    let req: ChallengeRequest = match params(p) {
        Ok(r) => r,
        Err(r) => return r,
    };
    if validate_id(&req.project_id).is_err() {
        return err("invalid_params", "project id is not a canonical ULID");
    }
    // A challenge is only handed to a project the supervisor is starting (or
    // whose session this daemon knows), so strangers cannot fill the table.
    let known = registry()
        .state_at(&req.project_id, Instant::now())
        .is_some();
    if !known && !spawn_expected(&req.project_id, now_unix()) {
        return reg_err(&RegistryError::NotExpected(req.project_id));
    }
    let env = match env_from(ctx).await {
        Ok(e) => e,
        Err(e) => return err(e.kind(), e.to_string()),
    };
    match clawft_types::project::find_by_id(&env.manifests_dir, &req.project_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return err(
                "project_not_found",
                format!("project {} is not registered", req.project_id),
            );
        }
        Err(e) => return err("project_store_error", e.to_string()),
    }
    match issue_challenge(&req.project_id) {
        Ok(nonce) => Response::success(
            serde_json::to_value(ChallengeReply {
                nonce,
                user_key_id: key_id(&env.user_key.verifying_key().to_bytes()),
            })
            .unwrap_or(Value::Null),
        ),
        Err(e) => err(e.kind(), e.to_string()),
    }
}

/// How a registration is authorised.
enum Auth {
    Spawn(crate::mesh_local_registry::SpawnExpectation),
    Reregister,
}

fn authorise(req: &RegisterRequest, now: u64) -> Result<Auth, RegistryError> {
    if let Some(n) = req.spawn_nonce.as_deref() {
        // A live session blocks a new one before anything else runs, so the
        // refused child neither gets a certificate nor burns its nonce.
        if let Some((SessionState::Live, _)) = registry().state_at(&req.project_id, Instant::now())
        {
            return Err(RegistryError::SecondSession(req.project_id.clone()));
        }
        return peek_spawn(&req.project_id, Some(n), req.pid, now).map(Auth::Spawn);
    }
    match registry().state_at(&req.project_id, Instant::now()) {
        Some((SessionState::Expired, pid)) if pid == req.pid => Ok(Auth::Reregister),
        Some((SessionState::Live, _)) => Err(RegistryError::SecondSession(req.project_id.clone())),
        Some((SessionState::Expired, want)) => {
            Err(RegistryError::PidMismatch { got: req.pid, want })
        }
        None => Err(RegistryError::NotExpected(req.project_id.clone())),
    }
}

async fn register(ctx: &ExtCtx, p: Value) -> Response {
    let req: RegisterRequest = match params(p) {
        Ok(r) => r,
        Err(r) => return r,
    };
    // 1. shape
    if req.protocol != PROTOCOL_TAG {
        return err(
            "proto_mismatch",
            format!("expected {PROTOCOL_TAG}, got {:?}", req.protocol),
        );
    }
    if validate_id(&req.project_id).is_err() {
        return err("invalid_params", "project id is not a canonical ULID");
    }
    let (Some(pubkey), Some(pop_sig)) = (
        ident::parse_pubkey(&req.project_pubkey),
        hex_decode::<64>(&req.nonce_reply.sig),
    ) else {
        return err(
            "invalid_params",
            "project_pubkey and nonce_reply.sig must be lowercase hex",
        );
    };
    let hex32 =
        |s: &str| s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !hex32(&req.client_nonce) {
        return err(
            "invalid_params",
            "client_nonce must be 32 lowercase hex characters",
        );
    }
    if req.socket.len() > MAX_FIELD
        || req.version.len() > 64
        || req.addresses.len() > 16
        || req.topic_prefixes.len() > 16
    {
        return err("invalid_params", "registration fields are too large");
    }
    // Addresses and topics are the project's own, nothing else.
    let topic_prefix = format!("chain/{}/", req.project_id);
    if req.addresses.iter().any(|a| a != &req.project_id)
        || req
            .topic_prefixes
            .iter()
            .any(|t| !t.starts_with(&topic_prefix))
    {
        return err(
            "invalid_params",
            "addresses and topic_prefixes must belong to the registering project",
        );
    }
    // The binding signature ties this challenge to the child's own socket,
    // pid and client randomness (the fixed PoP does not cover them).
    let bind_ok = hex_decode::<64>(&req.bind_sig).is_some_and(|sig| {
        ed25519_dalek::VerifyingKey::from_bytes(&pubkey).is_ok_and(|vk| {
            vk.verify_strict(
                &bind_signed_bytes(
                    &req.project_id,
                    &req.nonce_reply.nonce,
                    &req.client_nonce,
                    &req.socket,
                    req.pid,
                ),
                &ed25519_dalek::Signature::from_bytes(&sig),
            )
            .is_ok()
        })
    });
    if !bind_ok {
        return err(
            "pop_failed",
            "bind_sig does not verify (socket, pid and client_nonce must be signed by the project key)",
        );
    }
    // 2. authorisation (no side effects yet)
    let now = now_unix();
    let auth = match authorise(&req, now) {
        Ok(a) => a,
        Err(e) => return reg_err(&e),
    };
    // 3. the spawned root
    let spawn_info = match &auth {
        Auth::Spawn(e) => {
            if req.root_sha256 != root_sha256(&e.root) {
                return reg_err(&RegistryError::WrongRoot);
            }
            SpawnInfo {
                pid: req.pid,
                exe_sha: e.exe_sha.clone(),
            }
        }
        Auth::Reregister => SpawnInfo {
            pid: req.pid,
            exe_sha: String::new(),
        },
    };
    // 4. the challenge nonce
    // An unknown, expired, used or evicted challenge is "start over with a
    // fresh one", distinct from a wrong signature.
    let nonce = match claim_nonce(&req.nonce_reply.nonce, &req.project_id) {
        Ok(n) => n,
        Err(e) => return err("challenge_unknown", e.to_string()),
    };
    // 5. the certificate (manifest, root, PoP, TOFU, revocation)
    let issued = match issue_for_register(
        ctx,
        crate::project_cert_rpc::RegisterRequest {
            project_id: req.project_id.clone(),
            project_pubkey: pubkey,
            root_sha256: req.root_sha256.clone(),
            spawn: spawn_info,
            nonce,
            pop_sig,
        },
    )
    .await
    {
        Ok(i) => i,
        Err(e) => return err(e.kind(), e.to_string()),
    };
    // 6. consume the spawn nonce, open the session
    if let Auth::Spawn(e) = &auth
        && !consume_spawn(e)
    {
        return reg_err(&RegistryError::BadSpawnNonce);
    }
    let session = match registry().register_at(
        NewSession {
            project_id: req.project_id.clone(),
            socket: req.socket.clone().into(),
            pid: req.pid,
            addresses: req.addresses.clone(),
            topic_prefixes: req.topic_prefixes.clone(),
            version: req.version.clone(),
            project_key_id: issued.cert.project_key_id.clone(),
            project_pubkey: pubkey,
        },
        Instant::now(),
    ) {
        Ok(s) => s,
        Err(e) => return reg_err(&e),
    };
    // 7. the signed acknowledgement and the user-chain head
    let env = match env_from(ctx).await {
        Ok(e) => e,
        Err(e) => return err(e.kind(), e.to_string()),
    };
    let sig = env.user_key.sign(&ack_signed_bytes(
        &req.project_id,
        &session.session,
        &req.nonce_reply.nonce,
        &req.client_nonce,
    ));
    let head = {
        let k = ctx.kernel.read().await;
        k.chain_manager().map(|c| ParentHead {
            user_seq: c.head_sequence(),
            user_event_hash: ident::hex(&c.head_hash()),
        })
    };
    let ack = RegisterAck {
        ok: true,
        session: session.session,
        cert: Some(issued.cert),
        accepted: vec![req.project_id],
        proto: clawft_rpc::mesh_local::ProtoRange {
            current: PROTO_MESH_LOCAL,
            min: PROTO_MESH_LOCAL,
        },
        heartbeat_secs: HEARTBEAT_SECS,
        machine_cert: None,
        parent_head: head,
        parent_sig: Some(ident::hex(&sig.to_bytes())),
    };
    Response::success(serde_json::to_value(ack).unwrap_or(Value::Null))
}

fn heartbeat(p: Value) -> Response {
    let req: HeartbeatRequest = match params(p) {
        Ok(r) => r,
        Err(r) => return r,
    };
    if let Err(e) = registry().verify_proof_at(
        &SessionProof {
            op: "heartbeat",
            session: &req.session,
            pid: req.pid,
            at_unix: req.at_unix,
            extra: &activity_digest(&req.activity),
            sig_hex: &req.sig,
        },
        now_unix(),
        Instant::now(),
    ) {
        return reg_err(&e);
    }
    match registry().heartbeat_at(&req.session, req.activity, Instant::now()) {
        Ok(()) => Response::success(json!({ "ok": true, "heartbeat_secs": HEARTBEAT_SECS })),
        Err(e) => reg_err(&e),
    }
}

fn unregister(p: Value) -> Response {
    let req: UnregisterRequest = match params(p) {
        Ok(r) => r,
        Err(r) => return r,
    };
    if let Err(e) = registry().verify_proof_at(
        &SessionProof {
            op: "unregister",
            session: &req.session,
            pid: req.pid,
            at_unix: req.at_unix,
            extra: "",
            sig_hex: &req.sig,
        },
        now_unix(),
        Instant::now(),
    ) {
        return reg_err(&e);
    }
    match registry().unregister(&req.session) {
        Ok(id) => {
            tracing::info!(project = %id, reason = %req.reason.chars().take(64).collect::<String>(), "child unregistered");
            Response::success(json!({ "ok": true }))
        }
        Err(e) => reg_err(&e),
    }
}
