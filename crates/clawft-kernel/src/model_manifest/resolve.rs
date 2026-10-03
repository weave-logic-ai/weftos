//! Trust, resolution and consistency for [`ModelRegistry`] entries: what a
//! caller may rely on when `resolve` returns, the lazy-check window, and the
//! structural checks that guard a tampered registry file.

use super::adopt::{FileRole, allowed_roots, contained};
use super::body::{ModelError, ModelPackageBody, verify_model};
use super::check::{CheckMode, ModelState};
use super::registry::{LocalFile, ModelEntry, ModelRegistry, ModelTrust, ResolvedModel};
use crate::workload_pkg::codec::hex_encode;

impl ModelRegistry {
    /// Hide or advertise a model in node facts (local, unsigned opt-out:
    /// shard hashes fingerprint which models this node holds).
    pub fn set_advertised(&self, id_or_name: &str, advertised: bool) -> Result<(), ModelError> {
        let mut g = self.lock();
        let id = Self::find(&g, id_or_name)?;
        if let Some(e) = g.models.get_mut(&id) {
            e.hidden = !advertised;
        }
        self.save(&g)
    }

    /// The trust problem with an entry, if any: its signature no longer
    /// verifies against the pinned anchors (key removed, body edited), its
    /// package id is not the registry key, its files disagree with its body,
    /// or the package, a signer or a shard hash is revoked.
    pub(crate) fn trust_problem(&self, id: &str, entry: &ModelEntry, trust: &ModelTrust) -> Option<String> {
        if let Err(why) = consistency(id, entry) {
            return Some(why);
        }
        let verified = match verify_model(&entry.envelope, &trust.anchors) {
            Ok(v) => v,
            Err(e) => return Some(format!("attestation does not verify: {e}")),
        };
        if verified.package_id != id {
            return Some("package id differs from the registry key".into());
        }
        let list = trust.revocations.as_ref()?;
        let signers: Vec<String> = verified
            .signers
            .iter()
            .filter_map(|s| trust.anchors.signers.iter().find(|k| k.key_id == s.key_id))
            .map(|k| hex_encode(&k.public_key))
            .collect();
        let hashes: Vec<String> = verified.body.shard_hashes().map(String::from).collect();
        list.first_revoked(Some(id), signers.iter(), hashes.iter())
            .map(|r| format!("revoked: {r:?}"))
    }

    /// True when a trust is configured and the entry fails it. Without a
    /// configured trust this is false (advertising then relies on the
    /// structural checks and the file checks alone; `resolve` still refuses).
    pub(crate) fn untrusted(&self, id: &str, entry: &ModelEntry) -> bool {
        let trust = self.trust.lock().unwrap_or_else(|p| p.into_inner()).clone();
        match trust {
            Some(t) => self.trust_problem(id, entry, &t).is_some(),
            None => consistency(id, entry).is_err(),
        }
    }

    /// The model for an adapter to load, verified against the registry's
    /// trust (see [`with_trust`](Self::with_trust)); refused with
    /// [`ModelError::NotReady`] when none is set.
    ///
    /// What a caller may rely on when this returns `Ok`:
    /// - the attestation verifies against the pinned anchors now, and the
    ///   package, its signers and its shard hashes are not revoked;
    /// - the registry's file list equals the attested body;
    /// - every file resolves inside the model root (or its HF `blobs/`),
    ///   and the returned paths are those resolved locations;
    /// - the first resolve of a model in this process hashed every byte
    ///   (a full check, which on a 45 GB model takes minutes); later
    ///   resolves re-hash only files whose size or mtime changed.
    ///
    /// The lazy window: after the first resolve, a file replaced with the
    /// same size and the same mtime is not noticed until the next process
    /// start or an explicit [`CheckMode::Full`] check. Adapters that must
    /// close that window call `check(.., CheckMode::Full)` before loading.
    /// Paths are returned, not handles: a file can still change between
    /// this call and the adapter opening it.
    pub fn resolve(&self, id_or_name: &str) -> Result<ResolvedModel, ModelError> {
        let trust = self.trust.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let id = self.get(id_or_name).map(|(id, _)| id)?;
        let Some(trust) = trust else {
            return Err(ModelError::NotReady {
                id,
                reason: "the registry has no trust anchors configured".into(),
            });
        };
        self.resolve_with(id_or_name, &trust)
    }

