//! Mints a TEST licence directory and signed record files for exercising the ADR-106 start check
//! on a real device (see docs/cogs/weft-licence.md, "Running cogs on the Seed"). Everything is
//! signed with an operator key (approvals, binding, revocation) and a separate grant key (grants;
//! the kernel refuses a binding whose grant key is an operator key) and a fixed test mesh id, so it proves
//! the host's verification, not the licence service.
//!
//! mint_test_licence --key <seed-file> --grant-key <seed-file> --binary <cog binary> --cog-id <id> --version <v> --out <dir>
//!
//! Writes `<dir>/licence/{config,trust}.json` and `<dir>/{binding,grant,approval,revoke}.json`
//! (each a `Records` document for `weft-cog-host licence import`). Never prints the key.

use clawft_kernel::licence::{
    Approval, BindState, BindingRecord, CheckoutGrant, GrantArtifact, LicenceRef, MeshId, sign_approval, sign_binding, sign_grant,
};
use clawft_kernel::mesh_swarm_revoke::sign_revocation;
use clawft_kernel::revocation::RevocationKind;
use ed25519_dalek::SigningKey;
use weftos_cog_host::licence::{Records, hashes};

fn arg(a: &[String], f: &str) -> String {
    a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).cloned().unwrap_or_else(|| panic!("missing {f}"))
}

fn hexk(k: &SigningKey) -> String {
    k.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn write(path: std::path::PathBuf, v: serde_json::Value) {
    std::fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    println!("wrote {}", path.display());
}

fn load_key(path: String) -> SigningKey {
    let seed_hex = std::fs::read_to_string(path).unwrap();
    let seed: Vec<u8> = (0..32).map(|i| u8::from_str_radix(&seed_hex.trim()[2 * i..2 * i + 2], 16).unwrap()).collect();
    SigningKey::from_bytes(&seed.try_into().unwrap())
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (key, grant_key) = (load_key(arg(&a, "--key")), load_key(arg(&a, "--grant-key")));
    let bytes = std::fs::read(arg(&a, "--binary")).unwrap();
    let (cog_id, version) = (arg(&a, "--cog-id"), arg(&a, "--version"));
    let out = std::path::PathBuf::from(arg(&a, "--out"));
    std::fs::create_dir_all(out.join("licence")).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let mesh = MeshId::derive(&[9; 32], &[7; 32]).to_hex();
    let (sha256, blake3) = hashes(&bytes);

    write(out.join("licence/config.json"), serde_json::json!({"mesh_id": mesh}));
    write(
        out.join("licence/trust.json"),
        serde_json::json!({"schema": "weftos.workload-trust.v1", "operator_keys": [{"key_id": "cog0-test-operator", "public_key": hexk(&key)}]}),
    );

    let binding = BindingRecord {
        v: 2,
        device_id: "cog0-test".into(),
        device_pubkey: hexk(&SigningKey::from_bytes(&[20; 32])),
        mesh_id: mesh.clone(),
        grant_pubkey: hexk(&grant_key),
        steward_node_id: "node-test-steward".into(),
        steward_pubkey: hexk(&SigningKey::from_bytes(&[21; 32])),
        state: BindState::Bound,
        seq: 1,
        bound_at: now,
    };
    let rec = |r: Records| serde_json::to_value(r).unwrap();
    write(out.join("binding.json"), rec(Records { binding: Some(sign_binding(&binding, &key).unwrap()), ..Default::default() }));

    let grant = CheckoutGrant {
        v: 1,
        grant_id: String::new(),
        mesh_id: mesh.clone(),
        seed_device_id: "cog0-test".into(),
        grant_key_id: String::new(),
        source: "cognitum".into(),
        registry: "registry.example".into(),
        cog_id: cog_id.clone(),
        version: version.clone(),
        artifacts: vec![GrantArtifact { arch: "arm".into(), size: bytes.len() as u64, sha256: sha256.clone(), blake3: blake3.clone() }],
        manifest_sha256: weftos_cog_repo::sha256_hex(b"manifest"),
        licence: LicenceRef { ref_sha256: weftos_cog_repo::sha256_hex(b"licence"), expires: now + 30 * 86400 },
        seq: 1,
        issued_at: now,
        expires_at: now + 86400,
    };
    write(out.join("grant.json"), rec(Records { grants: vec![sign_grant(&grant, &grant_key).unwrap()], ..Default::default() }));

    let approval = Approval { v: 1, mesh_id: mesh, cog_id, version, sha256: vec![sha256], approved_at: now };
    write(out.join("approval.json"), rec(Records { approvals: vec![sign_approval(&approval, &key).unwrap()], ..Default::default() }));

    let notice = sign_revocation(RevocationKind::ArtifactHash, &blake3, "cog0 test revocation", now, &key).unwrap();
    write(out.join("revoke.json"), rec(Records { revocations: vec![notice], ..Default::default() }));
    println!("cog blake3 {blake3}");
}
