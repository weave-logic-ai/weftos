//! The merged identity view and journal repair (see the parent module docs).

use std::path::Path;

use clawft_kernel::project_identity::{IdentityJournal, JournalLock, JournalRecord, RevocationView, SOURCE};
use clawft_types::project::cert::ProjectCert;
use serde_json::{Value, json};

use super::{CertEnv, IssueError, user_pubkey};

pub(super) fn read_cert_files(dir: &Path) -> Vec<ProjectCert> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".cert.json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect()
}

fn has_chain_evidence(env: &CertEnv) -> bool {
    env.chain.tail_from(0).iter().any(|e| e.source == SOURCE)
}

fn view_from(env: &CertEnv, journal: Vec<JournalRecord>, certs: &[ProjectCert]) -> RevocationView {
    RevocationView::build(&user_pubkey(env), &env.chain.tail_from(0), &journal, certs)
}

/// The one view every issue, rekey, revoke and verification decision uses:
/// journal, certificate files and chain merged, every certificate
/// re-verified against the user key. Takes a shared journal lock. Errors
/// (instead of returning a weaker view) when the journal cannot be trusted,
/// including when it is missing although a certificate file or a
/// `user.projects` chain event shows it should exist.
pub fn current_view(env: &CertEnv) -> Result<RevocationView, IssueError> {
    let certs = read_cert_files(&env.manifests_dir);
    let evidence = !certs.is_empty() || has_chain_evidence(env);
    let journal = IdentityJournal::new(&env.manifests_dir).read(evidence)?;
    Ok(view_from(env, journal, &certs))
}

/// [`current_view`] for a caller holding the exclusive journal lock.
pub(super) fn current_view_locked(env: &CertEnv, lock: &JournalLock) -> Result<RevocationView, IssueError> {
    let certs = read_cert_files(&env.manifests_dir);
    let evidence = !certs.is_empty() || has_chain_evidence(env);
    let journal = IdentityJournal::new(&env.manifests_dir).read_locked(lock, evidence)?;
    Ok(view_from(env, journal, &certs))
}

/// `project.identity.repair`: rebuild a corrupt or missing journal from the
/// verified certificate files and the chain. The old file is kept as
/// `identity.journal.jsonl.corrupt-<secs>`. Refuses when the journal is
/// healthy. Revocations that only the lost journal remembered cannot be
/// recovered: re-check `project.cert.show` for every project afterwards and
/// re-run `project.revoke` where needed.
pub fn repair(env: &CertEnv) -> Result<Value, IssueError> {
    let journal = IdentityJournal::new(&env.manifests_dir);
    let lock = journal.lock()?;
    let certs = read_cert_files(&env.manifests_dir);
    let evidence = !certs.is_empty() || has_chain_evidence(env);
    if journal.read_locked(&lock, evidence).is_ok() {
        return Err(IssueError::Invalid("the identity journal is healthy; nothing to repair".into()));
    }
    let view = view_from(env, Vec::new(), &certs);
    let records = view.export_records();
    let moved = journal.replace(&lock, &records)?;
    Ok(json!({
        "records": records.len(),
        "rejected": view.rejected(),
        "moved_aside": moved.map(|p| p.display().to_string()),
    }))
}

