//! `GET /licence/v1/artifact/<blake3>` (the byte transfer, 3 per artifact per
//! 24 h per steward key) and `GET /licence/v1/grants?since=<ctr>`.

use serde::{Deserialize, Serialize};
use serde_json::json;
use weft_licence_wire::{
    SignedEnvelope, envelope_key, parse_canonical, sha256_hex, sign_envelope, verify_envelope,
};

use crate::error::ApiError;
use crate::request::Request;
use crate::service::{Body, Response, Service, Steward};
use crate::state::ServeRec;

/// Domain tag of operator serve overrides.
pub const OVERRIDE_DOMAIN: &str = "weft-licence-v1/serve-override";

/// An operator-signed raise of the per-day serve limit, for example after a
/// steward rebuild. It names one steward key and one artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServeOverride {
    /// Always 1.
    pub v: u32,
    /// Steward key id (`ed25519:` + 16 hex).
    pub key_id: String,
    /// Cog id.
    pub cog_id: String,
    /// Version.
    pub version: String,
    /// Architecture.
    pub arch: String,
    /// Extra transfers allowed per 24 h while the override lasts.
    pub extra: u32,
    /// End of the override, unix seconds.
    pub expires_at: u64,
}

/// Sign an override as the operator.
pub fn sign_override(o: &ServeOverride, operator: &ed25519_dalek::SigningKey) -> SignedEnvelope {
    sign_envelope(OVERRIDE_DOMAIN, o, operator).expect("override serializes")
}

/// Verify an override envelope against the pinned operator keys.
pub fn verify_override(
    env: &SignedEnvelope,
    operator_ok: &dyn Fn(&[u8; 32]) -> bool,
) -> Result<ServeOverride, String> {
    let pk = envelope_key(env).map_err(|e| e.to_string())?;
    if !operator_ok(&pk) {
        return Err("signer is not a pinned operator key".into());
    }
    verify_envelope(OVERRIDE_DOMAIN, env, &pk).map_err(|e| e.to_string())?;
    let o: ServeOverride = parse_canonical(&env.payload).map_err(|e| e.to_string())?;
    if o.v != 1 {
        return Err("override version".into());
    }
    Ok(o)
}

/// Install an override file into the state directory (CLI `override`).
pub fn install_override(dir: &std::path::Path, env: &SignedEnvelope) -> Result<(), crate::error::SvcError> {
    let raw = serde_json::to_vec(env).map_err(|e| crate::error::SvcError::Persist(e.to_string()))?;
    let name = format!("{}.json", &sha256_hex(env.payload.as_bytes())[..32]);
    crate::fsio::write_atomic(&dir.join(name), &raw).map_err(|e| crate::error::SvcError::Persist(e.to_string()))
}

