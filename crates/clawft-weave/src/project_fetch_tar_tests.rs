use std::path::Path;
use std::time::Duration;

use super::*;
use crate::project_fetch_repos::{RepoEntry, git};

fn w(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn g(repo: &Path, args: &[&str]) {
    git(repo, args, None, Duration::from_secs(30)).unwrap();
}

/// A project root: non-git `data/`, an archived `models/`, a sub-repository
/// `code/` with a tracked file, an ignored build dir and an untracked WIP file,
/// secrets of every kind, and a symlink pointing outside.
fn project() -> (tempfile::TempDir, Vec<RepoEntry>) {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    w(&r.join("data/a.bin"), "AAAA");
    w(&r.join("data/.env"), "SECRET=1");
    w(&r.join("data/sub/server.pem"), "pem");
    w(&r.join("data/notes.txt"), "n");
    w(&r.join("models/big.bin"), "weights");
    w(&r.join(".weftos/project.toml"), "id = \"x\"");
    w(&r.join(".weftos/archive.toml"), "version = 1\n[[archive]]\npath = \"models/\"\nreason = \"weights live on the primary\"\n");
    w(&r.join("outside.txt"), "o");
    #[cfg(unix)]
    std::os::unix::fs::symlink(r.join("outside.txt"), r.join("data/link")).unwrap();
    let code = r.join("code");
    w(&code.join("tracked.txt"), "t");
    w(&code.join(".gitignore"), "target/\n");
    w(&code.join("target/out.o"), "object");
    w(&code.join("wip.txt"), "uncommitted");
    g(&code, &["init", "-q"]);
    g(&code, &["add", "tracked.txt", ".gitignore"]);
    g(&code, &["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "init"]);
    let repos = vec![RepoEntry { dir: "code".into(), path: code }];
    (t, repos)
}

fn plan_of(root: &Path, repos: &[RepoEntry]) -> (Plan, Vec<ArchiveEntry>) {
    let archive = read_archive(root).unwrap();
    let p = plan(&Exclusions { root, archive: &archive, repos, secret_dirs: &[root.join(".weftos/chain")] });
    (p, archive)
}

#[test]
fn the_plan_keeps_non_git_and_ignored_content_and_nothing_secret_archived_or_linked() {
    let (t, repos) = project();
    let (p, archive) = plan_of(t.path(), &repos);
    assert_eq!(archive[0].path, "models");
    let files: Vec<&str> = p.files.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(files, vec!["code/target/out.o", "data/a.bin", "data/notes.txt", "outside.txt"], "{files:?}");
    assert_eq!(p.archived, vec!["models (weights live on the primary)"]);
    // .env, server.pem and the symlink.
    assert_eq!(p.excluded, 3);
    assert!(!p.truncated && !p.is_large());
    assert_eq!(p.bytes, 4 + 1 + 6 + 1);
    assert!(p.dirs.contains(&"data".to_string()) && p.dirs.contains(&"code/target".to_string()));
    assert!(!p.dirs.iter().any(|d| d.starts_with(".weftos") || d == "models" || d == "code"), "{:?}", p.dirs);
    assert_eq!(p.largest(2), vec!["code (0 MiB)".to_string(), "data (0 MiB)".to_string()]);
}

#[test]
fn tar_round_trip_recreates_only_the_planned_files() {
    let (t, repos) = project();
    let (p, _) = plan_of(t.path(), &repos);
    let out = t.path().join("out.tar");
    let bytes = build_tar(t.path(), &p, &out).unwrap();
    assert!(bytes > 0);
    let dest = tempfile::tempdir().unwrap();
    let u = unpack_tar(&out, dest.path()).unwrap();
    assert_eq!((u.files, u.refused), (4, 0));
    assert_eq!(std::fs::read_to_string(dest.path().join("data/a.bin")).unwrap(), "AAAA");
    assert_eq!(std::fs::read_to_string(dest.path().join("code/target/out.o")).unwrap(), "object");
    for absent in ["data/.env", "data/sub/server.pem", "data/link", "models/big.bin", ".weftos/project.toml", "code/tracked.txt", "code/wip.txt"] {
        assert!(std::fs::symlink_metadata(dest.path().join(absent)).is_err(), "{absent} must not arrive");
    }
}

#[test]
fn unpack_refuses_links_and_paths_that_leave_the_destination() {
    let t = tempfile::tempdir().unwrap();
    let tar_path = t.path().join("evil.tar");
    {
        let mut b = tar::Builder::new(std::fs::File::create(&tar_path).unwrap());
        // The tar crate refuses to write `..` itself, so the hostile name goes
        // into the raw header bytes.
        let mut h = tar::Header::new_gnu();
        h.as_gnu_mut().unwrap().name[..13].copy_from_slice(b"../escape.txt");
        h.set_size(1);
        h.set_mode(0o644);
        h.set_cksum();
        b.append(&h, &b"x"[..]).unwrap();
        let mut h = tar::Header::new_gnu();
        h.as_gnu_mut().unwrap().name[..4].copy_from_slice(b"link");
        h.as_gnu_mut().unwrap().linkname[..10].copy_from_slice(b"/etc/hosts");
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        h.set_mode(0o777);
        h.set_cksum();
        b.append(&h, &[][..]).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(2);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "ok/fine.txt", &b"ok"[..]).unwrap();
        b.finish().unwrap();
    }
    let dest = t.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let u = unpack_tar(&tar_path, &dest).unwrap();
    assert_eq!((u.files, u.refused), (1, 2));
    assert!(!t.path().join("escape.txt").exists());
    assert!(std::fs::symlink_metadata(dest.join("link")).is_err());
    assert_eq!(std::fs::read_to_string(dest.join("ok/fine.txt")).unwrap(), "ok");
}

#[test]
fn archive_list_paths_must_stay_inside_the_project() {
    let t = tempfile::tempdir().unwrap();
    for bad in ["../x", "/etc", "a/../b", ""] {
        w(&t.path().join(".weftos/archive.toml"), &format!("[[archive]]\npath = \"{bad}\"\n"));
        assert!(read_archive(t.path()).is_err(), "{bad:?}");
    }
    w(&t.path().join(".weftos/archive.toml"), "[[archive]]\npath = \"data/raw/\"\n");
    assert_eq!(read_archive(t.path()).unwrap()[0].path, "data/raw");
    std::fs::remove_file(t.path().join(".weftos/archive.toml")).unwrap();
    assert!(read_archive(t.path()).unwrap().is_empty());
}

#[test]
fn credential_shaped_names() {
    for s in [".env", ".env.local", "prod.env", ".netrc", "id_rsa", "id_ed25519.pub", "server.pem", "node.key", ".ssh", "store.p12"] {
        assert!(secret_name(s), "{s}");
    }
    for s in ["env.rs", "keys.md", "README", "environment.toml", "monkey.txt", "prod.env.bak", "envelope"] {
        assert!(!secret_name(s), "{s}");
    }
}
