//! `install` checks: every copy of weft, weaver and weftos.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::channel::{remedy_for, Channel, ChannelKind, Sources};
use super::env::DoctorEnv;
use super::probe::{probe_version, sha256_file, version_key};
use super::{Component, Finding, Severity};

/// Binaries doctor inventories.
pub const NAMES: [&str; 3] = ["weft", "weaver", "weftos"];

/// One binary found on disk.
#[derive(Debug, Clone, Serialize)]
pub struct BinCopy {
    /// Binary name.
    pub name: String,
    /// Path as found (not symlink-resolved).
    pub path: PathBuf,
    /// Symlink-resolved path (identity of the file).
    pub canonical: PathBuf,
    /// Directory is on `PATH`.
    pub on_path: bool,
    /// Position of its directory on `PATH` (lower wins).
    pub path_rank: Option<usize>,
    /// Reported version, e.g. `0.8.1`.
    pub version: Option<String>,
    /// Build stamp inside the parentheses of `--version`.
    pub build: Option<String>,
    /// Build stamp carries `-dirty`.
    pub dirty: bool,
    /// Hex SHA-256 of the file.
    pub sha256: Option<String>,
    /// Owning channel.
    pub channel: Channel,
    /// First on `PATH` for its name.
    pub winner: bool,
    /// Owner uid differs from the home dir owner (removal needs sudo).
    pub foreign_owner: bool,
}

impl BinCopy {
    /// `0.8.1`, `0.8.0-dirty`, or `unknown`.
    pub fn display_version(&self) -> String {
        match (&self.version, self.dirty) {
            (Some(v), true) => format!("{v}-dirty"),
            (Some(v), false) => v.clone(),
            (None, _) => "unknown".into(),
        }
    }

    fn label(&self) -> String {
        format!("{} {} [{}]", self.path.display(), self.display_version(), channel_label(&self.channel))
    }
}

fn channel_label(c: &Channel) -> String {
    let kind = match c.kind {
        ChannelKind::Homebrew => "homebrew",
        ChannelKind::CargoDist => "cargo-dist",
        ChannelKind::DevBuild => "build.sh",
        ChannelKind::CargoInstall => "cargo-install",
        ChannelKind::Unknown => "unknown channel",
    };
    match &c.detail {
        Some(d) if c.kind != ChannelKind::Unknown => format!("{kind}: {d}"),
        _ => kind.to_string(),
    }
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

#[cfg(unix)]
fn uid_of(p: &Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| m.uid())
}

#[cfg(not(unix))]
fn uid_of(_: &Path) -> Option<u32> {
    None
}

/// Find, probe and classify every copy. Ordered by name, then `PATH` rank.
pub fn scan(env: &DoctorEnv) -> Vec<BinCopy> {
    let sources = Sources::load(env);
    let home_uid = uid_of(&env.home);
    let mut out = Vec::new();
    for name in NAMES {
        let mut seen: Vec<PathBuf> = Vec::new();
        let mut copies: Vec<BinCopy> = Vec::new();
        for dir in env.scan_dirs() {
            let path = dir.join(name);
            if !is_executable(&path) {
                continue;
            }
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.contains(&canonical) {
                continue;
            }
            seen.push(canonical.clone());
            let ver = probe_version(&path, env.probe_timeout);
            let dirty = ver.as_ref().is_some_and(|v| v.dirty);
            let path_rank = env.path_dirs.iter().position(|d| *d == dir);
            copies.push(BinCopy {
                name: name.to_string(),
                channel: sources.detect(&path, &canonical, name, dirty),
                sha256: sha256_file(&canonical),
                version: ver.as_ref().map(|v| v.version.clone()),
                build: ver.and_then(|v| v.build),
                dirty,
                on_path: path_rank.is_some(),
                path_rank,
                winner: false,
                foreign_owner: home_uid.is_some() && uid_of(&canonical) != home_uid,
                canonical,
                path,
            });
        }
        if let Some(w) = copies.iter_mut().filter(|c| c.on_path).min_by_key(|c| c.path_rank) {
            w.winner = true;
        }
        copies.sort_by_key(|c| (c.path_rank.is_none(), c.path_rank));
        out.extend(copies);
    }
    out
}

/// True when `other` is a better build than `winner`.
fn is_newer(other: &BinCopy, winner: &BinCopy) -> bool {
    let (a, b) = (
        version_key(other.version.as_deref().unwrap_or("")),
        version_key(winner.version.as_deref().unwrap_or("")),
    );
    a > b || (a == b && winner.dirty && !other.dirty)
}

/// Turn the inventory into findings.
pub fn findings(copies: &[BinCopy]) -> Vec<Finding> {
    let mut out = Vec::new();
    for name in NAMES {
        let group: Vec<&BinCopy> = copies.iter().filter(|c| c.name == name).collect();
        out.extend(name_findings(name, &group));
    }
    out
}

