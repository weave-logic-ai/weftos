//! Install-method decision and the atomic, all-or-nothing binary swap used by
//! `weaver update`.
//!
//! [`decide`] never writes. It classifies the running `weaver` with the same
//! evidence `weaver doctor install` uses (Homebrew Cellar, cargo-dist receipt,
//! `build.sh` marker, cargo ledger) and either refuses with the owning
//! channel's own command or returns the destinations to replace.
//!
//! [`apply`] replaces each binary with a same-directory `rename`, so another
//! process sees the old or the new file, never a partial one. The old file is
//! kept (hard link, else copy) until every binary has been swapped; any
//! failure restores all of them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use clawft_rpc::doctor::DoctorEnv;
use clawft_rpc::doctor::channel::{Channel, ChannelKind, Sources, remedy_for};
use clawft_rpc::doctor::probe::probe_version;
use serde_json::Value;

use super::update_release::Staged;

/// How this install is managed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    /// cargo-dist receipt at this path.
    Receipt(PathBuf),
    /// No receipt; binaries next to the running `weaver`.
    Unmanaged,
}

/// One binary to replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dest {
    pub name: String,
    pub path: PathBuf,
    /// Version the existing file reports; `None` when absent or unprobeable.
    pub installed: Option<String>,
    /// The file does not exist yet.
    pub is_new: bool,
}

/// What an update would replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub method: Method,
    pub dir: PathBuf,
    pub dests: Vec<Dest>,
    pub notes: Vec<String>,
}

/// Result of [`decide`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Proceed(Plan),
    /// Not ours to update; `command` is what the owner should run.
    Refuse { reason: String, command: String },
}

fn refuse(reason: String, command: String) -> Decision {
    Decision::Refuse { reason, command }
}

fn channel_name(c: &Channel) -> &'static str {
    match c.kind {
        ChannelKind::Homebrew => "Homebrew",
        ChannelKind::CargoDist => "the release installer",
        ChannelKind::DevBuild => "a source build (scripts/build.sh install)",
        ChannelKind::CargoInstall => "cargo install",
        ChannelKind::Unknown => "an unknown channel",
    }
}

struct ReceiptInfo {
    path: PathBuf,
    prefix: PathBuf,
}

fn receipts(env: &DoctorEnv) -> Vec<ReceiptInfo> {
    let mut out = Vec::new();
    let Ok(apps) = std::fs::read_dir(&env.config_dir) else { return out };
    for app in apps.flatten() {
        let Ok(files) = std::fs::read_dir(app.path()) else { continue };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if !(name.contains("receipt") && name.ends_with(".json")) {
                continue;
            }
            let v: Option<Value> = std::fs::read_to_string(f.path()).ok().and_then(|t| serde_json::from_str(&t).ok());
            if let Some(prefix) = v.as_ref().and_then(|v| v["install_prefix"].as_str()) {
                out.push(ReceiptInfo { path: f.path(), prefix: PathBuf::from(prefix) });
            }
        }
    }
    out
}

fn in_prefix(dir: &Path, prefix: &Path) -> bool {
    dir == prefix || dir == prefix.join("bin")
}

/// Decide whether and where to update. `exe` is the running `weaver`;
/// `names` the binaries the release ships; `dirty` marks a `-dirty` build.
pub fn decide(env: &DoctorEnv, exe: &Path, dirty: bool, names: &[String]) -> Decision {
    let exe = crate::service_units::strip_deleted(exe);
    let canon = std::fs::canonicalize(&exe).unwrap_or_else(|_| exe.clone());
    let sources = Sources::load(env);
    let ch = sources.detect(&exe, &canon, "weaver", dirty);
    let Some(dir) = canon.parent().map(Path::to_path_buf) else {
        return refuse(format!("cannot locate the directory of {}", canon.display()), "weaver doctor install".into());
    };
    let method = match ch.kind {
        ChannelKind::Homebrew | ChannelKind::DevBuild | ChannelKind::CargoInstall => {
            return refuse(
                format!("{} is managed by {}; weaver update will not overwrite it", canon.display(), channel_name(&ch)),
                remedy_for(&ch, "weaver"),
            );
        }
        ChannelKind::CargoDist => {
            match receipts(env).into_iter().find(|r| in_prefix(&dir, &r.prefix)) {
                Some(r) => Method::Receipt(r.path),
                None => return refuse("the release receipt for this install cannot be read".into(), "weaver doctor install".into()),
            }
        }
        ChannelKind::Unknown => {
            if let Some(r) = receipts(env).first() {
                return refuse(
                    format!(
                        "{} is not the copy the release receipt {} manages (prefix {})",
                        canon.display(),
                        r.path.display(),
                        r.prefix.display()
                    ),
                    format!("{}/weaver update   (or: weaver doctor install)", r.prefix.display()),
                );
            }
            Method::Unmanaged
        }
    };
    let mut dests = Vec::new();
    let mut notes = Vec::new();
    for name in names {
        let path = dir.join(name);
        let meta = std::fs::symlink_metadata(&path).ok();
        if let Some(m) = &meta {
            if m.file_type().is_symlink() {
                let target = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                let c = sources.detect(&path, &target, name, false);
                return refuse(
                    format!("{} is a symlink to {} ({}); weaver update will not replace it", path.display(), target.display(), channel_name(&c)),
                    remedy_for(&c, name),
                );
            }
            let c = sources.detect(&path, &path, name, false);
            if matches!(c.kind, ChannelKind::Homebrew | ChannelKind::DevBuild | ChannelKind::CargoInstall) {
                return refuse(
                    format!("{} is managed by {}; weaver update will not overwrite it", path.display(), channel_name(&c)),
                    remedy_for(&c, name),
                );
            }
        }
        if meta.is_none() && method == Method::Unmanaged && name != "weaver" {
            notes.push(format!("{name} is not installed in {}; skipping it", dir.display()));
            continue;
        }
        dests.push(Dest {
            name: name.clone(),
            installed: meta.as_ref().and_then(|_| probe_version(&path, Duration::from_secs(5))).map(|v| v.version),
            is_new: meta.is_none(),
            path,
        });
    }
    Decision::Proceed(Plan { method, dir, dests, notes })
}

