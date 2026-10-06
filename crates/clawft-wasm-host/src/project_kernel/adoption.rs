//! A launcher must call this before adopting a Wasmtime project runner.
//! The executable/PID checks remain the native supervisor's responsibility.
use clawft_types::project::{
    ProjectCert,
    canon::{canonical_json, hex_decode},
};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;

/// All expected values come from the supervisor's own launch record.
pub struct Expected<'a> {
    pub project_id: &'a str,
    pub pid: u32,
    pub socket: &'a str,
    pub root_sha256: &'a str,
    pub artifact_sha256: &'a str,
}

/// Verify the reply to `kernel.handshake {challenge}`. The caller must choose
/// a fresh random 32-byte challenge for EVERY attempt, encoded as lower hex.
/// Identity, root, host socket, runner PID, selected adapter and artifact hash
/// are signed by the certified guest key; no logical/native fallback exists.
pub fn verify_adoption(
    reply: &Value,
    challenge: &str,
    cert: &ProjectCert,
    user: &[u8; 32],
    expected: &Expected<'_>,
) -> Result<(), String> {
    if hex_decode::<32>(challenge).is_none() {
        return Err("invalid adoption challenge".into());
    }
    cert.verify(user, chrono::Utc::now())
        .map_err(|e| e.to_string())?;
    if cert.project_id != expected.project_id
        || reply["project_id"].as_str() != Some(expected.project_id)
        || reply["node_id"].as_str() != Some(&cert.project_key_id)
        || reply["user_key_id"].as_str() != Some(&cert.user_key_id)
        || reply["pid"].as_u64() != Some(expected.pid as u64)
        || reply["socket"].as_str() != Some(expected.socket)
        || reply["root_sha256"].as_str() != Some(expected.root_sha256)
        || reply["artifact_sha256"].as_str() != Some(expected.artifact_sha256)
        || reply["sandbox"] != "wasmtime"
        || reply["adapter"] != "wasmtime-project-v1"
        || reply["profile"] != "project"
    {
        return Err("adoption launch identity mismatch".into());
    }
    let sig = hex_decode::<64>(
        reply["guest_sig"]
            .as_str()
            .ok_or("unsigned guest handshake")?,
    )
    .ok_or("bad handshake signature")?;
    let mut body = reply.clone();
    body.as_object_mut()
        .ok_or("invalid handshake")?
        .remove("guest_sig");
    let bytes = format!(
        "weftos-wasm-project-handshake-v1\n{challenge}\n{}",
        canonical_json(&body)
    );
    let pubkey = hex_decode::<32>(&cert.project_pubkey).ok_or("invalid project key")?;
    VerifyingKey::from_bytes(&pubkey)
        .map_err(|e| e.to_string())?
        .verify_strict(bytes.as_bytes(), &Signature::from_bytes(&sig))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_types::project::{CertRequest, canon::hex_encode};
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    #[test]
    fn adoption_binds_every_launch_fact_and_fresh_challenge() {
        let user = SigningKey::from_bytes(&[1; 32]);
        let guest = SigningKey::from_bytes(&[2; 32]);
        let id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let cert = ProjectCert::sign(
            &user,
            &CertRequest {
                project_id: id.into(),
                project_pubkey: guest.verifying_key().to_bytes(),
                serial: 1,
                issued_at: chrono::Utc::now(),
                expires_at: None,
            },
        );
        let hash = "ab".repeat(32);
        let nonce = "cd".repeat(32);
        let expected = Expected {
            project_id: id,
            pid: 123,
            socket: "/isolated/run/kernel.sock",
            root_sha256: &hash,
            artifact_sha256: &hash,
        };
        let mut reply = json!({"project_id":id,"node_id":cert.project_key_id,"user_key_id":cert.user_key_id,
            "pid":123,"socket":expected.socket,"root_sha256":hash,"artifact_sha256":hash,
            "sandbox":"wasmtime","adapter":"wasmtime-project-v1","profile":"project"});
        let msg = format!(
            "weftos-wasm-project-handshake-v1\n{nonce}\n{}",
            canonical_json(&reply)
        );
        reply["guest_sig"] = json!(hex_encode(&guest.sign(msg.as_bytes()).to_bytes()));
        let pk = user.verifying_key().to_bytes();
        assert!(verify_adoption(&reply, &nonce, &cert, &pk, &expected).is_ok());
        assert!(verify_adoption(&reply, &"ef".repeat(32), &cert, &pk, &expected).is_err());
        for field in [
            "project_id",
            "node_id",
            "user_key_id",
            "pid",
            "socket",
            "root_sha256",
            "artifact_sha256",
            "sandbox",
            "adapter",
            "profile",
        ] {
            let mut changed = reply.clone();
            changed[field] = json!("substituted");
            assert!(
                verify_adoption(&changed, &nonce, &cert, &pk, &expected).is_err(),
                "{field}"
            );
        }
        let mut no_sig = reply.clone();
        no_sig.as_object_mut().unwrap().remove("guest_sig");
        assert!(verify_adoption(&no_sig, &nonce, &cert, &pk, &expected).is_err());
    }
}
