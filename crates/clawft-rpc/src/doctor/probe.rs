//! Side-effect-free probes: `--version` capture, version parsing, hashing.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Parsed `--version` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfo {
    /// Semver-ish token, e.g. `0.8.1`.
    pub version: String,
    /// Parenthesised build stamp, e.g. `2cd752e1-dirty 2026-09-28T14:25Z`.
    pub build: Option<String>,
    /// Build stamp carries `-dirty`.
    pub dirty: bool,
}

/// Parse `weft 0.8.0 (dec3b28f-dirty 2026-07-31T23:09:07Z)` or `weftos 0.8.1`.
pub fn parse_version(text: &str) -> Option<VersionInfo> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let token = line
        .split_whitespace()
        .map(|t| t.trim_start_matches('v'))
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains('.'))?;
    let build = line
        .find('(')
        .and_then(|s| line[s + 1..].find(')').map(|e| line[s + 1..s + 1 + e].to_string()));
    Some(VersionInfo {
        version: token.to_string(),
        dirty: line.contains("-dirty"),
        build,
    })
}

/// Semver precedence value: `major.minor.patch` plus optional prerelease.
/// Build metadata, `-dirty` and git-describe suffixes are dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Semver {
    /// `[major, minor, patch]`.
    pub core: [u64; 3],
    /// Dot-separated prerelease identifiers (`rc1` -> `["rc1"]`); empty for a release.
    pub pre: Vec<String>,
}

/// Parse a version token into [`Semver`]; `None` when unparseable.
pub fn parse_semver(v: &str) -> Option<Semver> {
    let v = v.trim().trim_start_matches('v');
    let v = v.split('+').next()?;
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let nums: Vec<u64> = core.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<_>>()?;
    if nums.len() < 2 || nums.len() > 3 {
        return None;
    }
    let mut c = [0u64; 3];
    c[..nums.len()].copy_from_slice(&nums);
    let mut segs: Vec<&str> = pre.map(|p| p.split('-').collect()).unwrap_or_default();
    segs.retain(|s| *s != "dirty");
    // git describe: `<n>-g<hash>` at the end is not a prerelease.
    if segs.len() >= 2
        && segs[segs.len() - 2].chars().all(|c| c.is_ascii_digit())
        && segs[segs.len() - 1].starts_with('g')
    {
        segs.truncate(segs.len() - 2);
    }
    let pre = segs.join("-").split('.').filter(|s| !s.is_empty()).map(str::to_owned).collect();
    Some(Semver { core: c, pre })
}

impl Ord for Semver {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        self.core.cmp(&o.core).then_with(|| match (self.pre.is_empty(), o.pre.is_empty()) {
            (true, true) => Equal,
            (true, false) => Greater, // release > prerelease
            (false, true) => Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&o.pre) {
                    let ord = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        (Ok(_), Err(_)) => Less,
                        (Err(_), Ok(_)) => Greater,
                        (Err(_), Err(_)) => a.cmp(b),
                    };
                    if ord != Equal {
                        return ord;
                    }
                }
                self.pre.len().cmp(&o.pre.len())
            }
        })
    }
}

