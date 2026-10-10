//! Where routes come from: every project registered with the user daemon
//! (`~/.weftos/projects/*.toml`), each project's root and, for an ADR-108
//! workspace, the repository directories its manifest lists (`repos`). A
//! directory contributes routes when it holds `compose/ports.yaml`; a
//! registered project also contributes the routes of its dashboard overlay
//! (`~/.weftos/routes/<ULID>.yaml`, ADR-116 R3), merged after its own.
//!
//! Reload is cheap on purpose: a fingerprint of (path, mtime, len) over the
//! candidate files, the overlays and the directories themselves is compared
//! on each poll; the table is rebuilt only when it changes (no watcher
//! dependency).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::router_overlay::{overlay_path, parse_overlay};
use crate::router_routes::{ProjectRoutes, RouteTable, parse_ports_yaml};

/// Relative path of the port registry inside a project (ADR-098).
pub const PORTS_FILE: &str = "compose/ports.yaml";
/// Most directories scanned per reload.
pub const MAX_DIRS: usize = 256;
/// Largest `ports.yaml` read.
pub const MAX_PORTS_FILE_BYTES: u64 = 64 * 1024;

/// A directory that may hold a `ports.yaml`, with the name used when the
/// file has no `project:`, and the project ULID when it is a manifest root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub dir: PathBuf,
    pub ulid: Option<String>,
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
    /// Where dashboard overlays live; `None` reads no overlays.
    fn overlays_dir(&self) -> Option<PathBuf> {
        None
    }
}

/// The user daemon's manifest index.
pub struct ManifestSource {
    pub manifests_dir: PathBuf,
    /// `~/.weftos/routes`.
    pub overlays_dir: PathBuf,
}

impl RouteSource for ManifestSource {
    fn candidates(&self) -> Vec<Candidate> {
        let list = clawft_types::project::list_manifests(&self.manifests_dir).map(|l| l.manifests).unwrap_or_default();
        let mut out = Vec::new();
        for m in list {
            out.push(Candidate { name: m.name.clone(), dir: m.root.clone(), ulid: Some(m.id.clone()) });
            for dir in m.workspace_repos() {
                let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or(&m.name).to_owned();
                out.push(Candidate { name, dir, ulid: None });
            }
        }
        out.truncate(MAX_DIRS);
        out
    }

    fn extra_watch(&self) -> Vec<PathBuf> {
        vec![self.manifests_dir.clone(), self.overlays_dir.clone()]
    }

    fn overlays_dir(&self) -> Option<PathBuf> {
        Some(self.overlays_dir.clone())
    }
}

/// A fixed list of directories (tests, and `weaver route list --dir`).
pub struct DirsSource {
    pub candidates: Vec<Candidate>,
    pub overlays: Option<PathBuf>,
}

impl DirsSource {
    /// Directories only, no overlays.
    pub fn new(candidates: Vec<Candidate>) -> Self {
        Self { candidates, overlays: None }
    }
}

impl RouteSource for DirsSource {
    fn candidates(&self) -> Vec<Candidate> {
        self.candidates.iter().take(MAX_DIRS).cloned().collect()
    }

    fn extra_watch(&self) -> Vec<PathBuf> {
        self.overlays.iter().cloned().collect()
    }

    fn overlays_dir(&self) -> Option<PathBuf> {
        self.overlays.clone()
    }
}

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok().and_then(|m| m.modified().ok().map(|t| (t, m.len())));
    (path.to_path_buf(), meta)
}

fn overlay_of(source: &dyn RouteSource, c: &Candidate) -> Option<PathBuf> {
    Some(overlay_path(&source.overlays_dir()?, c.ulid.as_deref()?))
}

/// The fingerprint of a source: one stamp per candidate `ports.yaml` and
/// overlay plus the extra watched paths. Equal fingerprints mean nothing to reload.
pub fn fingerprint(source: &dyn RouteSource) -> Vec<Stamp> {
    let mut out: Vec<Stamp> = Vec::new();
    for c in source.candidates() {
        out.push(stamp(&c.dir.join(PORTS_FILE)));
        if let Some(p) = overlay_of(source, &c) {
            out.push(stamp(&p));
        }
    }
    out.extend(source.extra_watch().iter().map(|p| stamp(p)));
    out
}

