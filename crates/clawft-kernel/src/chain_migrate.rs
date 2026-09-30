//! `weaver migrate user-chain`: copy the legacy `~/.clawft` chain to the
//! Phase 1 user chain location (`~/.weftos/chain`), verified, source untouched.
//!
//! Safety sequence (ADR-103 Phase 1, plan package E):
//!
//! 1. refuse when the destination already holds something that is not this
//!    migration (identical or already-migrated is a no-op);
//! 2. refuse when a kernel holds the source `chain.lock`, or when the
//!    mixed-version guard says a lock-unaware writer touched it recently;
//! 3. take the source `chain.lock` ourselves for the whole run;
//! 4. copy to a temp dir beside the destination, fsync every file;
//! 5. re-hash the source: any change since the plan aborts;
//! 6. load the copy with the kernel's own restore path, check byte hashes,
//!    event count, head hash, integrity and signature against the source;
//! 7. write `MIGRATED_FROM.json`, atomically rename into place, then write
//!    the marker beside the source.
//!
//! Any failure before step 7's rename removes the temp dir. The source chain
//! files are never modified, moved or deleted; the only writes in the source
//! directory are the transient `chain.lock` and the final marker.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use clawft_types::runtime_paths::{
    LEGACY_MIGRATED_MARKER, MIGRATED_FROM_FILE, RuntimePaths,
};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::chain::ChainManager;
use crate::chain_storage::{ChainLock, lock_unaware_writer_age};

/// Chain files, relative to a runtime root, in copy order (key last).
const SET: [&str; 5] = [
    "chain.rvf",
    "chain.json",
    "chain.tree.json",
    "chain/anchors.jsonl",
    "chain.key",
];

/// Why a migration did not happen.
#[derive(Debug)]
pub enum MigrateError {
    /// A safety rule said no; nothing was changed.
    Refused(String),
    /// The source has no chain.
    NothingToMigrate(String),
    /// The copy did not verify; it was removed.
    VerifyFailed(String),
    /// An I/O step failed; partial output was removed.
    Io(String),
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(m) => write!(f, "refused: {m}"),
            Self::NothingToMigrate(m) => write!(f, "nothing to migrate: {m}"),
            Self::VerifyFailed(m) => write!(f, "verification failed (copy removed): {m}"),
            Self::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for MigrateError {}

fn io<T>(ctx: &str, r: std::io::Result<T>) -> Result<T, MigrateError> {
    r.map_err(|e| MigrateError::Io(format!("{ctx}: {e}")))
}

/// Inputs. Paths are injected; the CLI resolves `$HOME`.
pub struct MigrateOptions<'a> {
    /// Legacy runtime root holding `chain.rvf` / `chain.json` (`~/.clawft`).
    pub from: &'a Path,
    /// Destination chain directory (`~/.weftos/chain`).
    pub to: &'a Path,
    /// Plan and verify the source only; write nothing.
    pub dry_run: bool,
    /// Clock for the mixed-version guard.
    pub now: SystemTime,
    /// Recorded in the marker (`weaver` version).
    pub tool_version: &'a str,
}

/// One file in the set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Path relative to the chain root.
    pub name: String,
    /// Size in bytes.
    pub bytes: u64,
    /// Hex SHA-256 of the contents.
    pub sha256: String,
}

/// What restoring the chain yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// Events in the chain.
    pub events: usize,
    /// Head sequence number.
    pub sequence: u64,
    /// Hex head hash.
    pub hash: String,
    /// `verified`, or why the RVF signature could not be checked.
    pub signature: String,
}

/// What a migration would copy, from the source.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Source root.
    pub from: PathBuf,
    /// Destination chain directory.
    pub to: PathBuf,
    /// Files that exist in the source.
    pub files: Vec<FileEntry>,
    /// Source head.
    pub head: Head,
}

/// `MIGRATED_FROM.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    /// Source root.
    pub source: String,
    /// Copied files with hashes.
    pub files: Vec<FileEntry>,
    /// Head hash at migration.
    pub head_hash: String,
    /// Head sequence at migration.
    pub sequence: u64,
    /// Event count at migration.
    pub events: usize,
    /// RVF signature state at migration.
    pub signature: String,
    /// RFC 3339 time.
    pub at: String,
    /// `weaver` version.
    pub weaver_version: String,
}

