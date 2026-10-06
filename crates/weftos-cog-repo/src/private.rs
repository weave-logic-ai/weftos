//! Private repository commands (ADR-105): `init`, `keygen`, `add`, `sign`, `verify`, `publish`.
//!
//! A private repo is a directory:
//!
//! ```text
//! <repo-dir>/repo.toml     name + public key (committable)
//! <repo-dir>/.gitignore    ignores keys and the working copies
//! <repo-dir>/dist/<id>/    staged cogs: cog-<id>-arm[-arm64][-x86_64] + manifest.json
//! <repo-dir>/repo/         signed output: registry.json + cogs/<arch>/...  (this is what you host)
//! ```
//!
//! The private key is written by `keygen` to a path you choose, with mode 0600, and is refused
//! inside the repo directory so it cannot be committed by accident. This tool never uploads
//! anything: `publish` copies the verified `repo/` to a directory, and hosting it is up to you.

use std::path::{Path, PathBuf};

use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use ed25519_dalek::{SigningKey, VerifyingKey};
use weftos_cog_repo::{Registry, RevokedKeys, COG_ARCHES};

use crate::{arg, fetch, revocations, sign_tree, verify_any, R};

const REPO_TOML: &str = "repo.toml";
const GITIGNORE: &str = "# Never commit signing keys.\n*.pem\n*.key\n*.seed\n";

/// 64 hex chars -> a valid Ed25519 public key.
pub fn parse_pubkey(s: &str) -> R<VerifyingKey> {
    let raw: [u8; 32] = hex::decode(s.trim())
        .map_err(|e| format!("public key is not hex: {e}"))?
        .try_into()
        .map_err(|_| "public key must be 32 bytes (64 hex chars)".to_string())?;
    VerifyingKey::from_bytes(&raw).map_err(|e| format!("not a valid Ed25519 public key: {e}"))
}

struct RepoConfig {
    name: String,
    pubkey: Option<String>,
}

fn load_config(dir: &Path) -> R<RepoConfig> {
    let p = dir.join(REPO_TOML);
    let text = std::fs::read_to_string(&p).map_err(|e| {
        format!(
            "{} is not a private repo (no {REPO_TOML}: {e}); run `init` first",
            dir.display()
        )
    })?;
    let v: toml::Value = toml::from_str(&text).map_err(|e| format!("parse {p:?}: {e}"))?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
    Ok(RepoConfig {
        name: get("name").ok_or("repo.toml has no name")?,
        pubkey: get("pubkey"),
    })
}

fn write_config(dir: &Path, c: &RepoConfig) -> R<()> {
    let mut t = toml::Table::new();
    t.insert("name".into(), toml::Value::String(c.name.clone()));
    if let Some(k) = &c.pubkey {
        t.insert("pubkey".into(), toml::Value::String(k.clone()));
    }
    std::fs::write(
        dir.join(REPO_TOML),
        toml::to_string_pretty(&t).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("write {REPO_TOML}: {e}"))
}

fn repo_key(dir: &Path) -> R<VerifyingKey> {
    let c = load_config(dir)?;
    let k = c
        .pubkey
        .ok_or("repo.toml has no pubkey: run `keygen --out <key.pem> --repo <repo-dir>` first")?;
    parse_pubkey(&k)
}

fn valid_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

fn valid_cog_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

// ---- init ------------------------------------------------------------------

