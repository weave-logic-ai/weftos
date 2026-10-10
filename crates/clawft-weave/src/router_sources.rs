//! Where routes come from: every project registered with the user daemon
//! (`~/.weftos/projects/*.toml`), each project's root and, for an ADR-108
//! workspace, the repository directories its manifest lists (`repos`). A
//! directory contributes routes when it holds `compose/ports.yaml`.
//!
//! Reload is cheap on purpose: a fingerprint of (path, mtime, len) over the
//! candidate files and the manifest directory itself is compared on each poll;
//! the table is rebuilt only when it changes (no watcher dependency).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::router_routes::{ProjectRoutes, RouteTable, parse_ports_yaml};

/// Relative path of the port registry inside a project (ADR-098).
pub const PORTS_FILE: &str = "compose/ports.yaml";
/// Most directories scanned per reload.
pub const MAX_DIRS: usize = 256;
/// Largest `ports.yaml` read.
pub const MAX_PORTS_FILE_BYTES: u64 = 64 * 1024;

/// A directory that may hold a `ports.yaml`, with the name used when the
/// file has no `project:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub dir: PathBuf,
}

/// A fingerprint element: a path and its (mtime, len), `None` when absent.
pub type Stamp = (PathBuf, Option<(SystemTime, u64)>);

/// Lists route candidates.
pub trait RouteSource: Send + Sync {
    fn candidates(&self) -> Vec<Candidate>;
    /// Paths whose change should trigger a reload, beyond the candidates' files.
    fn extra_watch(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// The user daemon's manifest index.
pub struct ManifestSource {
    pub manifests_dir: PathBuf,
}

impl RouteSource for ManifestSource {
    fn candidates(&self) -> Vec<Candidate> {
        let list = clawft_types::project::list_manifests(&self.manifests_dir).map(|l| l.manifests).unwrap_or_default();
        let mut out = Vec::new();
        for m in list {
            out.push(Candidate { name: m.name.clone(), dir: m.root.clone() });
            for dir in m.workspace_repos() {
                let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or(&m.name).to_owned();
                out.push(Candidate { name, dir });
            }
        }
        out.truncate(MAX_DIRS);
        out
    }

    fn extra_watch(&self) -> Vec<PathBuf> {
        vec![self.manifests_dir.clone()]
    }
}

/// A fixed list of directories (tests, and `weaver route list --dir`).
pub struct DirsSource(pub Vec<Candidate>);

impl RouteSource for DirsSource {
    fn candidates(&self) -> Vec<Candidate> {
        self.0.iter().take(MAX_DIRS).cloned().collect()
    }
}

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok().and_then(|m| m.modified().ok().map(|t| (t, m.len())));
    (path.to_path_buf(), meta)
}

/// The fingerprint of a source: one stamp per candidate `ports.yaml` plus the
/// extra watched paths. Equal fingerprints mean nothing to reload.
pub fn fingerprint(source: &dyn RouteSource) -> Vec<Stamp> {
    let mut out: Vec<Stamp> = source.candidates().iter().map(|c| stamp(&c.dir.join(PORTS_FILE))).collect();
    out.extend(source.extra_watch().iter().map(|p| stamp(p)));
    out
}

/// Read and admit every candidate's routes, in order.
pub fn load(source: &dyn RouteSource) -> RouteTable {
    let mut projects: Vec<ProjectRoutes> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for c in source.candidates() {
        let file = c.dir.join(PORTS_FILE);
        if seen.contains(&c.dir) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&file) else { continue };
        if !meta.is_file() || meta.len() > MAX_PORTS_FILE_BYTES {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        seen.push(c.dir.clone());
        projects.push(parse_ports_yaml(&text, &c.name, &c.dir));
    }
    RouteTable::build(projects)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, text: &str) {
        std::fs::create_dir_all(dir.join("compose")).unwrap();
        std::fs::write(dir.join(PORTS_FILE), text).unwrap();
    }

    #[test]
    fn dirs_without_a_ports_file_are_skipped_and_order_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let c = tmp.path().join("c");
        write(&a, "project: aa\nroutes:\n  - { port: 3001 }\n");
        std::fs::create_dir_all(&b).unwrap();
        write(&c, "routes:\n  - { prefix: /cc/deep, port: 3002 }\n");
        let src = DirsSource(vec![
            Candidate { name: "A".into(), dir: a.clone() },
            Candidate { name: "B".into(), dir: b },
            Candidate { name: "C Proj".into(), dir: c },
        ]);
        let t = load(&src);
        assert_eq!(t.projects.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(), ["aa", "c-proj"]);
        assert_eq!(t.routes[0].prefix, "/cc/deep");
        assert_eq!(t.routes[1].prefix, "/aa");
        assert!(t.refused.is_empty(), "{:?}", t.refused);
    }

    #[test]
    fn fingerprint_changes_with_the_file_and_is_stable_otherwise() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        write(&a, "routes:\n  - { port: 3001 }\n");
        let src = DirsSource(vec![Candidate { name: "a".into(), dir: a.clone() }]);
        let f1 = fingerprint(&src);
        assert_eq!(f1, fingerprint(&src));
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&a, "routes:\n  - { port: 3001 }\n  - { prefix: /a/two, port: 3003 }\n");
        assert_ne!(f1, fingerprint(&src));
    }

    #[test]
    fn manifest_source_lists_roots_and_workspace_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let manifests = tmp.path().join("projects");
        std::fs::create_dir_all(&manifests).unwrap();
        let root = tmp.path().join("ws");
        let repo = tmp.path().join("ws-repo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        let toml = format!(
            "schema = 1\nid = \"01M479THA4EGEQT5B1CF6WE1SN\"\nname = \"ws\"\nroot = {root:?}\nstate = \"active\"\n\
             created = \"2026-10-07T02:48:45Z\"\nlast_seen = \"2026-10-07T02:48:46Z\"\nproject_toml = \"present\"\n\
             role = \"workspace\"\nrepos = [{repo:?}]\n",
            root = root.display().to_string(),
            repo = repo.display().to_string()
        );
        std::fs::write(manifests.join("01M479THA4EGEQT5B1CF6WE1SN.toml"), toml).unwrap();
        let src = ManifestSource { manifests_dir: manifests.clone() };
        let c = src.candidates();
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(c[0].name, "ws");
        assert_eq!(c[1].dir, repo);
        assert_eq!(src.extra_watch(), vec![manifests]);
    }
}
