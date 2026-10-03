//! `POST /licence/v1/checkout` and `POST /licence/v1/renew` (ADR-106 sections
//! 4 and 6). A grant's `seq` is on disk before the grant is released.

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;
use serde_json::{Value, json};
use weft_licence_wire::{
    CheckoutGrant, GrantArtifact, LicenceRef, MAX_GRANT_TTL_SECS, MeshId, SignedGrant, hex_encode,
    sha256_hex, sign_grant, verify_grant_signature,
};

use crate::cache::CacheError;
use crate::error::ApiError;
use crate::providers::{Entitlement, FetchError, LicenceCheckError};
use crate::request::Request;
use crate::service::{Inner, Response, Service, Steward};
use crate::state::{Slot, SlotStatus, Slots, slot_key};

#[derive(Deserialize)]
struct CheckoutReq {
    #[serde(default)]
    request_id: String,
    cog_id: String,
    version: String,
    arch: String,
}

#[derive(Deserialize, Default)]
struct RenewReq {
    #[serde(default)]
    release: Vec<ReleaseRef>,
}

#[derive(Deserialize)]
struct ReleaseRef {
    cog_id: String,
    version: String,
}

fn token_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub(crate) fn licence_error(e: LicenceCheckError) -> ApiError {
    match e {
        LicenceCheckError::Unlicensed => ApiError::new(403, "cog_unlicensed", "no licence covers this cog"),
        LicenceCheckError::Expired => ApiError::new(403, "licence_expired", "the covering licence has expired"),
        LicenceCheckError::Unreadable(m) => ApiError::new(503, "licence_unreadable", m),
    }
}

fn fetch_error(e: FetchError) -> ApiError {
    match e {
        FetchError::NotFound => ApiError::new(404, "cog_not_found", "the registry does not list this cog"),
        FetchError::VersionUnavailable(v) => ApiError::new(404, "version_unavailable", format!("registry lists {v}")),
        FetchError::ArchUnavailable(a) => ApiError::new(404, "arch_unavailable", format!("no {a} binary in the registry")),
        FetchError::SizeUnknown => ApiError::new(422, "size_unknown", "the registry gives no size"),
        FetchError::Failed(m) => ApiError::new(502, "fetch_failed", m),
        FetchError::Verify(m) => ApiError::new(502, "verify_failed", m),
    }
}

/// The JSON a checkout answers: the grant and where each artifact is served.
pub(crate) fn grant_response(grant: &SignedGrant, g: &CheckoutGrant) -> Value {
    let arts: Vec<Value> = g
        .artifacts
        .iter()
        .map(|a| {
            json!({"arch": a.arch, "size": a.size, "sha256": a.sha256, "blake3": a.blake3,
                   "path": format!("/licence/v1/artifact/{}", a.blake3)})
        })
        .collect();
    json!({"grant": grant, "artifacts": arts})
}

/// Sign the next grant for `slot` (whose `arches`, `seq`, `status`, `licence`
/// are already updated) and store it. `withdraw` makes `expires_at <= issued_at`.
fn sign_slot(
    svc: &Service,
    slots: &mut Slots,
    key: &str,
    mesh: &MeshId,
    now: u64,
    expires_at: u64,
) -> Result<SignedGrant, ApiError> {
    let sk = svc.key.as_ref().ok_or_else(|| ApiError::new(503, "no_key", "no grant key"))?;
    if now < svc.cfg.clock_floor || now < slots.last_issued_at {
        return Err(ApiError::new(503, "clock_not_set", "refusing to sign before the clock floor"));
    }
    slots.ctr += 1;
    slots.last_issued_at = slots.last_issued_at.max(now);
    let ctr = slots.ctr;
    let slot = slots.slots.get_mut(key).expect("slot exists");
    let grant = CheckoutGrant {
        v: 1,
        grant_id: String::new(),
        mesh_id: mesh.to_hex(),
        seed_device_id: svc.cfg.device_id.clone(),
        grant_key_id: String::new(),
        source: "cognitum".into(),
        registry: slot.registry.clone(),
        cog_id: slot.cog_id.clone(),
        version: slot.version.clone(),
        artifacts: slot.arches.values().cloned().collect(),
        manifest_sha256: slot.manifest_sha256.clone(),
        licence: slot.licence.clone(),
        seq: slot.seq,
        issued_at: now,
        expires_at,
    };
    let signed = sign_grant(&grant, sk).map_err(|e| ApiError::new(500, "sign_failed", e.to_string()))?;
    slot.issue_ctr = ctr;
    slot.grant = Some(signed.clone());
    Ok(signed)
}