fn name_findings(name: &str, group: &[&BinCopy]) -> Vec<Finding> {
    let c = Component::Install;
    let installer = remedy_for(&Channel { kind: ChannelKind::CargoDist, detail: Some("weftos".into()) }, name);
    if group.is_empty() {
        let sev = if name == "weftos" { Severity::Warn } else { Severity::Fail };
        return vec![Finding::new(c, format!("winner:{name}"), sev, format!("{name} not found on PATH or in standard install dirs"))
            .remedy(installer)];
    }
    let Some(winner) = group.iter().find(|x| x.winner) else {
        return vec![Finding::new(
            c,
            format!("winner:{name}"),
            Severity::Warn,
            format!("{name} exists ({}) but no directory holding it is on PATH", group[0].path.display()),
        )
        .remedy(format!("add {} to PATH", group[0].path.parent().map(|p| p.display().to_string()).unwrap_or_default()))];
    };

    let mut out = Vec::new();
    let newer: Vec<&&BinCopy> = group.iter().filter(|x| !x.winner && is_newer(x, winner)).collect();
    let (sev, msg, remedy) = if winner.version.is_none() {
        (Severity::Fail, format!("{} did not report a version (does it run?)", winner.path.display()), Some(remedy_for(&winner.channel, name)))
    } else if let Some(n) = newer.first() {
        (
            Severity::Warn,
            format!("{} wins on PATH but shadowed copy {} is newer", winner.label(), n.label()),
            Some(remedy_for(&winner.channel, name)),
        )
    } else if winner.dirty {
        (
            Severity::Warn,
            format!("{} wins on PATH and is a dirty dev build", winner.label()),
            Some(remedy_for(&winner.channel, name)),
        )
    } else {
        (Severity::Ok, format!("{} wins on PATH", winner.label()), None)
    };
    let mut f = Finding::new(c, format!("winner:{name}"), sev, msg);
    f.remedy = remedy;
    out.push(f);

    if group.len() > 1 {
        let same = group.iter().all(|x| x.sha256.is_some() && x.sha256 == group[0].sha256);
        let list = group.iter().map(|x| x.label()).collect::<Vec<_>>().join("; ");
        let victim = group.iter().rev().find(|x| !x.winner).copied().unwrap_or(group[0]);
        let rm = format!(
            "{}rm {}   (keep {})",
            if victim.foreign_owner { "sudo " } else { "" },
            victim.path.display(),
            winner.path.display()
        );
        out.push(
            Finding::new(
                c,
                format!("duplicates:{name}"),
                Severity::Warn,
                format!("{} copies of {name}{}: {list}", group.len(), if same { " (identical bytes)" } else { "" }),
            )
            .remedy(rm),
        );
    }
    out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::doctor::env::test_env;
    use std::os::unix::fs::PermissionsExt;

    fn fake_bin(dir: &Path, name: &str, out: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\necho '{out}'\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn env_with(root: &Path, path: &[&Path]) -> DoctorEnv {
        let mut env = test_env(root);
        env.path_dirs = path.iter().map(|p| p.to_path_buf()).collect();
        env
    }

    #[test]
    fn path_order_picks_winner_and_flags_newer_shadowed() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        fake_bin(&a, "weft", "weft 0.8.0 (dec3b28f-dirty 2026-07-31T23:09:07Z)");
        fake_bin(&b, "weft", "weft 0.8.1 (2cd752e1 2026-09-28T14:25Z)");
        let env = env_with(d.path(), &[&a, &b]);
        let copies = scan(&env);
        let weft: Vec<_> = copies.iter().filter(|c| c.name == "weft").collect();
        assert_eq!(weft.len(), 2);
        assert!(weft[0].winner && weft[0].path.starts_with(&a) && weft[0].dirty);
        assert_eq!(weft[0].display_version(), "0.8.0-dirty");
        assert_eq!(weft[0].channel.kind, ChannelKind::DevBuild);
        let f = findings(&copies);
        let w = f.iter().find(|x| x.id == "winner:weft").unwrap();
        assert_eq!(w.severity, Severity::Warn);
        assert!(w.message.contains("newer"), "{}", w.message);
        assert!(f.iter().any(|x| x.id == "duplicates:weft" && x.severity == Severity::Warn));
    }

    #[test]
    fn clean_single_copy_passes_and_missing_weaver_fails() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        fake_bin(&a, "weft", "weft 0.8.1 (2cd752e1 2026-09-28T14:25Z)");
        let env = env_with(d.path(), &[&a]);
        let f = findings(&scan(&env));
        assert_eq!(f.iter().find(|x| x.id == "winner:weft").unwrap().severity, Severity::Ok);
        assert_eq!(f.iter().find(|x| x.id == "winner:weaver").unwrap().severity, Severity::Fail);
        assert_eq!(f.iter().find(|x| x.id == "winner:weftos").unwrap().severity, Severity::Warn);
    }

    #[test]
    fn identical_duplicates_and_version_subcommand_fallback() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        // weftos has no --version flag: only the `version` subcommand works.
        for dir in [&a, &b] {
            std::fs::create_dir_all(dir).unwrap();
            let p = dir.join("weftos");
            std::fs::write(&p, "#!/bin/sh\n[ \"$1\" = version ] && echo 'weftos 0.8.1' || exit 2\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let env = env_with(d.path(), &[&a, &b]);
        let copies = scan(&env);
        assert!(copies.iter().filter(|c| c.name == "weftos").all(|c| c.version.as_deref() == Some("0.8.1")));
        let f = findings(&copies);
        let dup = f.iter().find(|x| x.id == "duplicates:weftos").unwrap();
        assert!(dup.message.contains("identical bytes"));
        assert!(dup.remedy.as_deref().unwrap().contains("rm "));
    }

    #[test]
    fn symlink_to_same_file_is_not_a_duplicate() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        let real = fake_bin(&a, "weft", "weft 0.8.1 (2cd752e1 2026-09-28T14:25Z)");
        std::fs::create_dir_all(&b).unwrap();
        std::os::unix::fs::symlink(&real, b.join("weft")).unwrap();
        let env = env_with(d.path(), &[&b, &a]);
        let copies = scan(&env);
        assert_eq!(copies.iter().filter(|c| c.name == "weft").count(), 1);
    }

    #[test]
    fn off_path_only_copy_warns() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        fake_bin(&a, "weft", "weft 0.8.1 (x)");
        let mut env = env_with(d.path(), &[]);
        env.extra_bin_dirs = vec![a];
        let f = findings(&scan(&env));
        assert_eq!(f.iter().find(|x| x.id == "winner:weft").unwrap().severity, Severity::Warn);
    }
}
