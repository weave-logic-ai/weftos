//! Checking an adopted model's files against its attestation (stat-only,
//! lazy or full), and the resulting [`ModelState`].

use super::adopt::{hash_file, stamp};
use super::body::ModelError;
use super::registry::ModelEntry;

/// How hard to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckMode {
    /// Existence and size only. Never reads file content: used when
    /// advertising facts.
    Stat,
    /// Re-hash only files whose stamp changed or is missing.
    Lazy,
    /// Re-hash every file.
    Full,
}

/// Result for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOutcome {
    /// Matches the attestation.
    Ok {
        /// True when the content was hashed on this check.
        rehashed: bool,
    },
    /// Not on disk.
    Missing,
    /// Present but differs from the attestation.
    Mismatch {
        /// What was found (a hash, or a byte count for a size mismatch).
        actual: String,
    },
}

/// Usability of a model right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    /// Every file is present and matches.
    Ready,
    /// Files are unavailable but not wrong: a detached drive or deleted
    /// shards. The workload is `Degraded`, not silently broken.
    Degraded {
        /// Why.
        reason: String,
        /// True when the whole root is gone (an unmounted drive).
        detached: bool,
    },
    /// A file differs from the attested hash. Never used.
    Refused {
        /// Why.
        reason: String,
    },
}

impl ModelState {
    /// True for [`ModelState::Ready`].
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Outcome of a check.
#[derive(Debug, Clone)]
pub struct ModelCheck {
    /// Package id.
    pub package_id: String,
    /// Model name.
    pub name: String,
    /// Overall state.
    pub state: ModelState,
    /// Per-file outcomes in file order.
    pub files: Vec<(String, FileOutcome)>,
    /// Shard hashes that are present and (at least) size-correct.
    pub present_shards: Vec<String>,
    /// Number of shards the manifest lists.
    pub total_shards: usize,
}

pub(super) type Stamps = Vec<Option<(u64, u64)>>;

pub(super) fn check_entry(
    id: &str,
    entry: &ModelEntry,
    mode: CheckMode,
) -> Result<(ModelCheck, Stamps), ModelError> {
    let body = entry.body()?;
    let mut outcomes = Vec::new();
    let mut stamps = Vec::new();
    for f in &entry.files {
        let abs = entry.root.join(&f.path);
        let (outcome, st) = match stamp(&abs) {
            Err(_) => (FileOutcome::Missing, None),
            Ok((size, _)) if size != f.size => {
                (FileOutcome::Mismatch { actual: format!("{size} bytes") }, None)
            }
            Ok(s) => match mode {
                CheckMode::Stat => (FileOutcome::Ok { rehashed: false }, f.verified),
                CheckMode::Lazy if f.verified == Some(s) => {
                    (FileOutcome::Ok { rehashed: false }, f.verified)
                }
                _ => match hash_file(&abs) {
                    Ok(h) if h == f.blake3 => (FileOutcome::Ok { rehashed: true }, Some(s)),
                    Ok(h) => (FileOutcome::Mismatch { actual: h }, None),
                    Err(_) => (FileOutcome::Missing, None),
                },
            },
        };
        outcomes.push((f.path.clone(), outcome));
        stamps.push(st);
    }
    let mismatch = outcomes
        .iter()
        .find(|(_, o)| matches!(o, FileOutcome::Mismatch { .. }));
    let missing = outcomes.iter().filter(|(_, o)| *o == FileOutcome::Missing).count();
    let state = if let Some((p, o)) = mismatch {
        let FileOutcome::Mismatch { actual } = o else { unreachable!() };
        ModelState::Refused { reason: format!("{p}: expected hash or size differs (found {actual})") }
    } else if !entry.root.exists() {
        ModelState::Degraded {
            reason: "model store is detached (root directory not present)".into(),
            detached: true,
        }
    } else if missing > 0 {
        ModelState::Degraded { reason: format!("{missing} file(s) missing"), detached: false }
    } else if let Some(why) = &entry.refused {
        // Cheap modes cannot clear a refusal; only a passing full check does.
        if mode == CheckMode::Full {
            ModelState::Ready
        } else {
            ModelState::Refused { reason: why.clone() }
        }
    } else {
        ModelState::Ready
    };
    let present_shards = body
        .shards
        .iter()
        .filter(|s| {
            outcomes
                .iter()
                .any(|(p, o)| *p == s.path && matches!(o, FileOutcome::Ok { .. }))
        })
        .map(|s| s.blake3.clone())
        .collect();
    let check = ModelCheck {
        package_id: id.to_string(),
        name: body.name.clone(),
        state,
        files: outcomes,
        present_shards,
        total_shards: body.shards.len(),
    };
    Ok((check, stamps))
}
