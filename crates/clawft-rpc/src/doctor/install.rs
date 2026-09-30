//! `install` checks: every copy of weft, weaver and weftos.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::channel::{remedy_for, Channel, ChannelKind, Sources};
use super::env::DoctorEnv;
use super::probe::{probe_binary, sha256_file, version_key};
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
    /// Why the binary was not run (for example it is a script).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_note: Option<String>,
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
            let (ver, probe_note) = probe_binary(&path, env.probe_timeout, env.probe_scripts);
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
                probe_note,
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

/// Channel rank: managed installers beat `cargo install`, which beats dev
/// builds, which beat copies nothing owns.
fn channel_rank(k: ChannelKind) -> u8 {
    match k {
        ChannelKind::CargoDist | ChannelKind::Homebrew => 3,
        ChannelKind::CargoInstall => 2,
        ChannelKind::DevBuild => 1,
        ChannelKind::Unknown => 0,
    }
}

/// Quality of a copy: newer version, then clean over dirty, then channel.
/// Higher is better; the ranking rule for every duplicate/shadow decision.
fn rank(c: &BinCopy) -> (Vec<u64>, bool, u8) {
    (version_key(c.version.as_deref().unwrap_or("")), !c.dirty, channel_rank(c.channel.kind))
}

/// The copy to keep: highest rank, PATH winner on ties.
fn best<'a>(group: &[&'a BinCopy]) -> &'a BinCopy {
    let mut best = group.iter().find(|x| x.winner).copied().unwrap_or(group[0]);
    for x in group {
        if rank(x) > rank(best) {
            best = x;
        }
    }
    best
}

fn same_bytes(a: &BinCopy, b: &BinCopy) -> bool {
    a.sha256.is_some() && a.sha256 == b.sha256
}

fn rm_cmd(x: &BinCopy) -> String {
    format!("{}rm {}", if x.foreign_owner { "sudo " } else { "" }, x.path.display())
}

fn dir_of(x: &BinCopy) -> String {
    x.path.parent().map(|p| p.display().to_string()).unwrap_or_default()
}

