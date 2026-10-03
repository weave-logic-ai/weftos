//! Release discovery, download and verification for `weaver update`.
//!
//! Trust chain, all of it fail-closed:
//! 1. `dist-manifest.json` (cargo-dist) names the release tag and, per target
//!    triple, the archives and the binaries inside. It is the only source of
//!    the release set; nothing is guessed from asset names.
//! 2. Every archive is hashed and compared with its published `.sha256`
//!    (and with `sha256.sum` when the manifest declares one). A missing,
//!    malformed or disagreeing checksum aborts the update.
//! 3. The archive listing is checked for absolute or `..` entries before
//!    extraction, and each binary is run once (`--version`) so a wrong-arch or
//!    wrong-version payload is caught before anything is installed.
//!
//! Nothing here writes outside the staging directory it is handed.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use clawft_rpc::doctor::probe::{parse_semver, probe_version, sha256_file};
use serde_json::Value;

/// Binaries `weaver update` is willing to install. A manifest naming anything
/// else is ignored for that entry: a release never gets to pick new file names
/// in the user's bin directory.
pub const KNOWN_BINARIES: [&str; 3] = ["weft", "weaver", "weftos"];

const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;

/// Where releases are fetched from.
#[derive(Debug, Clone)]
pub struct Source {
    /// `https://github.com/<owner>/<repo>/releases`.
    pub base: String,
    /// Allow plain `http://` (loopback mock servers in tests only).
    pub allow_http: bool,
}

impl Source {
    /// The real GitHub Releases of this project.
    pub fn github() -> Self {
        Self { base: "https://github.com/weave-logic-ai/weftos/releases".into(), allow_http: false }
    }

    fn manifest_url(&self) -> String {
        format!("{}/latest/download/dist-manifest.json", self.base)
    }

    fn asset_url(&self, tag: &str, name: &str) -> String {
        format!("{}/download/{tag}/{name}", self.base)
    }
}

/// One archive of the release set for this platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseArtifact {
    pub name: String,
    pub checksum_name: String,
    pub binaries: Vec<String>,
}

/// The release set for one target triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub artifacts: Vec<ReleaseArtifact>,
    pub unified_checksum: Option<String>,
}

impl Release {
    /// Every binary the release installs, in manifest order.
    pub fn binaries(&self) -> Vec<String> {
        self.artifacts.iter().flat_map(|a| a.binaries.clone()).collect()
    }

    /// Archive that carries `bin`.
    pub fn artifact_of(&self, bin: &str) -> Option<&ReleaseArtifact> {
        self.artifacts.iter().find(|a| a.binaries.iter().any(|b| b == bin))
    }
}

/// A verified, extracted binary waiting to be installed.
#[derive(Debug, Clone)]
pub struct Staged {
    pub name: String,
    pub path: PathBuf,
}

fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

/// Parse a cargo-dist `dist-manifest.json` for `triple`.
pub fn parse_manifest(text: &str, triple: &str) -> anyhow::Result<Release> {
    let v: Value = serde_json::from_str(text).context("dist-manifest.json is not valid JSON")?;
    let tag = v["announcement_tag"].as_str().ok_or_else(|| anyhow!("manifest has no announcement_tag"))?;
    if !safe_name(tag) {
        bail!("manifest tag {tag:?} is not a plain tag name");
    }
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    if parse_semver(&version).is_none() {
        bail!("manifest tag {tag:?} is not a version");
    }
    let arts = v["artifacts"].as_object().ok_or_else(|| anyhow!("manifest has no artifacts"))?;
    let mut artifacts: Vec<ReleaseArtifact> = Vec::new();
    let mut unified_checksum = None;
    for (key, a) in arts {
        match a["kind"].as_str() {
            Some("unified-checksum") => {
                let name = a["name"].as_str().unwrap_or(key);
                if !safe_name(name) {
                    bail!("manifest checksum file name {name:?} is not a plain file name");
                }
                unified_checksum = Some(name.to_string());
            }
            Some("executable-zip") => {
                let on_triple = a["target_triples"]
                    .as_array()
                    .is_some_and(|t| t.iter().any(|x| x.as_str() == Some(triple)));
                if !on_triple {
                    continue;
                }
                let name = a["name"].as_str().unwrap_or(key);
                if !safe_name(name) {
                    bail!("manifest archive name {name:?} is not a plain file name");
                }
                let checksum_name =
                    a["checksum"].as_str().map(str::to_owned).unwrap_or_else(|| format!("{name}.sha256"));
                if !safe_name(&checksum_name) {
                    bail!("manifest checksum name {checksum_name:?} is not a plain file name");
                }
                let binaries: Vec<String> = a["assets"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|x| x["kind"].as_str() == Some("executable"))
                    .filter_map(|x| x["name"].as_str().or_else(|| x["path"].as_str()))
                    .map(|n| n.trim_end_matches(".exe").to_string())
                    .filter(|n| KNOWN_BINARIES.contains(&n.as_str()))
                    .collect();
                if !binaries.is_empty() {
                    artifacts.push(ReleaseArtifact { name: name.into(), checksum_name, binaries });
                }
            }
            _ => {}
        }
    }
    if artifacts.is_empty() {
        bail!("release {tag} has no archive for {triple}");
    }
    let mut seen: Vec<String> = Vec::new();
    for b in artifacts.iter().flat_map(|a| &a.binaries) {
        if seen.contains(b) {
            bail!("release {tag} ships {b} in more than one archive for {triple}");
        }
        seen.push(b.clone());
    }
    if !seen.iter().any(|b| b == "weaver") {
        bail!("release {tag} has no weaver binary for {triple}");
    }
    Ok(Release { tag: tag.into(), version, artifacts, unified_checksum })
}

