//! The merged revocation / binding view (see the parent module docs).

use std::collections::{HashMap, HashSet};

use clawft_types::project::cert::{ProjectCert, key_id};

use super::{
    IdentityError, JournalRecord, KIND_REGISTER, KIND_REKEY, KIND_REVOKE, KIND_ROTATED, SOURCE,
    verify_cert_historic,
};
use crate::user_key_rotation::UserKeyHistory;
use crate::chain::ChainEvent;

/// Outcome of [`RevocationView::plan_registration`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    /// This key is already certified; reuse the certificate (no new event).
    Existing(Box<ProjectCert>),
    /// First key for the id (or the first after a revoke): certify with
    /// this serial.
    New {
        /// Serial to issue.
        serial: u64,
    },
}

#[derive(Debug, Default, Clone)]
struct ProjectIdentity {
    certs: Vec<ProjectCert>,
    revoked: HashSet<String>,
    /// Keys replaced by a `project.rekey`: revoked as signers, but not
    /// compromised (the owner replaced them on purpose).
    retired: HashSet<String>,
}

/// Which keys of which projects are certified or revoked, merged from the
/// user chain, the identity journal and the certificate files. Built only
/// by [`Self::build`]; there is no way to add an unverified certificate.
#[derive(Debug, Clone)]
pub struct RevocationView {
    trust: UserKeyHistory,
    /// Chain sequence of the `project.register`/`project.rekey` event that
    /// carries each certificate (by signature).
    anchored: HashMap<String, u64>,
    /// Chain sequence of the first `user.key.rotated` event per retired key id.
    rotated_seq: HashMap<String, u64>,
    projects: HashMap<String, ProjectIdentity>,
    /// key id -> the project that first claimed it (certified or revoked).
    key_owner: HashMap<String, String>,
    rejected: usize,
}

impl RevocationView {
    /// Merge `events` (only [`SOURCE`] events count), `journal` records and
    /// `cert_files` into one view. Every certificate must verify against
    /// `user_pubkey` or it is dropped (see [`Self::rejected`]).
    pub fn build(
        user_pubkey: &[u8; 32],
        events: &[ChainEvent],
        journal: &[JournalRecord],
        cert_files: &[ProjectCert],
    ) -> Self {
        Self::build_with(&UserKeyHistory::single(user_pubkey), events, journal, cert_files)
    }

    /// [`Self::build`] under a user-key rotation history: a certificate sealed
    /// by a retired user key counts when it is dated at or before that key's
    /// rotation point (ADR-103 A13); one dated after it is dropped.
    pub fn build_with(
        trust: &UserKeyHistory,
        events: &[ChainEvent],
        journal: &[JournalRecord],
        cert_files: &[ProjectCert],
    ) -> Self {
        let mut v = Self {
            trust: trust.clone(),
            anchored: HashMap::new(),
            rotated_seq: HashMap::new(),
            projects: HashMap::new(),
            key_owner: HashMap::new(),
            rejected: 0,
        };
        v.index_chain(events);
        for rec in journal {
            match rec {
                JournalRecord::Register { cert } => v.ingest_cert(cert),
                JournalRecord::Rekey { old_key_id, cert } => {
                    v.ingest_revoke(&cert.project_id, old_key_id);
                    v.ingest_retire(&cert.project_id, old_key_id);
                    v.ingest_cert(cert);
                }
                JournalRecord::Revoke { project_id, key_id } => v.ingest_revoke(project_id, key_id),
                JournalRecord::Init {} => {}
            }
        }
        for c in cert_files {
            v.ingest_cert(c);
        }
        for e in events {
            v.ingest_event(e);
        }
        v
    }