fn licence_ref(ent: &Entitlement, now: u64) -> LicenceRef {
    // No declared expiry: the licence side reads as "now + the 7 day maximum",
    // so a verifier's `max(now, floor) < licence.expires` check stays finite.
    LicenceRef { ref_sha256: ent.ref_sha256.clone(), expires: ent.expires.unwrap_or(now + MAX_GRANT_TTL_SECS) }
}

fn mesh_of(inner: &Inner) -> Option<MeshId> {
    inner.binding.as_ref().filter(|b| b.is_bound()).and_then(|b| MeshId::from_hex(&b.record.mesh_id))
}

fn pinned(slots: &Slots) -> HashSet<String> {
    slots
        .slots
        .values()
        .filter(|s| s.status == SlotStatus::Active)
        .flat_map(|s| s.arches.values().map(|a| a.blake3.clone()))
        .collect()
}

impl Service {
    fn valid_grant(&self, slot: &Slot, now: u64) -> Option<(SignedGrant, CheckoutGrant)> {
        let sk = self.key.as_ref()?;
        let signed = slot.grant.clone()?;
        let g = verify_grant_signature(&signed, &sk.verifying_key().to_bytes()).ok()?;
        (slot.status == SlotStatus::Active && !g.is_withdrawal() && g.expires_at > now).then_some((signed, g))
    }

    pub(crate) fn checkout(&self, req: &Request, now: u64, _steward: &Steward) -> Response {
        match self.checkout_inner(req, now) {
            Ok(v) => Response::json(200, v),
            Err(e) => Response::err(e),
        }
    }

    fn checkout_inner(&self, req: &Request, now: u64) -> Result<Value, ApiError> {
        let body: CheckoutReq = serde_json::from_slice(&req.body)
            .map_err(|e| ApiError::new(400, "bad_request", e.to_string()))?;
        let _ = &body.request_id;
        if !token_ok(&body.cog_id) || !token_ok(&body.version) || !token_ok(&body.arch) {
            return Err(ApiError::new(400, "bad_request", "cog_id, version or arch is malformed"));
        }
        let _permit = self
            .permits
            .try_acquire()
            .ok_or_else(|| ApiError::new(429, "busy", "a checkout is already in flight"))?;
        let mesh = mesh_of(&self.lock()).ok_or_else(|| ApiError::new(409, "seed_not_bound", "no mesh binding"))?;
        let ent = self.licence.entitlement(&body.cog_id, &mesh.to_hex(), now).map_err(licence_error)?;
        let entry = self.fetcher.resolve(&body.cog_id, &body.version).map_err(fetch_error)?;
        let key = slot_key(&entry.cog_id, &entry.version);
        let limit = self.cfg.limits.max_artifact_bytes;
        let art = entry
            .artifacts
            .iter()
            .find(|a| a.arch == body.arch)
            .ok_or_else(|| ApiError::new(404, "arch_unavailable", format!("no {} binary", body.arch)))?;
        {
            let inner = self.lock();
            let held = inner.store.slots.slots.get(&key).cloned();
            if let Some((slot, have)) = held.and_then(|s| s.arches.get(&body.arch).cloned().map(|h| (s, h))) {
                if have.sha256 != art.sha256 {
                    return Err(ApiError::new(409, "artifact_changed", "the registry republished this version"));
                }
                let live = self.valid_grant(&slot, now);
                if let Some((signed, g)) = live.filter(|_| inner.cache.contains(&have.blake3)) {
                    return Ok(grant_response(&signed, &g)); // already checked out
                }
            }
        }
        if art.size > limit {
            return Err(ApiError::new(413, "artifact_too_large", format!("{} bytes over the {limit} limit", art.size)));
        }
        let bytes = self.fetcher.fetch(&entry, &body.arch).map_err(fetch_error)?;
        if bytes.len() as u64 > limit {
            return Err(ApiError::new(413, "artifact_too_large", "downloaded artifact over the limit"));
        }
        let sha = sha256_hex(&bytes);
        if sha != art.sha256 || (art.size != 0 && bytes.len() as u64 > art.size) {
            return Err(ApiError::new(502, "verify_failed", "sha256 does not match the registry"));
        }
        let blake3 = hex_encode(blake3::hash(&bytes).as_bytes());
        let now = (self.clock)().max(now);
        let mut inner = self.lock();
        let mut work = inner.store.slots.clone();
        let slot = work.slots.entry(key.clone()).or_insert_with(|| Slot {
            cog_id: entry.cog_id.clone(),
            version: entry.version.clone(),
            seq: 0,
            arches: BTreeMap::new(),
            manifest_sha256: entry.manifest_sha256.clone(),
            registry: entry.registry.clone(),
            licence: licence_ref(&ent, now),
            status: SlotStatus::Active,
            issue_ctr: 0,
            grant: None,
        });
        // The union of arches applies across checkouts.
        slot.arches.insert(
            body.arch.clone(),
            GrantArtifact { arch: body.arch.clone(), size: bytes.len() as u64, sha256: sha, blake3: blake3.clone() },
        );
        slot.seq += 1;
        slot.status = SlotStatus::Active;
        slot.manifest_sha256 = entry.manifest_sha256.clone();
        slot.licence = licence_ref(&ent, now);
        let pins = {
            let mut p = pinned(&work);
            p.insert(blake3.clone());
            p
        };
        inner.cache.put(&blake3, &bytes, &pins).map_err(|e| match e {
            CacheError::TooLarge => ApiError::new(413, "artifact_too_large", "larger than the cache"),
            CacheError::Full => ApiError::new(507, "cache_full", "the cache holds only active checkouts"),
            CacheError::Io(m) => ApiError::new(503, "cache_io", m),
        })?;
        let expires_at = (now + self.cfg.grant_ttl_secs).min(work.slots[&key].licence.expires);
        let signed = sign_slot(self, &mut work, &key, &mesh, now, expires_at)?;
        // Durable BEFORE release: on failure the grant is dropped and the
        // in-memory table is not advanced.
        let prev = std::mem::replace(&mut inner.store.slots, work);
        if let Err(e) = inner.store.persist_slots() {
            inner.store.slots = prev;
            return Err(ApiError::new(503, "persist_failed", e.to_string()));
        }
        drop(inner);
        self.release(&signed);
        self.log(&format!(
            "checkout granted cog={} version={} arch={} seq={} bytes_fetched={}",
            body.cog_id, entry.version, body.arch, work_seq(&signed), bytes.len()
        ));
        let g = weft_licence_wire::verify_grant_signature(&signed, &self.key.as_ref().expect("key").verifying_key().to_bytes())
            .map_err(|e| ApiError::new(500, "sign_failed", e.to_string()))?;
        Ok(grant_response(&signed, &g))
    }