/// Hash from a `.sha256` file (`<hex>  <file>` or `<hex> *<file>`); when the
/// file names a name it must be `artifact`.
pub fn parse_sha256_file(text: &str, artifact: &str) -> anyhow::Result<String> {
    let line = text.lines().find(|l| !l.trim().is_empty()).ok_or_else(|| anyhow!("empty checksum file"))?;
    let mut parts = line.split_whitespace();
    let hash = parts.next().unwrap_or("").to_ascii_lowercase();
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("malformed checksum for {artifact}");
    }
    if let Some(file) = parts.next() {
        let file = file.trim_start_matches('*');
        let file = file.rsplit('/').next().unwrap_or(file);
        if file != artifact {
            bail!("checksum file is for {file}, not {artifact}");
        }
    }
    Ok(hash)
}

/// Hash for `artifact` out of a unified `sha256.sum`, when it lists it.
pub fn find_in_unified(text: &str, artifact: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut p = l.split_whitespace();
        let (h, f) = (p.next()?, p.next()?);
        let f = f.trim_start_matches('*');
        (f.rsplit('/').next() == Some(artifact)).then(|| h.to_ascii_lowercase())
    })
}

fn curl(src: &Source, url: &str, dest: &Path) -> anyhow::Result<()> {
    if !src.allow_http && !url.starts_with("https://") {
        bail!("refusing non-https URL {url}");
    }
    let mut c = Command::new("curl");
    c.args(["-fsSL", "--max-time", "300", "--max-filesize", "1073741824"]);
    if !src.allow_http {
        c.args(["--proto", "=https", "--proto-redir", "=https"]);
    }
    let status = c.arg("-o").arg(dest).arg(url).status().context("cannot run curl")?;
    if !status.success() {
        bail!("download failed ({status}): {url}");
    }
    Ok(())
}

fn fetch_text(src: &Source, url: &str, scratch: &Path) -> anyhow::Result<String> {
    let f = scratch.join("fetch.txt");
    curl(src, url, &f)?;
    if std::fs::metadata(&f)?.len() > MAX_TEXT_BYTES {
        bail!("{url} is larger than {MAX_TEXT_BYTES} bytes");
    }
    std::fs::read_to_string(&f).with_context(|| format!("{url} is not text"))
}

/// Fetch and parse the latest release's manifest for `triple`.
pub fn fetch_latest(src: &Source, triple: &str, scratch: &Path) -> anyhow::Result<Release> {
    let text = fetch_text(src, &src.manifest_url(), scratch)?;
    parse_manifest(&text, triple)
}

/// Run `bin --version` and require it to report `version`.
fn smoke_check(path: &Path, version: &str) -> anyhow::Result<()> {
    let got = probe_version(path, Duration::from_secs(10))
        .ok_or_else(|| anyhow!("{} did not run on this host (wrong platform?)", path.display()))?;
    if parse_semver(&got.version) != parse_semver(version) {
        bail!("{} reports {} but the release is {version}", path.display(), got.version);
    }
    Ok(())
}

fn unsafe_entry(entry: &str) -> bool {
    entry.starts_with('/') || entry.split(['/', '\\']).any(|c| c == "..")
}

fn archive_is_safe(tarball: &Path) -> anyhow::Result<()> {
    let out = Command::new("tar").arg("tzf").arg(tarball).output().context("cannot run tar")?;
    if !out.status.success() {
        bail!("{} is not a readable archive", tarball.display());
    }
    for entry in String::from_utf8_lossy(&out.stdout).lines() {
        if unsafe_entry(entry) {
            bail!("archive entry {entry:?} escapes the extraction directory");
        }
    }
    Ok(())
}