    /// Record where the chain holds each certificate and each rotation, the
    /// evidence a retired-key certificate needs (see [`Self::verify_historic`]).
    fn index_chain(&mut self, events: &[ChainEvent]) {
        for ev in events.iter().filter(|e| e.source == SOURCE) {
            let Some(p) = ev.payload.as_ref() else { continue };
            match ev.kind.as_str() {
                KIND_REGISTER | KIND_REKEY => {
                    let field = if ev.kind == KIND_REGISTER { "cert" } else { "new_cert" };
                    if let Some(c) = p.get(field).and_then(|c| serde_json::from_value::<ProjectCert>(c.clone()).ok()) {
                        self.anchored.entry(c.sig).or_insert(ev.sequence);
                    }
                }
                k if k == KIND_ROTATED => {
                    if let Some(old) = p.get("record").and_then(|r| r.get("old_key_id")).and_then(|v| v.as_str()) {
                        self.rotated_seq.entry(old.to_owned()).or_insert(ev.sequence);
                    }
                }
                _ => {}
            }
        }
    }

    fn ingest_event(&mut self, ev: &ChainEvent) {
        if ev.source != SOURCE {
            return;
        }
        let Some(p) = ev.payload.as_ref() else { return };
        let cert_of = |field: &str| {
            p.get(field)
                .and_then(|c| serde_json::from_value::<ProjectCert>(c.clone()).ok())
        };
        let text = |field: &str| p.get(field).and_then(|v| v.as_str());
        match ev.kind.as_str() {
            KIND_REGISTER => {
                if let Some(c) = cert_of("cert") {
                    self.ingest_cert(&c);
                }
            }
            KIND_REKEY => {
                if let (Some(id), Some(old)) = (text("project_id"), text("old_key_id")) {
                    self.ingest_revoke(id, old);
                    self.ingest_retire(id, old);
                }
                if let Some(c) = cert_of("new_cert") {
                    self.ingest_cert(&c);
                }
            }
            KIND_REVOKE => {
                if let (Some(id), Some(old)) = (text("project_id"), text("old_key_id")) {
                    self.ingest_revoke(id, old);
                }
            }
            _ => {}
        }
    }

    fn ingest_cert(&mut self, c: &ProjectCert) {
        if let Some(cause) = self.rejection_cause(c) {
            // The view is rebuilt on every identity call: warn once per
            // (project, serial, cause), not on each rebuild.
            static SEEN: std::sync::Mutex<Option<HashSet<(String, u64, String)>>> = std::sync::Mutex::new(None);
            let first = SEEN
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(HashSet::new)
                .insert((c.project_id.clone(), c.serial, cause.clone()));
            if first {
                tracing::warn!(
                    project = %c.project_id,
                    user_key = %c.user_key_id,
                    serial = c.serial,
                    "project certificate rejected: {cause}"
                );
            }
            self.rejected += 1;
            return;
        }
        self.key_owner
            .entry(c.project_key_id.clone())
            .or_insert_with(|| c.project_id.clone());
        let st = self.projects.entry(c.project_id.clone()).or_default();
        if !st.certs.iter().any(|x| x.sig == c.sig) {
            st.certs.push(c.clone());
        }
    }

    /// Why `c` cannot be trusted, or `None` when it can.
    ///
    /// Signature and shape under the user key that sealed `c`. A certificate
    /// sealed by the key in use needs nothing more. One sealed by a RETIRED
    /// key counts only when it is dated at or before the rotation point AND
    /// the user chain proves it: a `project.register`/`project.rekey` event
    /// carrying it has a sequence below the `user.key.rotated` event of that
    /// key. A journal line or certificate file alone, however it is dated,
    /// is not evidence (whoever holds the old private key can backdate; only
    /// the daemon appends to the reserved chain source).
    pub(crate) fn rejection_cause(&self, c: &ProjectCert) -> Option<String> {
        if let Err(e) = verify_cert_historic(c, &self.trust) {
            return Some(format!("it does not verify under the user key history ({e})"));
        }
        if c.user_key_id == key_id(self.trust.current()) {
            return None;
        }
        let anchored = self.anchored.get(&c.sig);
        let rotated = self.rotated_seq.get(&c.user_key_id);
        match (anchored, rotated) {
            (Some(s), Some(r)) if s < r => None,
            _ => Some(format!(
                "it is sealed by retired user key {} but the user chain does not corroborate it \
                 (certificate event at chain seq {anchored:?}, key rotated at chain seq {rotated:?}); \
                 a journal line or certificate file alone is not evidence",
                c.user_key_id
            )),
        }
    }