pub fn cmd_init(args: &[String]) -> R<()> {
    let dir = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or("init needs <repo-dir>")?;
    let name = arg(args, "--name")
        .ok_or("init needs --name <repo-name> (lower-case, e.g. acme-private)")?;
    if !valid_name(name) {
        return Err(format!(
            "bad repo name {name:?} (use [a-z0-9][a-z0-9_-]{{0,31}})"
        ));
    }
    let dir = PathBuf::from(dir);
    if dir.join(REPO_TOML).exists() {
        return Err(format!(
            "{} already holds a repo ({REPO_TOML} exists); not overwriting",
            dir.display()
        ));
    }
    std::fs::create_dir_all(dir.join("dist")).map_err(|e| format!("mkdir: {e}"))?;
    write_config(
        &dir,
        &RepoConfig {
            name: name.to_string(),
            pubkey: None,
        },
    )?;
    std::fs::write(dir.join(".gitignore"), GITIGNORE)
        .map_err(|e| format!("write .gitignore: {e}"))?;
    eprintln!("initialised private repo '{name}' in {}", dir.display());
    eprintln!(
        "next: weft-cog-repo keygen --out <path outside the repo>/{name}.pem --repo {}",
        dir.display()
    );
    Ok(())
}

// ---- keygen ----------------------------------------------------------------

fn random_seed() -> R<[u8; 32]> {
    use std::io::Read;
    let mut seed = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut seed))
        .map_err(|e| format!("cannot read /dev/urandom: {e} (keygen supports unix hosts)"))?;
    Ok(seed)
}

/// Resolve `path` without requiring it to exist: canonicalize the nearest existing ancestor and
/// append the rest. A `..` in the part that does not exist yet is refused (it could climb back
/// into a directory we are guarding).
fn resolve_lenient(path: &Path) -> R<PathBuf> {
    use std::path::Component;
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("cwd: {e}"))?
            .join(path)
    };
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = abs.as_path();
    loop {
        if let Ok(real) = cur.canonicalize() {
            let mut out = real;
            for c in rest.iter().rev() {
                out.push(c);
            }
            return Ok(out);
        }
        match cur.components().next_back() {
            Some(Component::Normal(n)) => rest.push(n.to_os_string()),
            Some(Component::ParentDir) => {
                return Err(format!(
                "refusing a '..' component in {path:?} below a directory that does not exist yet"
            ))
            }
            Some(Component::CurDir) => {}
            _ => return Err(format!("cannot resolve {path:?}")),
        }
        cur = cur
            .parent()
            .ok_or_else(|| format!("cannot resolve {path:?}"))?;
    }
}

fn inside(path: &Path, dir: &Path) -> R<bool> {
    Ok(resolve_lenient(path)?.starts_with(resolve_lenient(dir)?))
}

pub fn cmd_keygen(args: &[String]) -> R<()> {
    let out = PathBuf::from(arg(args, "--out").ok_or("keygen needs --out <key.pem>")?);
    let repo = arg(args, "--repo").map(PathBuf::from);
    if let Some(r) = &repo {
        if inside(&out, r)? {
            return Err(format!(
                "refusing to write the private key inside the repo directory {}: it could be committed. Choose a path outside it (e.g. ~/.config/weftos/keys/)",
                r.display()
            ));
        }
        load_config(r)?; // fail before writing a key if the repo is not initialised
    }
    let key = SigningKey::from_bytes(&random_seed()?);
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| format!("encode key: {e}"))?;
    write_private(&out, pem.as_bytes())?;
    let pubkey = hex::encode(key.verifying_key().to_bytes());
    if let Some(r) = &repo {
        let mut c = load_config(r)?;
        c.pubkey = Some(pubkey.clone());
        write_config(r, &c)?;
    }
    eprintln!(
        "wrote private key {} (mode 0600). Back it up; never commit it.",
        out.display()
    );
    println!("{pubkey}");
    Ok(())
}

/// Create a new file with mode 0600 from the start; an existing file is never overwritten.
fn write_private(path: &Path, bytes: &[u8]) -> R<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {dir:?}: {e}"))?;
    }
    let mut f = o
        .open(path)
        .map_err(|e| format!("create {path:?}: {e} (an existing key is never overwritten)"))?;
    f.write_all(bytes)
        .map_err(|e| format!("write {path:?}: {e}"))
}

// ---- add -------------------------------------------------------------------

struct Meta {
    id: Option<String>,
    name: Option<String>,
    version: Option<String>,
    category: Option<String>,
    description: Option<String>,
    hardware: Vec<String>,
}