/// A small regular file's text, `None` when absent, too large or unreadable.
fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_PORTS_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Read and admit every candidate's routes, in order: its `ports.yaml`, then
/// its overlay (registered projects only).
pub fn load(source: &dyn RouteSource) -> RouteTable {
    let mut projects: Vec<ProjectRoutes> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for c in source.candidates() {
        if seen.contains(&c.dir) {
            continue;
        }
        let repo = read_small(&c.dir.join(PORTS_FILE)).map(|text| parse_ports_yaml(&text, &c.name, &c.dir));
        let overlay = overlay_of(source, &c).and_then(|p| read_small(&p));
        if repo.is_none() && overlay.is_none() {
            continue;
        }
        seen.push(c.dir.clone());
        let mut pr = repo.unwrap_or_else(|| ProjectRoutes::empty(&c.name, &c.dir));
        pr.info.ulid = c.ulid.clone();
        if let Some(text) = overlay {
            let (routes, refused) = parse_overlay(&text, &pr.info.slug);
            pr.routes.extend(routes);
            pr.refused.extend(refused);
        }
        projects.push(pr);
    }
    RouteTable::build(projects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router_routes::Source;

    fn write(dir: &Path, text: &str) {
        std::fs::create_dir_all(dir.join("compose")).unwrap();
        std::fs::write(dir.join(PORTS_FILE), text).unwrap();
    }

    fn cand(name: &str, dir: &Path) -> Candidate {
        Candidate { name: name.into(), dir: dir.to_path_buf(), ulid: None }
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
        let src = DirsSource::new(vec![cand("A", &a), cand("B", &b), cand("C Proj", &c)]);
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
        let src = DirsSource::new(vec![cand("a", &a)]);
        let f1 = fingerprint(&src);
        assert_eq!(f1, fingerprint(&src));
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&a, "routes:\n  - { port: 3001 }\n  - { prefix: /a/two, port: 3003 }\n");
        assert_ne!(f1, fingerprint(&src));
    }

    #[test]
    fn overlays_merge_after_the_repository_and_the_repository_wins_on_a_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        write(&a, "project: aa\nroutes:\n  - { prefix: /aa, port: 3001 }\n");
        let overlays = tmp.path().join("routes");
        std::fs::create_dir_all(&overlays).unwrap();
        let ulid = "01K00000000000000000000000";
        std::fs::write(overlay_path(&overlays, ulid), "routes:\n  - { prefix: /aa, port: 3005 }\n  - { prefix: /aa-admin, port: 3006, allow: [bob@example.com] }\n").unwrap();
        // A second registered project with no ports.yaml at all, overlay only.
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&b).unwrap();
        let ulid_b = "01K00000000000000000000001";
        std::fs::write(overlay_path(&overlays, ulid_b), "routes:\n  - { port: 3007 }\n").unwrap();
        let src = DirsSource {
            candidates: vec![Candidate { ulid: Some(ulid.into()), ..cand("a", &a) }, Candidate { ulid: Some(ulid_b.into()), ..cand("B Two", &b) }],
            overlays: Some(overlays.clone()),
        };
        let f1 = fingerprint(&src);
        let t = load(&src);
        let got: Vec<(&str, u16, Source)> = t.routes.iter().map(|r| (r.prefix.as_str(), r.port, r.source)).collect();
        assert_eq!(got, [("/aa-admin", 3006, Source::Dashboard), ("/b-two", 3007, Source::Dashboard), ("/aa", 3001, Source::Repo)]);
        assert_eq!(t.refused.len(), 1, "{:?}", t.refused);
        assert_eq!(t.refused[0].source, Source::Dashboard);
        assert!(t.refused[0].reason.contains("the repository wins"), "{}", t.refused[0].reason);
        assert_eq!(t.projects[0].ulid.as_deref(), Some(ulid));
        assert_eq!(t.projects[1].slug, "b-two");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(overlay_path(&overlays, ulid), "routes: []\n").unwrap();
        assert_ne!(f1, fingerprint(&src), "an overlay change is part of the fingerprint");
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
        let overlays = tmp.path().join("routes");
        let src = ManifestSource { manifests_dir: manifests.clone(), overlays_dir: overlays.clone() };
        let c = src.candidates();
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(c[0].name, "ws");
        assert_eq!(c[0].ulid.as_deref(), Some("01M479THA4EGEQT5B1CF6WE1SN"));
        assert_eq!(c[1].dir, repo);
        assert_eq!(c[1].ulid, None);
        assert_eq!(src.extra_watch(), vec![manifests, overlays]);
    }
}