    pub(crate) fn renew(&self, req: &Request, now: u64) -> Response {
        match self.renew_inner(req, now) {
            Ok(v) => Response::json(200, v),
            Err(e) => Response::err(e),
        }
    }

    fn renew_inner(&self, req: &Request, now: u64) -> Result<Value, ApiError> {
        let body: RenewReq = if req.body.is_empty() {
            RenewReq::default()
        } else {
            serde_json::from_slice(&req.body).map_err(|e| ApiError::new(400, "bad_request", e.to_string()))?
        };
        let mut inner = self.lock();
        let mesh = mesh_of(&inner).ok_or_else(|| ApiError::new(409, "seed_not_bound", "no mesh binding"))?;
        let mut work = inner.store.slots.clone();
        let release: HashSet<String> = body.release.iter().map(|r| slot_key(&r.cog_id, &r.version)).collect();
        let mut keys: Vec<(u64, String)> = work
            .slots
            .iter()
            .filter(|(k, s)| s.status == SlotStatus::Active || release.contains(*k))
            .map(|(k, s)| (s.issue_ctr, k.clone()))
            .collect();
        keys.sort();
        keys.truncate(self.cfg.limits.renew_batch);
        let mut out: Vec<SignedGrant> = Vec::new();
        for (_, key) in keys {
            let slot = work.slots.get_mut(&key).expect("listed");
            let mut withdraw = release.contains(&key);
            let mut expires_at = now;
            if !withdraw {
                match self.licence.entitlement(&slot.cog_id, &mesh.to_hex(), now) {
                    Ok(ent) => {
                        slot.licence = licence_ref(&ent, now);
                        expires_at = (now + self.cfg.grant_ttl_secs).min(slot.licence.expires);
                    }
                    Err(LicenceCheckError::Unreadable(_)) => continue, // retried at the next pull
                    Err(_) => {
                        slot.status = SlotStatus::Lapsed;
                        withdraw = true;
                    }
                }
            }
            if withdraw && slot.status == SlotStatus::Active {
                slot.status = SlotStatus::Released;
            }
            slot.seq += 1;
            out.push(sign_slot(self, &mut work, &key, &mesh, now, expires_at)?);
        }
        let prev = std::mem::replace(&mut inner.store.slots, work);
        if let Err(e) = inner.store.persist_slots() {
            inner.store.slots = prev;
            return Err(ApiError::new(503, "persist_failed", e.to_string()));
        }
        let next = inner.store.slots.ctr;
        drop(inner);
        for g in &out {
            self.release(g);
        }
        self.log(&format!("renew grants={}", out.len()));
        Ok(json!({"grants": out, "next": next}))
    }
}

fn work_seq(g: &SignedGrant) -> u64 {
    serde_json::from_str::<CheckoutGrant>(&g.payload).map(|g| g.seq).unwrap_or(0)
}
