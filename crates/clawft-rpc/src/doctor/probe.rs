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

/// Numeric components of a version, for ordering (`0.8.1` -> `[0, 8, 1]`).
pub fn version_key(v: &str) -> Vec<u64> {
    v.split(['.', '-', '+'])
        .map_while(|p| p.parse::<u64>().ok())
        .collect()
}

/// Run `cmd args` with a timeout and no stdin; stdout on success.
///
/// Only ever used with the fixed side-effect-free probes (`--version`,
/// `version`) on weft/weaver/weftos. The update nag is disabled for the child.
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