    /// [`resolve`](Self::resolve) against an explicit trust (the sharing
    /// gate passes the anchors the caller holds right now).
    pub fn resolve_with(&self, id_or_name: &str, trust: &ModelTrust) -> Result<ResolvedModel, ModelError> {
        let (id, entry) = self.get(id_or_name)?;
        let not_ready = |reason: String| ModelError::NotReady { id: id.clone(), reason };
        if let Some(why) = self.trust_problem(&id, &entry, trust) {
            return Err(not_ready(format!("untrusted: {why}")));
        }
        let first = !self
            .fully_verified
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&id);
        let mode = if first { CheckMode::Full } else { CheckMode::Lazy };
        let check = self.check(&id, mode)?;
        let entry = self.get(&id)?.1;
        if let Some(why) = &entry.refused {
            return Err(not_ready(format!("refused: {why}")));
        }
        match &check.state {
            ModelState::Ready => {}
            ModelState::Degraded { reason, .. } => return Err(not_ready(format!("degraded: {reason}"))),
            ModelState::Refused { reason } => return Err(not_ready(format!("refused: {reason}"))),
        }
        if first {
            self.fully_verified
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(id.clone());
        }
        let body = entry.body()?;
        let allowed = allowed_roots(&entry.root);
        let real = |rel: &str| contained(&entry.root.join(rel), &allowed);
        let shards = body
            .shards
            .iter()
            .map(|s| real(&s.path))
            .collect::<Result<Vec<_>, _>>()?;
        let tokenizer = body.tokenizer_path.as_deref().map(real).transpose()?;
        Ok(ResolvedModel {
            package_id: id,
            name: body.name.clone(),
            format: body.format,
            shards,
            root: entry.root.clone(),
            tokenizer,
            body,
        })
    }
}

/// The local file list a body implies: shards, plus tokenizer and template.
pub(crate) fn files_of_body(body: &ModelPackageBody) -> Vec<LocalFile> {
    let mut v: Vec<LocalFile> = body
        .shards
        .iter()
        .map(|s| LocalFile {
            path: s.path.clone(),
            role: FileRole::Shard,
            size: s.size,
            blake3: s.blake3.clone(),
            verified: None,
        })
        .collect();
    for (path, hash, role) in [
        (&body.tokenizer_path, &body.tokenizer_blake3, FileRole::Tokenizer),
        (&body.template_path, &body.template_blake3, FileRole::Template),
    ] {
        if let (Some(p), Some(h)) = (path, hash) {
            v.push(LocalFile {
                path: p.clone(),
                role,
                size: 0,
                blake3: h.clone(),
                verified: None,
            });
        }
    }
    v
}

/// Structural consistency: the entry's key is its envelope's package id and
/// its file list is exactly what the attested body lists (sizes of shards
/// and every hash). Needs no anchors, so it also guards `open`.
pub(crate) fn consistency(id: &str, e: &ModelEntry) -> Result<(), String> {
    let pid = e.envelope.package_id().map_err(|x| x.to_string())?;
    if pid != id {
        return Err("registry key is not the envelope's package id".into());
    }
    let body = e.body().map_err(|x| x.to_string())?;
    body.validate().map_err(|x| x.to_string())?;
    let want = files_of_body(&body);
    if want.len() != e.files.len() {
        return Err("file list differs from the attested body".into());
    }
    for (w, f) in want.iter().zip(&e.files) {
        let size_ok = w.role != FileRole::Shard || w.size == f.size;
        if w.path != f.path || w.role != f.role || w.blake3 != f.blake3 || !size_ok {
            return Err(format!("file {} differs from the attested body", f.path));
        }
    }
    Ok(())
}