/// Result of a run.
#[derive(Debug)]
pub enum Outcome {
    /// `--dry-run`: the plan, nothing written.
    DryRun(Plan),
    /// Copied, verified, in place.
    Migrated(Plan),
    /// The destination already holds this migration (or identical bytes).
    AlreadyMigrated(Option<Marker>),
}

fn sha_hex(p: &Path) -> std::io::Result<(u64, String)> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let n = std::io::copy(&mut f, &mut h)?;
    Ok((n, h.finalize().iter().map(|b| format!("{b:02x}")).collect()))
}

fn inventory(root: &Path) -> Result<Vec<FileEntry>, MigrateError> {
    let mut out = Vec::new();
    for name in SET {
        let p = root.join(name);
        if p.is_file() {
            let (bytes, sha256) = io(&format!("hash {}", p.display()), sha_hex(&p))?;
            out.push(FileEntry {
                name: name.into(),
                bytes,
                sha256,
            });
        }
    }
    Ok(out)
}

/// Restore the chain the way boot does (RVF preferred, else JSON), verify
/// integrity and, when both exist, the RVF signature against `chain.key`.
fn load_head(root: &Path) -> Result<Head, MigrateError> {
    let p = RuntimePaths::at(root);
    let rvf = p.chain_rvf();
    let loaded = if rvf.exists() {
        ChainManager::load_from_rvf(&rvf, 1000)
    } else {
        ChainManager::load_from_file(&p.chain_checkpoint(), 1000)
    };
    let mgr = loaded.map_err(|e| {
        MigrateError::VerifyFailed(format!("chain in {} does not restore: {e}", root.display()))
    })?;
    let v = mgr.verify_integrity();
    if !v.valid {
        return Err(MigrateError::VerifyFailed(format!(
            "chain in {} fails integrity ({} errors, first: {})",
            root.display(),
            v.errors.len(),
            v.errors.first().map_or("", String::as_str)
        )));
    }
    Ok(Head {
        events: mgr.len(),
        sequence: mgr.sequence(),
        hash: mgr.head_hash().iter().map(|b| format!("{b:02x}")).collect(),
        signature: signature_state(&p)?,
    })
}

fn signature_state(p: &RuntimePaths) -> Result<String, MigrateError> {
    let (rvf, key) = (p.chain_rvf(), p.chain_key());
    if !rvf.exists() {
        return Ok("no-rvf".into());
    }
    if !key.exists() {
        return Ok("no-key".into());
    }
    let bytes = io("read chain.key", std::fs::read(&key))?;
    let seed: [u8; 32] = bytes.try_into().map_err(|_| {
        MigrateError::VerifyFailed("chain.key is not 32 bytes".into())
    })?;
    let vk = SigningKey::from_bytes(&seed).verifying_key();
    match ChainManager::verify_rvf_signature(&rvf, &vk) {
        Ok(true) => Ok("verified".into()),
        Ok(false) => Err(MigrateError::VerifyFailed(
            "chain.rvf signature does not verify against chain.key".into(),
        )),
        Err(e) => Ok(format!("unverifiable ({e})")),
    }
}