impl Service {
    fn override_extra(&self, dir: &std::path::Path, key_id: &str, rec: &ServeRec, now: u64) -> u32 {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
            Err(e) => {
                // No extra transfers are allowed while the overrides cannot be read.
                self.log(&format!("cannot read the overrides directory: {e}"));
                return 0;
            }
        };
        let mut extra = 0u32;
        for ent in rd.flatten().take(64) {
            let Ok(Some(raw)) = crate::fsio::read_capped(&ent.path(), 16 * 1024) else { continue };
            let Ok(env) = serde_json::from_slice::<SignedEnvelope>(&raw) else { continue };
            let Ok(o) = verify_override(&env, &|pk| self.ops.contains(pk)) else { continue };
            if o.key_id == key_id && o.cog_id == rec.cog_id && o.version == rec.version && o.arch == rec.arch && o.expires_at > now {
                extra = extra.saturating_add(o.extra);
            }
        }
        extra
    }

    pub(crate) fn artifact(&self, req: &Request, now: u64, steward: &Steward) -> Response {
        match self.artifact_inner(req, now, steward) {
            Ok(r) => r,
            Err(e) => Response::err(e),
        }
    }

    fn artifact_inner(&self, req: &Request, now: u64, steward: &Steward) -> Result<Response, ApiError> {
        let hash = req.path().trim_start_matches("/licence/v1/artifact/");
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(ApiError::new(400, "bad_request", "artifact id is a 64-hex BLAKE3"));
        }
        let permit = self
            .permits
            .try_acquire()
            .ok_or_else(|| ApiError::new(429, "busy", "a transfer is already in flight"))?;
        let mut inner = self.lock();
        self.refresh_binding(&mut inner);
        let (Some(sk), Some(mesh)) = (inner.key.clone(), inner.binding.as_ref().filter(|b| b.is_bound()).map(|b| b.record.mesh_id.clone())) else {
            return Err(ApiError::new(409, "seed_not_bound", "no mesh binding"));
        };
        let pk = sk.verifying_key().to_bytes();
        let found = inner.store.slots.slots.values().find_map(|s| {
            let live = Service::valid_grant(s, now, &pk, &mesh).is_some();
            s.arches.values().find(|a| a.blake3 == hash).filter(|_| live).map(|a| (s.cog_id.clone(), s.version.clone(), a.arch.clone()))
        });
        let (cog_id, version, arch) = found.ok_or_else(|| ApiError::new(404, "no_grant", "no active grant covers this artifact"))?;
        let rec = ServeRec { key_id: steward.key_id.clone(), cog_id, version, arch, ts: now };
        inner.store.serves.retain(|s| s.ts.saturating_add(86_400) > now);
        let used = inner
            .store
            .serves
            .iter()
            .filter(|s| s.key_id == rec.key_id && s.cog_id == rec.cog_id && s.version == rec.version && s.arch == rec.arch)
            .count() as u32;
        let dir = inner.store.overrides_dir();
        let allowed = self.cfg.limits.serves_per_day.saturating_add(self.override_extra(&dir, &rec.key_id, &rec, now));
        if used >= allowed {
            return Err(ApiError::new(429, "serve_limit", format!("{used} transfers in 24 h; an operator override raises it")));
        }
        let (path, len) = inner.cache.get(hash).ok_or_else(|| ApiError::new(410, "gone", "the artifact is no longer cached"))?;
        // Open under the lock: the handle, not the path, is what gets sent.
        let file = std::fs::File::open(&path).map_err(|_| ApiError::new(410, "gone", "the artifact file is missing"))?;
        let line = format!(
            "byte transfer cog={} version={} arch={} bytes={} steward={} (serve {} of {})",
            rec.cog_id, rec.version, rec.arch, len, rec.key_id, used.saturating_add(1), allowed
        );
        inner.store.serves.push(rec);
        inner.store.persist_serves().map_err(|e| ApiError::new(503, "persist_failed", e.to_string()))?;
        self.log(&line);
        Ok(Response { status: 200, body: Body::File { file, len }, permit: Some(permit) })
    }

    pub(crate) fn grants(&self, req: &Request) -> Response {
        let since: u64 = req.query("since").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut inner = self.lock();
        self.refresh_binding(&mut inner);
        let mesh = inner.binding.as_ref().filter(|b| b.is_bound()).map(|b| b.record.mesh_id.clone()).unwrap_or_default();
        let mut rows: Vec<(u64, &SignedEnvelope)> = inner
            .store
            .slots
            .slots
            .values()
            .filter(|s| s.mesh_id == mesh && s.issue_ctr > since)
            .filter_map(|s| s.grant.as_ref().map(|g| (s.issue_ctr, g)))
            .collect();
        rows.sort_by_key(|(c, _)| *c);
        let batch = self.cfg.limits.renew_batch;
        let more = rows.len() > batch;
        rows.truncate(batch);
        let next = rows.last().map(|(c, _)| *c).unwrap_or(since);
        let grants: Vec<&SignedEnvelope> = rows.iter().map(|(_, g)| *g).collect();
        Response::json(200, json!({"grants": grants, "next": next, "more": more}))
    }
}