/// One-line remedy for a set of copies. Only ever names an `rm` for a copy
/// that is byte-identical to the best copy or strictly lower ranked; never
/// for the best copy.
fn duplicates_remedy(group: &[&BinCopy]) -> String {
    let keep = best(group);
    let removable: Vec<&&BinCopy> = group
        .iter()
        .filter(|x| x.path != keep.path && (same_bytes(x, keep) || rank(x) < rank(keep)))
        .collect();
    let rms = removable.iter().map(|x| rm_cmd(x)).collect::<Vec<_>>().join("; ");
    let winner = group.iter().find(|x| x.winner).copied();
    match winner {
        Some(w) if w.path != keep.path => {
            if keep.on_path {
                format!(
                    "keep {}: put {} ahead of {} on PATH, or remove the lower-ranked copies: {rms}",
                    keep.path.display(),
                    dir_of(keep),
                    dir_of(w)
                )
            } else {
                format!(
                    "best copy {} is not on PATH: add {} to PATH ahead of {}, or update the winner ({})",
                    keep.path.display(),
                    dir_of(keep),
                    dir_of(w),
                    remedy_for(&w.channel, &w.name)
                )
            }
        }
        _ if removable.is_empty() => {
            "copies differ and none is clearly lower ranked: compare them (weaver doctor --json lists sha256) and remove one by hand".into()
        }
        _ => format!("keep {}: {rms}", keep.path.display()),
    }
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
    let better: Vec<&&BinCopy> = group.iter().filter(|x| !x.winner && rank(x) > rank(winner)).collect();
    let (sev, msg, remedy) = if let (None, Some(note)) = (&winner.version, &winner.probe_note) {
        (Severity::Warn, format!("{} wins on PATH but was not probed: {note}", winner.path.display()), None)
    } else if winner.version.is_none() {
        (Severity::Fail, format!("{} did not report a version (does it run?)", winner.path.display()), Some(remedy_for(&winner.channel, name)))
    } else if let Some(n) = better.first() {
        (
            Severity::Warn,
            format!("{} wins on PATH but shadowed copy {} is newer or better managed", winner.label(), n.label()),
            Some(duplicates_remedy(group)),
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
        out.push(
            Finding::new(
                c,
                format!("duplicates:{name}"),
                Severity::Warn,
                format!("{} copies of {name}{}: {list}", group.len(), if same { " (identical bytes)" } else { "" }),
            )
            .remedy(duplicates_remedy(group)),
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

    fn remedy_of(f: &[Finding], id: &str) -> String {
        f.iter().find(|x| x.id == id).unwrap().remedy.clone().unwrap()
    }

    #[test]
    fn case_a_never_recommends_deleting_the_newer_shadowed_copy() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        let pa = fake_bin(&a, "weft", "weft 0.8.0 (aaaa1111 2026-01-01T00:00Z)");
        let pb = fake_bin(&b, "weft", "weft 0.8.1 (bbbb2222 2026-09-28T14:25Z)");
        let env = env_with(d.path(), &[&a, &b]);
        let f = findings(&scan(&env));
        let r = remedy_of(&f, "duplicates:weft");
        assert!(!r.contains(&format!("rm {}", pb.display())), "{r}");
        assert!(r.contains(&format!("keep {}", pb.display())), "{r}");
        assert!(r.contains("PATH"), "{r}");
        assert!(r.contains(&format!("rm {}", pa.display())), "the lower-ranked winner may be removed: {r}");
        // The winner finding carries a remedy too and is a WARN.
        let w = f.iter().find(|x| x.id == "winner:weft").unwrap();
        assert_eq!(w.severity, Severity::Warn);
        assert!(w.remedy.is_some());
    }

    #[test]
    fn case_b_keeps_the_cargo_dist_copy_and_removes_the_unmanaged_winner() {
        let d = tempfile::tempdir().unwrap();
        let (usr, cargo) = (d.path().join("usr-local-bin"), d.path().join("home/.cargo/bin"));
        let same = "weftos 0.8.1";
        let pu = fake_bin(&usr, "weftos", same);
        let pc = fake_bin(&cargo, "weftos", same);
        let mut env = env_with(d.path(), &[&usr, &cargo]);
        env.cargo_home = d.path().join("home/.cargo");
        std::fs::create_dir_all(env.config_dir.join("weftos")).unwrap();
        std::fs::write(
            env.config_dir.join("weftos/weftos-receipt.json"),
            format!(r#"{{"binaries":["weftos"],"install_prefix":"{}","source":{{"app_name":"weftos"}}}}"#, env.cargo_home.display()),
        )
        .unwrap();
        let copies = scan(&env);
        let cargo_copy = copies.iter().find(|c| c.path == pc).unwrap();
        assert_eq!(cargo_copy.channel.kind, ChannelKind::CargoDist);
        let f = findings(&copies);
        let r = remedy_of(&f, "duplicates:weftos");
        assert!(r.contains(&format!("rm {}", pu.display())), "{r}");
        assert!(!r.contains(&format!("rm {}", pc.display())), "{r}");
        assert!(r.contains(&format!("keep {}", pc.display())), "{r}");
    }

    #[test]
    fn differing_unranked_copies_get_no_rm() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        fake_bin(&a, "weft", "weft 0.8.1 (aaaa1111 2026-01-01T00:00Z)");
        fake_bin(&b, "weft", "weft 0.8.1 (bbbb2222 2026-01-02T00:00Z)");
        let f = findings(&scan(&env_with(d.path(), &[&a, &b])));
        let r = remedy_of(&f, "duplicates:weft");
        assert!(!r.contains("rm /"), "{r}");
    }

    #[test]
    fn all_lower_ranked_copies_are_listed() {
        let d = tempfile::tempdir().unwrap();
        let dirs: Vec<_> = ["a", "b", "c"].iter().map(|n| d.path().join(n)).collect();
        for x in &dirs {
            fake_bin(x, "weftos", "weftos 0.8.1");
        }
        let refs: Vec<&Path> = dirs.iter().map(|p| p.as_path()).collect();
        let f = findings(&scan(&env_with(d.path(), &refs)));
        let r = remedy_of(&f, "duplicates:weftos");
        assert_eq!(r.matches("rm ").count(), 2, "{r}");
    }

    #[test]
    fn scripts_are_not_run_without_the_test_opt_in() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        fake_bin(&a, "weft", "weft 0.8.1 (x)");
        let mut env = env_with(d.path(), &[&a]);
        env.probe_scripts = false;
        let copies = scan(&env);
        let w = copies.iter().find(|c| c.name == "weft").unwrap();
        assert!(w.version.is_none() && w.probe_note.is_some());
        let f = findings(&copies);
        let wf = f.iter().find(|x| x.id == "winner:weft").unwrap();
        assert_eq!(wf.severity, Severity::Warn);
        assert!(wf.message.contains("not probed"));
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