fn find_binary(dir: &Path, name: &str) -> Option<PathBuf> {
    let plain = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0);
    let top = dir.join(name);
    if plain(&top) {
        return Some(top);
    }
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path().join(name)).find(|p| plain(p))
}

/// Download, verify and extract every archive in `release` under `staging`.
/// Nothing outside `staging` is touched. Any failure aborts the whole set.
pub fn stage(
    src: &Source,
    release: &Release,
    staging: &Path,
    out: &mut dyn Write,
) -> anyhow::Result<Vec<Staged>> {
    let unified = match &release.unified_checksum {
        Some(n) => Some(fetch_text(src, &src.asset_url(&release.tag, n), staging)?),
        None => None,
    };
    let mut staged = Vec::new();
    for art in &release.artifacts {
        writeln!(out, "Downloading {}...", art.name)?;
        let tarball = staging.join(&art.name);
        curl(src, &src.asset_url(&release.tag, &art.name), &tarball)?;
        let want = parse_sha256_file(
            &fetch_text(src, &src.asset_url(&release.tag, &art.checksum_name), staging)?,
            &art.name,
        )?;
        let got = sha256_file(&tarball).ok_or_else(|| anyhow!("cannot hash {}", tarball.display()))?;
        if got != want {
            bail!("checksum mismatch for {}: published {want}, downloaded {got}", art.name);
        }
        if let Some(text) = &unified {
            match find_in_unified(text, &art.name) {
                Some(u) if u == want => {}
                Some(u) => bail!("sha256.sum disagrees for {}: {u} vs {want}", art.name),
                None => bail!("sha256.sum does not list {}", art.name),
            }
        }
        writeln!(out, "  verified sha256 {}", &got[..16])?;
        archive_is_safe(&tarball)?;
        let dir = staging.join(format!("x-{}", art.name));
        std::fs::create_dir_all(&dir)?;
        let st = Command::new("tar").arg("xzf").arg(&tarball).arg("-C").arg(&dir).status()?;
        if !st.success() {
            bail!("failed to extract {}", art.name);
        }
        for bin in &art.binaries {
            let path = find_binary(&dir, bin).ok_or_else(|| anyhow!("{bin} is not in {}", art.name))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            }
            smoke_check(&path, &release.version)?;
            staged.push(Staged { name: bin.clone(), path });
        }
    }
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_file_formats() {
        let h = "a".repeat(64);
        assert_eq!(parse_sha256_file(&format!("{h}  x.tar.gz\n"), "x.tar.gz").unwrap(), h);
        assert_eq!(parse_sha256_file(&format!("{} *x.tar.gz", h.to_uppercase()), "x.tar.gz").unwrap(), h);
        assert_eq!(parse_sha256_file(&h, "x.tar.gz").unwrap(), h);
        assert!(parse_sha256_file(&format!("{h}  y.tar.gz"), "x.tar.gz").is_err());
        assert!(parse_sha256_file("abc  x.tar.gz", "x.tar.gz").is_err());
        assert!(parse_sha256_file("", "x.tar.gz").is_err());
    }

    #[test]
    fn archive_entries_must_stay_inside() {
        assert!(unsafe_entry("/etc/passwd"));
        assert!(unsafe_entry("../x"));
        assert!(unsafe_entry("a/../../x"));
        assert!(!unsafe_entry("weaver-x/weaver"));
        assert!(!unsafe_entry("weaver-x/..hidden"));
    }

    #[test]
    fn manifest_rejects_unsafe_names_and_missing_weaver() {
        let m = |name: &str, bins: &str| {
            format!(
                r#"{{"announcement_tag":"v1.2.3","artifacts":{{"{name}":{{"kind":"executable-zip","name":"{name}","target_triples":["t"],"assets":[{bins}]}}}}}}"#
            )
        };
        let ok = r#"{"kind":"executable","name":"weaver"}"#;
        assert!(parse_manifest(&m("a.tar.gz", ok), "t").is_ok());
        assert!(parse_manifest(&m("../a.tar.gz", ok), "t").is_err());
        assert!(parse_manifest(&m("a.tar.gz", r#"{"kind":"executable","name":"weft"}"#), "t").is_err());
        // Unknown binary names are dropped, never installed.
        let r = parse_manifest(&m("a.tar.gz", &format!(r#"{ok},{{"kind":"executable","name":"evil"}}"#)), "t").unwrap();
        assert_eq!(r.binaries(), vec!["weaver".to_string()]);
        assert!(parse_manifest(&m("a.tar.gz", ok), "other-triple").is_err());
    }
}