fn meta_from_cog_toml(path: &str) -> R<Meta> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let v: toml::Value = toml::from_str(&text).map_err(|e| format!("parse {path}: {e}"))?;
    let c = v
        .get("cog")
        .ok_or_else(|| format!("{path} has no [cog] table"))?;
    let s = |k: &str| c.get(k).and_then(|x| x.as_str()).map(String::from);
    let hardware = c
        .get("hardware_requirement")
        .and_then(|h| h.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok(Meta {
        id: s("id"),
        name: s("name"),
        version: s("version"),
        category: s("category"),
        description: s("description"),
        hardware,
    })
}

pub fn cmd_add(args: &[String]) -> R<()> {
    let dir = PathBuf::from(
        args.first()
            .filter(|a| !a.starts_with("--"))
            .ok_or("add needs <repo-dir>")?,
    );
    load_config(&dir)?;
    let binary = arg(args, "--binary").ok_or("add needs --binary <cog binary>")?;
    let arch = arg(args, "--arch").unwrap_or("arm");
    let suffix = COG_ARCHES
        .iter()
        .find(|pair| pair.0 == arch)
        .map(|pair| pair.1)
        .ok_or_else(|| format!("--arch must be arm, arm64, or x86_64, got {arch:?}"))?;
    let mut meta = match arg(args, "--cog-toml") {
        Some(p) => meta_from_cog_toml(p)?,
        None => Meta {
            id: None,
            name: None,
            version: None,
            category: None,
            description: None,
            hardware: vec![],
        },
    };
    if let Some(v) = arg(args, "--id") {
        meta.id = Some(v.into());
    }
    let id = meta
        .id
        .clone()
        .ok_or("add needs --id <cog-id> (or a --cog-toml with [cog].id)")?;
    if !valid_cog_id(&id) {
        return Err(format!(
            "bad cog id {id:?} (lower-case alphanumerics and single hyphens)"
        ));
    }
    let bytes = std::fs::read(binary).map_err(|e| format!("read {binary}: {e}"))?;
    if bytes.is_empty() {
        return Err(format!("{binary} is empty"));
    }
    let cog_dir = dir.join("dist").join(&id);
    std::fs::create_dir_all(&cog_dir).map_err(|e| format!("mkdir: {e}"))?;
    std::fs::write(cog_dir.join(format!("cog-{id}{suffix}")), &bytes)
        .map_err(|e| format!("write binary: {e}"))?;

    let manifest_path = cog_dir.join("manifest.json");
    if let Some(m) = arg(args, "--manifest") {
        std::fs::copy(m, &manifest_path).map_err(|e| format!("copy manifest: {e}"))?;
    } else if !manifest_path.exists()
        || args.iter().any(|a| {
            a.starts_with("--name")
                || a == "--version"
                || a == "--category"
                || a == "--description"
                || a == "--hardware"
        })
        || arg(args, "--cog-toml").is_some()
    {
        let pick = |flag: &str, from: &Option<String>, dflt: &str| {
            arg(args, flag)
                .map(String::from)
                .or_else(|| from.clone())
                .unwrap_or_else(|| dflt.to_string())
        };
        let hardware: Vec<String> = match arg(args, "--hardware") {
            Some(h) => h
                .split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect(),
            None => meta.hardware.clone(),
        };
        let manifest = serde_json::json!({
            "name": pick("--name", &meta.name, &id),
            "version": pick("--version", &meta.version, "0.0.0"),
            "category": pick("--category", &meta.category, ""),
            "description": pick("--description", &meta.description, ""),
            "hardware_requirement": hardware,
        });
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("write manifest: {e}"))?;
    }
    eprintln!(
        "staged {id} [{arch}] ({} bytes) in {}",
        bytes.len(),
        cog_dir.display()
    );
    Ok(())
}

// ---- sign / verify / publish ----------------------------------------------

