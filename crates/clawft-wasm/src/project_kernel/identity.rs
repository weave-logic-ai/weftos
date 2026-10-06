//! Guest-side helpers: parent RPC, certificate and acknowledgement checks,
//! parent-policy verification and the governance engine builder.
use crate::{
    Result, abi, bridge,
    chain::ChainManager,
    governance::GovernanceEngine,
    governance_overlay::{self, Effective},
    mesh_local::*,
    parent_policy::{ParentPolicy, verify_parent_policy},
};
use chrono::Utc;
use clawft_types::project::{
    ProjectCert,
    canon::{hex_decode, hex_encode},
    cert::key_id,
};
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(crate) fn parent(id: &str, method: &str, params: Value) -> Result<Value> {
    let input =
        serde_json::to_vec(&json!({"method":method,"params":params,"project":id,"proto":1}))?;
    let reply: Value = serde_json::from_slice(&bridge(abi::PARENT, &input)?)?;
    if reply["ok"] != true {
        return Err(reply["error_kind"]
            .as_str()
            .unwrap_or("parent_unavailable")
            .to_owned()
            .into());
    }
    Ok(reply["result"].clone())
}

pub(crate) fn cert_check(
    cert: &ProjectCert,
    boot: &abi::Boot,
    key: &SigningKey,
    user: &[u8; 32],
) -> Result<()> {
    cert.verify(user, Utc::now())?;
    if cert.project_id != boot.project_id
        || cert.project_pubkey != hex_encode(&key.verifying_key().to_bytes())
        || cert.user_key_id != key_id(user)
    {
        return Err("certificate identity mismatch".into());
    }
    Ok(())
}

pub(crate) fn ack_check(
    ack: &RegisterAck,
    boot: &abi::Boot,
    key: &SigningKey,
    user: &[u8; 32],
    nonce: &str,
    client: &str,
) -> Result<ProjectCert> {
    if !ack.ok || ack.session.is_empty() || ack.proto.min > 1 || ack.proto.current < 1 {
        return Err("registration protocol refused".into());
    }
    let sig = hex_decode::<64>(
        ack.parent_sig
            .as_deref()
            .ok_or("unsigned registration ack")?,
    )
    .ok_or("bad ack signature")?;
    VerifyingKey::from_bytes(user)?.verify_strict(
        &ack_signed_bytes(&boot.project_id, &ack.session, nonce, client),
        &Signature::from_bytes(&sig),
    )?;
    let cert = ack.cert.as_ref().ok_or("missing project certificate")?;
    cert_check(cert, boot, key, user)?;
    Ok(cert.clone())
}

pub(crate) fn policy(
    boot: &abi::Boot,
    user: &[u8; 32],
    chain: &ChainManager,
    value: Value,
) -> Result<Effective> {
    let p: ParentPolicy = serde_json::from_value(value)?;
    verify_parent_policy(&p, user)?;
    let max = chain
        .tail(0)
        .iter()
        .filter(|e| e.source == "governance" && e.kind == "governance.overlay.applied")
        .filter_map(|e| e.payload.as_ref()?.get("parent_version")?.as_u64())
        .max()
        .unwrap_or(0);
    let pin = match fs::read_to_string("/project/state/parent-policy.version") {
        Ok(text) => Some(text.trim().parse::<u64>()?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if max > 0 && pin.is_none() {
        return Err("parent policy version pin missing".into());
    }
    if p.version < max.max(pin.unwrap_or(0)) {
        return Err("parent policy rollback".into());
    }
    if boot.project_id.is_empty() {
        return Err("missing project id".into());
    }
    Ok(governance_overlay::merge(
        &p,
        &governance_overlay::load_overlay(Path::new("/project/overlay.toml"))?,
    )?)
}

pub(crate) fn governance_overlay_effect() -> crate::governance::EffectVector {
    crate::governance::EffectVector {
        risk: 0.2,
        fairness: 0.0,
        privacy: 0.0,
        novelty: 0.0,
        security: 0.2,
    }
}

pub(crate) fn engine(e: &Effective) -> GovernanceEngine {
    let mut g = GovernanceEngine::new(e.risk_threshold(0.7), e.human_approval(false));
    for rule in e.rules.clone() {
        g.add_rule(rule);
    }
    g
}
