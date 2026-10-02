//! Journal- and bindings-derived listings used by the admin verbs.

use serde_json::{json, Value};

use clawft_mesh_local::{hexser, node_id_from_pubkey, Principal};

use crate::registry::Registry;
use crate::state::Core;

fn principal_json(p: &Principal) -> Value {
    match p {
        Principal::Uid(u) => json!({"uid": u}),
        Principal::Sid(s) => json!({"sid": s}),
    }
}

/// `bindings.list` reply data: bound, pending and revoked principals.
pub fn bindings_json(core: &Core, registry: &Registry, force_revoked: &[Principal]) -> Value {
    let bound: Vec<Value> = core
        .bindings
        .bound_principals()
        .iter()
        .map(|(p, k)| {
            let uid = node_id_from_pubkey(k);
            json!({
                "principal": principal_json(p), "user_id": uid, "user_pubkey": hexser::encode(k),
                "serials": core.bindings.serials(p),
                "registered": registry.get(&uid).is_some(),
            })
        })
        .collect();
    let pending: Vec<Value> = core
        .bindings
        .pending_principals()
        .iter()
        .map(|(p, k)| {
            json!({"principal": principal_json(p), "user_id": node_id_from_pubkey(k),
                   "user_pubkey": hexser::encode(k)})
        })
        .collect();
    let revoked: Vec<Value> =
        core.bindings.revoked_principal_list().iter().map(principal_json).collect();
    json!({
        "bound": bound, "pending": pending, "revoked": revoked,
        "force_revoked": force_revoked.iter().map(principal_json).collect::<Vec<_>>(),
        "degraded": core.bindings.degraded(),
        "read_only": core.journal.read_only(),
    })
}