pub fn cmd_sign(dir: &str, args: &[String]) -> R<()> {
    let dir = PathBuf::from(dir);
    let cfg = load_config(&dir)?;
    let want = repo_key(&dir)?;
    let pem_path = arg(args, "--key").ok_or("sign needs --key <key.pem>")?;
    let pem = std::fs::read_to_string(pem_path).map_err(|e| format!("read key {pem_path}: {e}"))?;
    let key = SigningKey::from_pkcs8_pem(&pem).map_err(|e| format!("parse pkcs8 pem: {e}"))?;
    if key.verifying_key().to_bytes() != want.to_bytes() {
        return Err(format!("the signing key does not match the public key in {}/{REPO_TOML}; consumers pinning that key would reject the repo", dir.display()));
    }
    let from = dir.join("dist");
    sign_tree(
        from.to_str().ok_or("bad path")?,
        &dir.join("repo"),
        &cfg.name,
        &key,
    )
}

/// Checks every artifact of `<dir>/repo` against `pins`, or the key in repo.toml when `pins` is
/// empty; returns the artifact count. A `--pin` that differs from repo.toml fails, it is never
/// silently replaced by the repo's own key.
fn verify_repo(dir: &Path, pins: &[VerifyingKey], revoked: &RevokedKeys) -> R<usize> {
    let keys: Vec<VerifyingKey> = if pins.is_empty() {
        vec![repo_key(dir)?]
    } else {
        pins.to_vec()
    };
    let base = dir.join("repo");
    let base_s = base.to_str().ok_or("bad path")?;
    let reg: Registry = serde_json::from_slice(&fetch(base_s, "registry.json")?)
        .map_err(|e| format!("parse registry.json: {e}"))?;
    let mut n = 0;
    let mut bad = Vec::new();
    for cog in &reg.cogs {
        for (arch, art) in &cog.artifacts {
            match fetch(base_s, &art.path)
                .and_then(|b| verify_any(&b, art, &keys, revoked).map_err(|e| e.to_string()))
            {
                Ok(()) => n += 1,
                Err(e) => bad.push(format!("{} [{arch}]: {e}", cog.id)),
            }
        }
    }
    if !bad.is_empty() {
        return Err(format!(
            "{} artifact(s) failed verification:\n  {}",
            bad.len(),
            bad.join("\n  ")
        ));
    }
    Ok(n)
}

pub fn cmd_verify(dir: &str, pins: &[VerifyingKey], revoked: &RevokedKeys) -> R<()> {
    let n = verify_repo(Path::new(dir), pins, revoked)?;
    if pins.is_empty() {
        eprintln!("{n} artifact(s) verified against the key in {dir}/{REPO_TOML}");
    } else {
        eprintln!(
            "{n} artifact(s) verified against the {} key(s) given with --pin",
            pins.len()
        );
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> R<()> {
    std::fs::create_dir_all(to).map_err(|e| format!("mkdir {to:?}: {e}"))?;
    for e in std::fs::read_dir(from)
        .map_err(|e| format!("read {from:?}: {e}"))?
        .flatten()
    {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let lower = e.file_name().to_string_lossy().to_ascii_lowercase();
        if lower.ends_with(".pem") || lower.ends_with(".key") || lower.ends_with(".seed") {
            return Err(format!(
                "refusing to publish {src:?}: it looks like key material"
            ));
        }
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("copy {src:?}: {e}"))?;
        }
    }
    Ok(())
}