fn check_destination(
    from: &Path,
    to: &Path,
    src: &[FileEntry],
) -> Result<Option<Outcome>, MigrateError> {
    if !to.exists() {
        return Ok(None);
    }
    let marker_path = to.join(MIGRATED_FROM_FILE);
    if marker_path.is_file() {
        let text = io("read marker", std::fs::read_to_string(&marker_path))?;
        let m: Marker = serde_json::from_str(&text).map_err(|e| {
            MigrateError::Refused(format!("{} is unreadable: {e}", marker_path.display()))
        })?;
        let same = m.source == from.to_string_lossy();
        if !same {
            return Err(MigrateError::Refused(format!(
                "{} already holds a chain migrated from {}",
                to.display(),
                m.source
            )));
        }
        if m.files != src {
            return Err(MigrateError::Refused(format!(
                "the source chain changed after it was migrated to {} (something wrote to \
                 {} since); the two chains have diverged. Resolve by hand: nothing was changed",
                to.display(),
                from.display()
            )));
        }
        return Ok(Some(Outcome::AlreadyMigrated(Some(m))));
    }
    let dst = inventory(to)?;
    if dst.is_empty() {
        let empty = io("list destination", std::fs::read_dir(to))?.next().is_none();
        return if empty {
            Ok(None)
        } else {
            Err(MigrateError::Refused(format!(
                "{} exists and is not empty (no chain in it)",
                to.display()
            )))
        };
    }
    if dst == src {
        return Ok(Some(Outcome::AlreadyMigrated(None)));
    }
    Err(MigrateError::Refused(format!(
        "{} already holds a chain that differs from {}; it will not be overwritten",
        to.display(),
        from.display()
    )))
}

fn guards(from: &Path, now: SystemTime) -> Result<(), MigrateError> {
    let ckpt = RuntimePaths::at(from).chain_checkpoint();
    ChainLock::probe(&ckpt).map_err(MigrateError::Refused)?;
    if let Some(age) = lock_unaware_writer_age(&ckpt, now) {
        return Err(MigrateError::Refused(format!(
            "the chain in {} was modified {age}s ago and has no chain.lock: an older kernel \
             may still be writing it. Stop every weaver daemon and retry",
            from.display()
        )));
    }
    Ok(())
}

/// Migrate. See the module docs for the sequence.
pub fn migrate_user_chain(opts: &MigrateOptions<'_>) -> Result<Outcome, MigrateError> {
    migrate_with_hook(opts, &mut |_, _| {})
}

/// [`migrate_user_chain`] with a hook called after the copy, before the
/// source re-check and verification, with `(source, temp copy)`. Tests use it
/// to corrupt the copy or mutate the source.
#[doc(hidden)]
pub fn migrate_with_hook(
    opts: &MigrateOptions<'_>,
    after_copy: &mut dyn FnMut(&Path, &Path),
) -> Result<Outcome, MigrateError> {
    let (from, to) = (opts.from, opts.to);
    let files = inventory(from)?;
    if !files.iter().any(|f| f.name == "chain.rvf" || f.name == "chain.json") {
        return Err(MigrateError::NothingToMigrate(format!(
            "no chain.rvf or chain.json in {}",
            from.display()
        )));
    }
    if let Some(done) = check_destination(from, to, &files)? {
        return Ok(done);
    }
    guards(from, opts.now)?;
    let head = load_head(from)?;
    let plan = Plan {
        from: from.into(),
        to: to.into(),
        files,
        head,
    };
    if opts.dry_run {
        return Ok(Outcome::DryRun(plan));
    }
    execute(opts, plan, after_copy)
}

fn execute(
    opts: &MigrateOptions<'_>,
    plan: Plan,
    after_copy: &mut dyn FnMut(&Path, &Path),
) -> Result<Outcome, MigrateError> {
    let (from, to) = (opts.from, opts.to);
    let ckpt = RuntimePaths::at(from).chain_checkpoint();
    let lock_existed = ChainLock::lock_path(&ckpt).exists();
    let lock = ChainLock::acquire(&ckpt).map_err(MigrateError::Refused)?;
    let result = copy_verify_place(opts, &plan, after_copy);
    drop(lock);
    if !lock_existed {
        // Leave the source as found: our lock must not make a never-locked
        // chain look lock-aware to the first-adoption guard.
        let _ = std::fs::remove_file(ChainLock::lock_path(&ckpt));
    }
    result?;
    write_source_marker(from, to, &plan);
    Ok(Outcome::Migrated(plan))
}

