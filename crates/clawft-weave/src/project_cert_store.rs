//! The merged identity view and journal repair (see the parent module docs).

use std::path::Path;

use clawft_kernel::project_identity::{IdentityJournal, JournalLock, JournalRecord, RevocationView, SOURCE};
use clawft_types::project::cert::ProjectCert;
use serde_json::{Value, json};

use super::{CertEnv, IssueError, user_history};

/// The `<id>.cert.json` files. A missing directory is empty; any other
/// directory or file read error fails closed instead of hiding a binding.
/// Files that read but do not parse are skipped (they can only seed a
/// binding the journal also holds).
pub(super) fn read_cert_files(dir: &Path) -> Result<Vec<ProjectCert>, IssueError> {
    let io = |e: std::io::Error| IssueError::Store(format!("{}: {e}", dir.display()));
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(e)),
    };
    let mut out = Vec::new();
    for entry in rd {
        let entry = entry.map_err(io)?;
        if !entry.file_name().to_string_lossy().ends_with(".cert.json") {
            continue;
        }
        // Only regular files are certificate files; a directory or symlink
        // with that name is skipped, real I/O errors still fail closed.
        if !entry.metadata().map_err(io)?.is_file() {
            continue;
        }
        let bytes = std::fs::read(entry.path())
            .map_err(|e| IssueError::Store(format!("{}: {e}", entry.path().display())))?;
        if let Ok(c) = serde_json::from_slice(&bytes) {
            out.push(c);
        }
    }
    Ok(out)
}

fn has_chain_evidence(env: &CertEnv) -> bool {
    env.chain.tail_from(0).iter().any(|e| e.source == SOURCE)
}

fn view_from(env: &CertEnv, journal: Vec<JournalRecord>, certs: &[ProjectCert]) -> Result<RevocationView, IssueError> {
    let trust = user_history(env)?;
    Ok(RevocationView::build_with(&trust, &env.chain.tail_from(0), &journal, certs))
}

/// The one view every issue, rekey, revoke and verification decision uses:
/// journal, certificate files and chain merged, every certificate
/// re-verified against the user key. Takes a shared journal lock. Errors
/// (instead of returning a weaker view) when the journal cannot be trusted,
/// including when it is missing although a certificate file or a
/// `user.projects` chain event shows it should exist.
///
/// Never call this while holding a `JournalLock`: `flock` conflicts between
/// separate opens even in one process, so it would block forever. Holders
/// use `current_view_locked`.
pub fn current_view(env: &CertEnv) -> Result<RevocationView, IssueError> {
    let certs = read_cert_files(&env.manifests_dir)?;
    let evidence = !certs.is_empty() || has_chain_evidence(env);
    let journal = IdentityJournal::new(&env.manifests_dir).read(evidence)?;
    view_from(env, journal, &certs)
}

/// [`current_view`] for a caller holding the exclusive journal lock.
pub(super) fn current_view_locked(env: &CertEnv, lock: &JournalLock) -> Result<RevocationView, IssueError> {
    let certs = read_cert_files(&env.manifests_dir)?;
    let evidence = !certs.is_empty() || has_chain_evidence(env);
    let journal = IdentityJournal::new(&env.manifests_dir).read_locked(lock, evidence)?;
    view_from(env, journal, &certs)
}

/// `project.identity.repair`: rebuild a corrupt or missing journal from the
/// verified certificate files and the chain. The old file is kept as
/// `identity.journal.jsonl.corrupt-<secs>`. Refuses when the journal is
/// healthy. Lines of the old journal that still parse are merged in (their
/// certificates are re-verified). Revocations that only a lost or unparseable
/// part of the journal remembered cannot be recovered: re-check `project.cert.show` for every project afterwards and
/// re-run `project.revoke` where needed.
pub fn repair(env: &CertEnv) -> Result<Value, IssueError> {
    let journal = IdentityJournal::new(&env.manifests_dir);
    let lock = journal.lock()?;
    let certs = read_cert_files(&env.manifests_dir)?;
    let evidence = !certs.is_empty() || has_chain_evidence(env);
    if journal.read_locked(&lock, evidence).is_ok() {
        return Err(IssueError::Invalid("the identity journal is healthy; nothing to repair".into()));
    }
    // A torn tail must not drop earlier journal-only revocations: merge
    // every line of the old journal that still parses.
    let salvaged = journal.salvage(&lock);
    let view = view_from(env, salvaged, &certs)?;
    let records = view.export_records();
    let moved = journal.replace(&lock, &records)?;
    Ok(json!({
        "records": records.len(),
        "rejected": view.rejected(),
        "moved_aside": moved.map(|p| p.display().to_string()),
    }))
}