pub fn cmd_publish(args: &[String]) -> R<()> {
    let dir = PathBuf::from(
        args.first()
            .filter(|a| !a.starts_with("--"))
            .ok_or("publish needs <repo-dir>")?,
    );
    let to = PathBuf::from(arg(args, "--to").ok_or("publish needs --to <dir>")?);
    let cfg = load_config(&dir)?;
    let n = verify_repo(&dir, &[], &revocations(args)?)?; // never publish something that does not verify
    copy_tree(&dir.join("repo"), &to)?;
    let pubkey = cfg.pubkey.clone().unwrap_or_default();
    eprintln!("published {n} verified artifact(s) to {}", to.display());
    eprintln!("host that directory yourself (static HTTPS, object storage or a file share), then in each project:\n");
    println!("[[cog_source]]\nname = \"{}\"\nkind = \"private\"\nurl = \"<URL or path where you host the published directory>\"\npinned_keys = [\"{pubkey}\"]", cfg.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("acme");
        let keys = tmp.path().join("keys");
        cmd_init(&a(&[&s(&repo), "--name", "acme-private"])).unwrap();
        cmd_keygen(&a(&[
            "--out",
            &s(&keys.join("acme.pem")),
            "--repo",
            &s(&repo),
        ]))
        .unwrap();
        let bin = tmp.path().join("cog-gauge");
        std::fs::write(&bin, b"\x7fELF acme gauge").unwrap();
        cmd_add(&a(&[
            &s(&repo),
            "--binary",
            &s(&bin),
            "--id",
            "acme-gauge",
            "--version",
            "0.2.0",
            "--hardware",
            "pi-zero-2w,v0-appliance",
        ]))
        .unwrap();
        (tmp, repo, keys.join("acme.pem"))
    }

    #[test]
    fn init_keygen_add_sign_verify_publish_round_trip() {
        let (tmp, repo, key) = setup();
        cmd_sign(&s(&repo), &a(&["--key", &s(&key)])).unwrap();
        cmd_verify(&s(&repo), &[], &RevokedKeys::none()).unwrap();
        let reg: Registry =
            serde_json::from_slice(&std::fs::read(repo.join("repo/registry.json")).unwrap())
                .unwrap();
        assert_eq!(reg.repo, "acme-private");
        assert_eq!(reg.cogs[0].id, "acme-gauge");
        assert_eq!(reg.cogs[0].version, "0.2.0");
        assert_eq!(
            reg.cogs[0].hardware_requirement,
            vec!["pi-zero-2w", "v0-appliance"]
        );

        let out = tmp.path().join("hosted");
        cmd_publish(&a(&[&s(&repo), "--to", &s(&out)])).unwrap();
        assert!(
            out.join("registry.json").is_file()
                && out.join("cogs/arm/cog-acme-gauge-arm").is_file()
        );
        // the published tree verifies against the repo key with the generic --pin path too
        let pubkey = load_config(&repo).unwrap().pubkey.unwrap();
        let k = parse_pubkey(&pubkey).unwrap();
        let r: Registry =
            serde_json::from_slice(&std::fs::read(out.join("registry.json")).unwrap()).unwrap();
        let art = &r.cogs[0].artifacts["arm"];
        verify_any(
            &std::fs::read(out.join(&art.path)).unwrap(),
            art,
            &[k],
            &RevokedKeys::none(),
        )
        .unwrap();
        // and does not verify under the WeaveLogic key
        assert!(verify_any(
            &std::fs::read(out.join(&art.path)).unwrap(),
            art,
            &[weftos_cog_repo::weavelogic_key()],
            &RevokedKeys::none()
        )
        .is_err());
    }

    #[test]
    fn a_revoked_repo_key_is_refused_by_verify_publish_and_the_public_path() {
        let (tmp, repo, key) = setup();
        cmd_sign(&s(&repo), &a(&["--key", &s(&key)])).unwrap();
        let pubkey = load_config(&repo).unwrap().pubkey.unwrap();
        let revoked = RevokedKeys::from_keys([pubkey.clone()]);
        let e = cmd_verify(&s(&repo), &[], &revoked).unwrap_err();
        assert!(e.contains("revoked"), "{e}");

        // The kernel's list format, from a file.
        let list = tmp.path().join("revoked_subjects.json");
        std::fs::write(
            &list,
            format!(
                r#"[{{"kind":"signer_key","id":"{pubkey}","revoked_at":1,"reason":"leaked"}}]"#
            ),
        )
        .unwrap();
        let out = tmp.path().join("hosted");
        let e = cmd_publish(&a(&[
            &s(&repo),
            "--to",
            &s(&out),
            "--revocations",
            &s(&list),
        ]))
        .unwrap_err();
        assert!(e.contains("revoked"), "{e}");
        assert!(!out.exists(), "nothing published");

        // The plain-registry path (weft-cog-repo verify <dir>, no repo.toml).
        cmd_publish(&a(&[&s(&repo), "--to", &s(&out)])).unwrap();
        let args = a(&[&s(&out), "--pin", &pubkey]);
        assert!(crate::cmd_verify(&args).is_ok());
        let args = a(&[&s(&out), "--pin", &pubkey, "--revocations", &s(&list)]);
        assert!(crate::cmd_verify(&args)
            .unwrap_err()
            .contains("failed verification"));
        // a revocation of some other key changes nothing
        let other = tmp.path().join("other.json");
        std::fs::write(
            &other,
            format!(
                r#"[{{"kind":"signer_key","id":"{}","revoked_at":1,"reason":"r"}}]"#,
                "cd".repeat(32)
            ),
        )
        .unwrap();
        let args = a(&[&s(&out), "--pin", &pubkey, "--revocations", &s(&other)]);
        assert!(crate::cmd_verify(&args).is_ok());
        // a malformed list fails closed
        let bad = tmp.path().join("bad.json");
        std::fs::write(&bad, "oops").unwrap();
        let args = a(&[&s(&out), "--pin", &pubkey, "--revocations", &s(&bad)]);
        assert!(crate::cmd_verify(&args).is_err());
    }

    #[test]
    fn verify_pin_wins_over_repo_toml() {
        let (tmp, repo, key) = setup();
        cmd_sign(&s(&repo), &a(&["--key", &s(&key)])).unwrap();
        // repo.toml declares the signing key; a --pin for another key must fail, not be ignored
        let other = tmp.path().join("other.pem");
        let other_pub = {
            cmd_keygen(&a(&["--out", &s(&other)])).unwrap();
            let k = SigningKey::from_pkcs8_pem(&std::fs::read_to_string(&other).unwrap()).unwrap();
            k.verifying_key()
        };
        assert!(cmd_verify(&s(&repo), &[other_pub], &RevokedKeys::none()).is_err());
        // pinning the real key passes, and so does pinning both
        let real = parse_pubkey(&load_config(&repo).unwrap().pubkey.unwrap()).unwrap();
        cmd_verify(&s(&repo), &[real], &RevokedKeys::none()).unwrap();
        cmd_verify(&s(&repo), &[other_pub, real], &RevokedKeys::none()).unwrap();
    }

    #[test]
    fn key_is_0600_never_overwritten_and_refused_inside_the_repo() {
        let (tmp, repo, key) = setup();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(std::fs::read_to_string(&key)
            .unwrap()
            .contains("PRIVATE KEY"));
        // overwrite refused
        let e = cmd_keygen(&a(&["--out", &s(&key)])).unwrap_err();
        assert!(e.contains("never overwritten"), "{e}");
        // inside the repo refused
        let e = cmd_keygen(&a(&[
            "--out",
            &s(&repo.join("dist/oops.pem")),
            "--repo",
            &s(&repo),
        ]))
        .unwrap_err();
        assert!(e.contains("inside the repo"), "{e}");
        assert!(!repo.join("dist/oops.pem").exists());
        // ... also when the parent directory does not exist yet
        let e = cmd_keygen(&a(&[
            "--out",
            &s(&repo.join("keys/new/acme.pem")),
            "--repo",
            &s(&repo),
        ]))
        .unwrap_err();
        assert!(e.contains("inside the repo"), "{e}");
        assert!(
            !repo.join("keys").exists(),
            "nothing was created inside the repo"
        );
        // ... and with a relative path and a `..` that climbs back in
        let e = cmd_keygen(&a(&[
            "--out",
            &s(&tmp.path().join("fresh/../acme/keys/k.pem")),
            "--repo",
            &s(&repo),
        ]))
        .unwrap_err();
        assert!(e.contains("inside the repo") || e.contains("'..'"), "{e}");
        assert!(!repo.join("keys").exists());
        // a path outside the repo whose parent does not exist yet is fine
        cmd_keygen(&a(&["--out", &s(&tmp.path().join("brand/new/dir/k.pem"))])).unwrap();
        // .gitignore guards keys
        assert!(std::fs::read_to_string(repo.join(".gitignore"))
            .unwrap()
            .contains("*.pem"));
        let _ = tmp;
    }

    #[test]
    fn sign_refuses_a_key_that_does_not_match_repo_toml() {
        let (tmp, repo, _key) = setup();
        let other = tmp.path().join("other.pem");
        cmd_keygen(&a(&["--out", &s(&other)])).unwrap();
        let e = cmd_sign(&s(&repo), &a(&["--key", &s(&other)])).unwrap_err();
        assert!(e.contains("does not match"), "{e}");
        assert!(!repo.join("repo/registry.json").exists());
    }

    #[test]
    fn verify_and_publish_refuse_a_tampered_repo() {
        let (tmp, repo, key) = setup();
        cmd_sign(&s(&repo), &a(&["--key", &s(&key)])).unwrap();
        std::fs::write(
            repo.join("repo/cogs/arm/cog-acme-gauge-arm"),
            b"\x7fELF evil gauge!",
        )
        .unwrap();
        assert!(cmd_verify(&s(&repo), &[], &RevokedKeys::none()).is_err());
        let out = tmp.path().join("hosted");
        assert!(cmd_publish(&a(&[&s(&repo), "--to", &s(&out)])).is_err());
        assert!(
            !out.exists(),
            "nothing is published when verification fails"
        );
    }

    #[test]
    fn init_refuses_existing_and_bad_names_and_add_validates() {
        let (tmp, repo, _) = setup();
        assert!(cmd_init(&a(&[&s(&repo), "--name", "x"]))
            .unwrap_err()
            .contains("already"));
        assert!(
            cmd_init(&a(&[&s(&tmp.path().join("n")), "--name", "Bad:Name"]))
                .unwrap_err()
                .contains("bad repo name")
        );
        let bin = tmp.path().join("b");
        std::fs::write(&bin, b"x").unwrap();
        assert!(cmd_add(&a(&[&s(&repo), "--binary", &s(&bin), "--id", "Bad_Id"])).is_err());
        assert!(cmd_add(&a(&[
            &s(&repo),
            "--binary",
            &s(&bin),
            "--id",
            "ok",
            "--arch",
            "x86"
        ]))
        .is_err());
        cmd_add(&a(&[
            &s(&repo),
            "--binary",
            &s(&bin),
            "--id",
            "host",
            "--arch",
            "x86_64",
        ]))
        .unwrap();
        assert!(repo.join("dist/host/cog-host-x86_64").is_file());
        assert!(cmd_add(&a(&["/no/such/repo", "--binary", &s(&bin), "--id", "ok"])).is_err());
        // metadata from a cog.toml
        let ct = tmp.path().join("cog.toml");
        std::fs::write(&ct, "[cog]\nid=\"from-toml\"\nname=\"From Toml\"\nversion=\"3.1.0\"\ncategory=\"sensing\"\nhardware_requirement=[\"v0-appliance\"]\n").unwrap();
        cmd_add(&a(&[
            &s(&repo),
            "--binary",
            &s(&bin),
            "--cog-toml",
            &s(&ct),
            "--arch",
            "arm64",
        ]))
        .unwrap();
        let m: serde_json::Value = serde_json::from_slice(
            &std::fs::read(repo.join("dist/from-toml/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(m["version"], "3.1.0");
        assert!(repo.join("dist/from-toml/cog-from-toml-arm64").is_file());
    }
}