fn copy_verify_place(
    opts: &MigrateOptions<'_>,
    plan: &Plan,
    after_copy: &mut dyn FnMut(&Path, &Path),
) -> Result<(), MigrateError> {
    let (from, to) = (opts.from, opts.to);
    // The lock is ours: re-plan from the locked state.
    if inventory(from)? != plan.files {
        return Err(MigrateError::Refused(
            "the source chain changed while taking the lock; retry".into(),
        ));
    }
    let parent = to.parent().unwrap_or(Path::new("."));
    io("create destination parent", std::fs::create_dir_all(parent))?;
    let name = to.file_name().map_or("chain".into(), |n| n.to_string_lossy());
    let tmp = parent.join(format!(".{name}.migrating-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let r = build_temp(opts, plan, &tmp, after_copy);
    if r.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        return r;
    }
    // An empty pre-existing destination dir is replaced; anything else was
    // refused earlier, and rename fails rather than overwrite.
    let _ = std::fs::remove_dir(to);
    let placed = io("rename into place", std::fs::rename(&tmp, to));
    if placed.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    placed?;
    sync_dir(parent);
    Ok(())
}

fn build_temp(
    opts: &MigrateOptions<'_>,
    plan: &Plan,
    tmp: &Path,
    after_copy: &mut dyn FnMut(&Path, &Path),
) -> Result<(), MigrateError> {
    io("create temp dir", std::fs::create_dir_all(tmp.join("chain")))?;
    for f in &plan.files {
        copy_fsync(&opts.from.join(&f.name), &tmp.join(&f.name), f.name == "chain.key")?;
    }
    sync_dir(&tmp.join("chain"));
    sync_dir(tmp);
    after_copy(opts.from, tmp);
    // Source unchanged since the plan?
    if inventory(opts.from)? != plan.files {
        return Err(MigrateError::Refused(
            "the source chain changed during the copy; nothing was migrated".into(),
        ));
    }
    // Bytes identical, then restore + compare head/integrity/signature.
    if inventory(tmp)? != plan.files {
        return Err(MigrateError::VerifyFailed(
            "copied files differ from the source (hash mismatch)".into(),
        ));
    }
    let copy = load_head(tmp)?;
    if copy != plan.head {
        return Err(MigrateError::VerifyFailed(format!(
            "head mismatch: source {:?}, copy {:?}",
            plan.head, copy
        )));
    }
    let marker = Marker {
        source: opts.from.to_string_lossy().into_owned(),
        files: plan.files.clone(),
        head_hash: plan.head.hash.clone(),
        sequence: plan.head.sequence,
        events: plan.head.events,
        signature: plan.head.signature.clone(),
        at: chrono::Utc::now().to_rfc3339(),
        weaver_version: opts.tool_version.into(),
    };
    let json = serde_json::to_vec_pretty(&marker)
        .map_err(|e| MigrateError::Io(format!("encode marker: {e}")))?;
    let mp = tmp.join(MIGRATED_FROM_FILE);
    let mut mf = io("write marker", std::fs::File::create(&mp))?;
    io("write marker", mf.write_all(&json))?;
    io("sync marker", mf.sync_all())?;
    sync_dir(tmp);
    Ok(())
}

fn copy_fsync(src: &Path, dst: &Path, secret: bool) -> Result<(), MigrateError> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let _ = secret;
    let mut input = io(&format!("open {}", src.display()), std::fs::File::open(src))?;
    let mut out = io(&format!("create {}", dst.display()), o.open(dst))?;
    io("copy", std::io::copy(&mut input, &mut out))?;
    io("fsync", out.sync_all())
}

fn sync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

/// Best effort: the copy is already in place and verified, so a read-only
/// source directory must not fail the migration.
fn write_source_marker(from: &Path, to: &Path, plan: &Plan) {
    let text = format!(
        "This chain was migrated to the WeftOS user chain.\n\
         migrated-to: {}\n\
         head-seq: {}\n\
         head-hash: {}\n\
         migrated-at: {}\n\
         The files here were copied, not moved or changed. Do not start a kernel on this \
         chain: it would fork history from {}. To override knowingly, pass \
         --adopt-legacy-chain or isolate the run with WEFTOS_RUNTIME_DIR.\n",
        to.display(),
        plan.head.sequence,
        plan.head.hash,
        chrono::Utc::now().to_rfc3339(),
        to.display()
    );
    let _ = std::fs::write(from.join(LEGACY_MIGRATED_MARKER), text);
}
