//! ADR-103 forward-v2 verification at the guest boundary, before dispatch.
use crate::Result;
use clawft_types::project::canon::{canonical_json, hex_decode, hex_encode};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub struct Forward {
    seen: HashMap<[u8; 64], Instant>,
}
impl Forward {
    pub fn new() -> Self {
        Self {
            seen: HashMap::new(),
        }
    }
    pub fn verify(
        &mut self,
        req: &Value,
        project: &str,
        target: &str,
        user: &[u8; 32],
        now_ms: u64,
    ) -> Result<()> {
        let h = &req["forward"];
        let method = req["method"].as_str().ok_or("missing method")?;
        if req["project"].as_str() != Some(project) || h["project_id"].as_str() != Some(project) {
            return Err("project_scope_mismatch".into());
        }
        if [project, target, method].iter().any(|s| s.contains('\n')) {
            return Err("forward_bad_signature".into());
        }
        let at = h["issued_at_ms"].as_u64().ok_or("forward_bad_signature")?;
        if now_ms.abs_diff(at) > 5000 {
            return Err("forward_expired".into());
        }
        let sig = hex_decode::<64>(h["sig"].as_str().ok_or("forward_bad_signature")?)
            .ok_or("forward_bad_signature")?;
        let hash = hex_encode(&Sha256::digest(canonical_json(&req["params"]).as_bytes()));
        let msg = format!("weftos-project-forward-v2\n{project}\n{at}\n{method}\n{hash}\n{target}");
        VerifyingKey::from_bytes(user)?
            .verify_strict(msg.as_bytes(), &Signature::from_bytes(&sig))?;
        let now = Instant::now();
        self.seen
            .retain(|_, t| now.saturating_duration_since(*t) <= Duration::from_secs(10));
        if self.seen.contains_key(&sig) {
            return Err("forward_replayed".into());
        }
        if self.seen.len() >= 4096 {
            return Err("forward_capacity".into());
        }
        self.seen.insert(sig, now);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