impl PartialOrd for Semver {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

/// Numeric components of a version, for ordering (`0.8.1` -> `[0, 8, 1]`).
pub fn version_key(v: &str) -> Vec<u64> {
    v.split(['.', '-', '+'])
        .map_while(|p| p.parse::<u64>().ok())
        .collect()
}

/// Run `cmd args` with a timeout and no stdin; stdout on success.
///
/// This EXECUTES `cmd`. `--version` is only side-effect free for our real
/// binaries; a same-named script earlier on `PATH` would run arbitrary code.
/// Callers therefore go through [`probe_binary`], which refuses files that
/// are not native executables unless the env opts in (tests). The update
/// nag is disabled for the child.
pub fn run_capture(cmd: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(cmd)
        .args(args)
        .env("WEFTOS_NO_UPDATE_CHECK", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait().ok()? {
            Some(s) => break s,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

/// Probe a binary's version: `--version`, then the `version` subcommand
/// (`weftos` has no `--version` flag).
pub fn probe_version(path: &Path, timeout: Duration) -> Option<VersionInfo> {
    run_capture(path, &["--version"], timeout)
        .or_else(|| run_capture(path, &["version"], timeout))
        .and_then(|t| parse_version(&t))
}

/// True when the file starts with a Mach-O, ELF or PE magic number.
pub fn is_native_executable(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    if f.read_exact(&mut magic).is_err() {
        return false;
    }
    matches!(
        magic,
        [0x7f, b'E', b'L', b'F']
            | [0xfe, 0xed, 0xfa, 0xce | 0xcf]
            | [0xce | 0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
    ) || magic[..2] == *b"MZ"
}

/// Probe a binary's version, but only run native executables.
///
/// Returns `(version, note)`. `note` explains why nothing ran: a script is
/// never executed unless `allow_scripts` is set (unit tests use scripts).
pub fn probe_binary(path: &Path, timeout: Duration, allow_scripts: bool) -> (Option<VersionInfo>, Option<String>) {
    if !allow_scripts && !is_native_executable(path) {
        return (None, Some("not a native executable (script?); not run".into()));
    }
    (probe_version(path, timeout), None)
}

/// Hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dirty_stamp() {
        let v = parse_version("weft 0.8.0 (dec3b28f-dirty 2026-07-31T23:09:07Z)\n").unwrap();
        assert_eq!(v.version, "0.8.0");
        assert!(v.dirty);
        assert_eq!(v.build.as_deref(), Some("dec3b28f-dirty 2026-07-31T23:09:07Z"));
    }

    #[test]
    fn parses_plain_and_bare() {
        let v = parse_version("weftos 0.8.1").unwrap();
        assert_eq!((v.version.as_str(), v.dirty, v.build), ("0.8.1", false, None));
        let v = parse_version("0.8.1 (2cd752e1 2026-09-28T14:25Z)").unwrap();
        assert_eq!(v.version, "0.8.1");
        assert!(parse_version("no version here").is_none());
    }

    #[test]
    fn scripts_are_not_native_and_are_not_run() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("weaver");
        std::fs::write(&p, "#!/bin/sh\ntouch ran\n").unwrap();
        assert!(!is_native_executable(&p));
        let (v, note) = probe_binary(&p, Duration::from_secs(2), false);
        assert!(v.is_none() && note.is_some());
        let e = d.path().join("elf");
        std::fs::write(&e, [0x7f, b'E', b'L', b'F', 0]).unwrap();
        assert!(is_native_executable(&e));
    }

    #[test]
    fn semver_precedence() {
        let p = |s| parse_semver(s).unwrap();
        assert!(p("0.8.1") > p("0.8.1-rc1"), "release beats prerelease");
        assert!(p("0.8.1-rc2") > p("0.8.1-rc1"));
        assert!(p("0.8.1-rc1") > p("0.8.0"));
        assert!(p("0.10.0") > p("0.9.9"));
        // -dirty, git-describe and build metadata do not count as newer.
        assert_eq!(p("0.8.1-dirty"), p("0.8.1"));
        assert_eq!(p("0.8.1+abc"), p("0.8.1"));
        assert_eq!(p("0.8.1-3-gdeadbee-dirty"), p("0.8.1"));
        assert_eq!(p("0.8.1-rc1-dirty"), p("0.8.1-rc1"));
        assert!(parse_semver("unknown").is_none() && parse_semver("1").is_none() && parse_semver("a.b.c").is_none());
    }

    #[test]
    fn version_ordering() {
        assert!(version_key("0.8.1") > version_key("0.8.0"));
        assert!(version_key("0.10.0") > version_key("0.9.9"));
    }

    #[test]
    fn sha_of_known_content() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