/// A directory the update can create files in.
pub fn check_writable(dir: &Path) -> anyhow::Result<()> {
    let probe = dir.join(format!(".weaver-update-probe-{}", std::process::id()));
    std::fs::write(&probe, b"").with_context(|| {
        format!("{} is not writable by this user; fix its permissions or re-run `sudo weaver update`", dir.display())
    })?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// One replaced binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub name: String,
    pub path: PathBuf,
    pub from: Option<String>,
}

struct Pending {
    dest: PathBuf,
    new: PathBuf,
    backup: Option<PathBuf>,
}

fn side(dest: &Path, tag: &str) -> PathBuf {
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dest.with_file_name(format!(".{name}.{tag}-{}", std::process::id()))
}

#[cfg(unix)]
fn swap_in(dest: &Path, new: &Path, backup: Option<&Path>) -> std::io::Result<()> {
    if let Some(b) = backup {
        let _ = std::fs::remove_file(b);
        if std::fs::hard_link(dest, b).is_err() {
            std::fs::copy(dest, b)?;
        }
    }
    std::fs::rename(new, dest)
}

#[cfg(not(unix))]
fn swap_in(dest: &Path, new: &Path, backup: Option<&Path>) -> std::io::Result<()> {
    if let Some(b) = backup {
        let _ = std::fs::remove_file(b);
        std::fs::rename(dest, b)?;
        if let Err(e) = std::fs::rename(new, dest) {
            let _ = std::fs::rename(b, dest);
            return Err(e);
        }
        return Ok(());
    }
    std::fs::rename(new, dest)
}

fn rollback(done: &[Pending]) -> Vec<String> {
    let mut failures = Vec::new();
    for p in done.iter().rev() {
        let r = match &p.backup {
            Some(b) => std::fs::rename(b, &p.dest),
            None => std::fs::remove_file(&p.dest),
        };
        if let Err(e) = r {
            failures.push(format!("{}: {e}", p.dest.display()));
        }
    }
    failures
}

/// Replace every destination from `staged`, all or nothing. `fail_at` (tests)
/// injects an I/O error before the n-th swap.
pub fn apply(plan: &Plan, staged: &[Staged], fail_at: Option<usize>) -> anyhow::Result<Vec<Replaced>> {
    check_writable(&plan.dir)?;
    // `weaver` last: if anything earlier fails the running binary was never touched.
    let mut order: Vec<&Dest> = plan.dests.iter().collect();
    order.sort_by_key(|d| d.name == "weaver");
    let mut pending: Vec<Pending> = Vec::new();
    for d in &order {
        let src = staged.iter().find(|s| s.name == d.name).with_context(|| format!("{} was not staged", d.name))?;
        let new = side(&d.path, "new");
        let r = std::fs::copy(&src.path, &new).and_then(|_| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
            }
            std::fs::File::open(&new)?.sync_all()
        });
        if let Err(e) = r {
            let _ = std::fs::remove_file(&new);
            for p in &pending {
                let _ = std::fs::remove_file(&p.new);
            }
            bail!("cannot stage {}: {e}", d.path.display());
        }
        pending.push(Pending { dest: d.path.clone(), new, backup: (!d.is_new).then(|| side(&d.path, "old")) });
    }
    let mut done: Vec<Pending> = Vec::new();
    let mut queue = pending.into_iter().enumerate();
    while let Some((i, p)) = queue.next() {
        let r = if fail_at == Some(i) {
            Err(std::io::Error::other("injected failure"))
        } else {
            swap_in(&p.dest, &p.new, p.backup.as_deref())
        };
        match r {
            Ok(()) => done.push(p),
            Err(e) => {
                let _ = std::fs::remove_file(&p.new);
                if let Some(b) = &p.backup {
                    let _ = std::fs::remove_file(b);
                }
                for (_, rest) in queue.by_ref() {
                    let _ = std::fs::remove_file(&rest.new);
                }
                let failed = rollback(&done);
                if failed.is_empty() {
                    bail!("could not install {}: {e}; every binary was restored to its previous version", p.dest.display());
                }
                bail!(
                    "could not install {}: {e}; ROLLBACK INCOMPLETE, restore these by hand from the .old files: {}",
                    p.dest.display(),
                    failed.join("; ")
                );
            }
        }
    }
    for p in &done {
        if let Some(b) = &p.backup {
            let _ = std::fs::remove_file(b);
        }
    }
    Ok(order
        .iter()
        .map(|d| Replaced { name: d.name.clone(), path: d.path.clone(), from: d.installed.clone() })
        .collect())
}

/// Record the new version in the cargo-dist receipt (atomic; other keys kept).
pub fn update_receipt_version(receipt: &Path, version: &str) -> anyhow::Result<()> {
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(receipt)?)?;
    v["version"] = Value::String(version.into());
    let tmp = side(receipt, "new");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&v)?)?;
    std::fs::rename(&tmp, receipt).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}