    fn ingest_revoke(&mut self, project_id: &str, key_id: &str) {
        self.key_owner
            .entry(key_id.to_owned())
            .or_insert_with(|| project_id.to_owned());
        self.projects
            .entry(project_id.to_owned())
            .or_default()
            .revoked
            .insert(key_id.to_owned());
    }

    fn ingest_retire(&mut self, project_id: &str, key_id: &str) {
        self.projects
            .entry(project_id.to_owned())
            .or_default()
            .retired
            .insert(key_id.to_owned());
    }

    /// Every project id the view knows (certified, rekeyed or revoked).
    pub fn all_project_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.projects.keys().cloned().collect();
        v.sort();
        v
    }

    /// Every certificate ever issued for `project_id` whose key was not
    /// revoked for compromise: the current key and keys replaced by a
    /// rekey. A key removed by `project.revoke` is excluded. Statements the
    /// project signed with these keys are history, not forgeries.
    pub fn key_history(&self, project_id: &str) -> Vec<&ProjectCert> {
        self.projects.get(project_id).map_or_else(Vec::new, |st| {
            st.certs
                .iter()
                .filter(|c| {
                    !st.revoked.contains(&c.project_key_id) || st.retired.contains(&c.project_key_id)
                })
                .collect()
        })
    }

    /// Was `key_id` removed by `project.revoke` (not replaced by a rekey)?
    pub fn is_compromised(&self, project_id: &str, key_id: &str) -> bool {
        self.projects
            .get(project_id)
            .is_some_and(|st| st.revoked.contains(key_id) && !st.retired.contains(key_id))
    }

    /// Every certificate ever issued for `project_id`, compromised keys
    /// included. For re-verifying records the daemon itself accepted while
    /// the key was valid.
    pub fn all_certs(&self, project_id: &str) -> Vec<&ProjectCert> {
        self.projects.get(project_id).map_or_else(Vec::new, |st| st.certs.iter().collect())
    }

    /// The state as journal records, for rebuilding a lost journal: every
    /// verified certificate, then every revocation this view knows. A
    /// revocation that only the lost journal remembered cannot be recovered.
    pub fn export_records(&self) -> Vec<JournalRecord> {
        let mut ids: Vec<&String> = self.projects.keys().collect();
        ids.sort();
        let mut out = Vec::new();
        for id in ids {
            let st = &self.projects[id];
            let mut certs: Vec<&ProjectCert> = st.certs.iter().collect();
            certs.sort_by_key(|c| c.serial);
            out.extend(certs.into_iter().map(|c| JournalRecord::Register { cert: c.clone() }));
            let mut rev: Vec<&String> = st.revoked.iter().collect();
            rev.sort();
            out.extend(rev.into_iter().map(|k| JournalRecord::Revoke {
                project_id: id.clone(),
                key_id: k.clone(),
            }));
        }
        out
    }

    /// Certificates dropped because they did not verify against the user key.
    pub fn rejected(&self) -> usize {
        self.rejected
    }

    /// Is this key revoked for this project?
    pub fn is_revoked(&self, project_id: &str, key_id: &str) -> bool {
        self.projects
            .get(project_id)
            .is_some_and(|s| s.revoked.contains(key_id))
    }

    /// True when `project.revoke` revoked a key of this project (a key only
    /// replaced by `project.rekey` does not count).
    pub fn was_revoked(&self, project_id: &str) -> bool {
        self.projects
            .get(project_id)
            .is_some_and(|s| s.revoked.iter().any(|k| !s.retired.contains(k)))
    }

    /// The certificate in force: the highest-serial one whose key is not
    /// revoked.
    pub fn current_cert(&self, project_id: &str) -> Option<&ProjectCert> {
        let st = self.projects.get(project_id)?;
        st.certs
            .iter()
            .filter(|c| !st.revoked.contains(&c.project_key_id))
            .max_by_key(|c| c.serial)
    }

    /// The `key_id` currently certified for `project_id`.
    pub fn bound_key_id(&self, project_id: &str) -> Option<&str> {
        self.current_cert(project_id).map(|c| c.project_key_id.as_str())
    }

    /// Highest serial ever issued for `project_id` (0 when none).
    pub fn last_serial(&self, project_id: &str) -> u64 {
        self.projects
            .get(project_id)
            .and_then(|s| s.certs.iter().map(|c| c.serial).max())
            .unwrap_or(0)
    }

    /// The project that first claimed `key_id`, certified or revoked.
    pub fn key_owner(&self, key_id: &str) -> Option<&str> {
        self.key_owner.get(key_id).map(String::as_str)
    }

    fn refuse_reuse(&self, project_id: &str, kid: &str) -> Result<(), IdentityError> {
        match self.key_owner(kid) {
            Some(o) if o != project_id => Err(IdentityError::KeyReuse {
                key_id: kid.to_owned(),
                other_project: o.to_owned(),
            }),
            _ => Ok(()),
        }
    }

    /// Refuse a certificate whose key was revoked or replaced.
    pub fn check_cert(&self, cert: &ProjectCert) -> Result<(), IdentityError> {
        let id = &cert.project_id;
        if self.is_revoked(id, &cert.project_key_id) {
            return Err(IdentityError::KeyRevoked {
                project_id: id.clone(),
                key_id: cert.project_key_id.clone(),
            });
        }
        match self.bound_key_id(id) {
            Some(b) if b != cert.project_key_id => Err(IdentityError::KeyConflict {
                project_id: id.clone(),
                bound: b.to_owned(),
            }),
            _ => Ok(()),
        }
    }

    /// TOFU decision for `project.register` of `project_pubkey`.
    pub fn plan_registration(
        &self,
        project_id: &str,
        project_pubkey: &[u8; 32],
    ) -> Result<Registration, IdentityError> {
        let kid = key_id(project_pubkey);
        self.refuse_reuse(project_id, &kid)?;
        if self.is_revoked(project_id, &kid) {
            return Err(IdentityError::KeyRevoked {
                project_id: project_id.to_owned(),
                key_id: kid,
            });
        }
        // Review S1: a revoked project never re-enrols, whatever the marker
        // says (a revoke whose marker could not be written must not be
        // undone by the next `ensure_running`). A key replaced by `rekey` is
        // retired, not revoked in this sense.
        if self.current_cert(project_id).is_none() && self.was_revoked(project_id) {
            return Err(IdentityError::ProjectRevoked(project_id.to_owned()));
        }
        match self.current_cert(project_id) {
            Some(c) if c.project_key_id == kid => Ok(Registration::Existing(Box::new(c.clone()))),
            Some(c) => Err(IdentityError::KeyConflict {
                project_id: project_id.to_owned(),
                bound: c.project_key_id.clone(),
            }),
            None => Ok(Registration::New {
                serial: self.last_serial(project_id) + 1,
            }),
        }
    }

    /// Rekey decision: the project must have a certified key, and the new
    /// one must differ from it, never have been revoked and not belong to
    /// another project. Returns the old `key_id` and the next serial.
    pub fn plan_rekey(
        &self,
        project_id: &str,
        new_pubkey: &[u8; 32],
    ) -> Result<(String, u64), IdentityError> {
        let old = self
            .bound_key_id(project_id)
            .ok_or_else(|| IdentityError::NotBound(project_id.to_owned()))?
            .to_owned();
        let new = key_id(new_pubkey);
        self.refuse_reuse(project_id, &new)?;
        if new == old {
            return Err(IdentityError::KeyConflict {
                project_id: project_id.to_owned(),
                bound: old,
            });
        }
        if self.is_revoked(project_id, &new) {
            return Err(IdentityError::KeyRevoked {
                project_id: project_id.to_owned(),
                key_id: new,
            });
        }
        Ok((old, self.last_serial(project_id) + 1))
    }
}

