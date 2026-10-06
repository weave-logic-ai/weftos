//! Workspace report tests: real temp git repositories, never the user's
//! checkouts, `~/.weftos` or the real dashboard.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::*;
use crate::dashboard_test_support::*;

const HB: &str = "/api/nodes/heartbeat";

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A repository with one commit on `main` and the given origin.
fn repo(dir: &Path, remote: Option<&str>) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "first"]);
    if let Some(r) = remote {
        git(dir, &["remote", "add", "origin", r]);
    }
}

fn projects(root: &Path) -> Vec<(String, std::path::PathBuf)> {
    vec![(ULID.to_owned(), root.to_path_buf())]
}

#[test]
fn credentials_are_stripped_from_every_remote_form() {
    let cases = [
        ("https://user:s3cret@github.com/org/repo.git", "https://github.com/org/repo.git"),
        ("https://ghp_token@github.com/org/repo.git", "https://github.com/org/repo.git"),
        ("https://github.com/org/repo.git?token=x#frag", "https://github.com/org/repo.git"),
        ("ssh://git:pw@host.example:2222/org/repo.git", "ssh://host.example:2222/org/repo.git"),
        ("git@github.com:org/repo.git", "github.com:org/repo.git"),
        ("/srv/git/repo.git", "/srv/git/repo.git"),
        ("../sibling", "../sibling"),
    ];
    for (raw, want) in cases {
        assert_eq!(sanitize_remote(raw).as_deref(), Some(want), "{raw}");
    }
    assert_eq!(sanitize_remote("  "), None);
}

#[test]
fn status_parsing_counts_files_and_reads_branch_and_divergence() {
    let out = b"# branch.oid 0123456789abcdef0123456789abcdef01234567\n# branch.head feature/x\n\
# branch.upstream origin/feature/x\n# branch.ab +3 -1\n1 .M N... 100644 100644 100644 a b src/a.rs\n\
2 R. N... 100644 100644 100644 a b R100 new.rs\told.rs\n? untracked secret-name.txt\nu UU N... 1 2 3 4 a b c conflicted\n";
    let f = parse_status(out);
    assert_eq!(f.branch.as_deref(), Some("feature/x"));
    assert_eq!(f.head.as_deref(), Some("01234567"));
    assert_eq!((f.ahead, f.behind, f.dirty), (Some(3), Some(1), 4));
    let json = serde_json::to_string(&f).unwrap();
    assert!(!json.contains("secret-name") && !json.contains("a.rs"), "no file names leave: {json}");

    let none = parse_status(b"# branch.oid (initial)\n# branch.head (detached)\n");
    assert_eq!((none.branch, none.head, none.ahead, none.behind, none.dirty), (None, None, None, None, 0));
}

#[test]
fn a_project_with_a_root_repo_and_a_nested_repo_is_reported() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("proj");
    repo(&root, Some("https://user:pw@example.com/org/proj.git"));
    repo(&root.join("sub"), None);
    // Dirty the root: one modified, two untracked (the nested repo is one more untracked entry).
    std::fs::write(root.join("a.txt"), "changed\n").unwrap();
    std::fs::write(root.join("u1"), "x").unwrap();
    std::fs::write(root.join("u2"), "x").unwrap();
    // A plain directory and a hidden one are not repositories.
    std::fs::create_dir_all(root.join("docs")).unwrap();

    let r = gather(&projects(&root));
    assert!(!r.truncated);
    assert_eq!(r.workspaces.len(), 1);
    let w = &r.workspaces[0];
    assert_eq!(w.ulid, ULID);
    assert_eq!(w.root, root.to_string_lossy());
    assert_eq!(w.git.len(), 2, "{:?}", w.git);
    let top = &w.git[0];
    assert_eq!(top.path, ".");
    assert_eq!(top.remote.as_deref(), Some("https://example.com/org/proj.git"));
    assert_eq!(top.branch.as_deref(), Some("main"));
    assert_eq!(top.head.as_ref().map(String::len), Some(8));
    assert_eq!(top.dirty, 4, "one modified, two untracked files, and the nested repo as an untracked entry");
    assert_eq!((top.ahead, top.behind), (None, None), "no upstream");
    let sub = &w.git[1];
    assert_eq!((sub.path.as_str(), sub.dirty, sub.remote.clone()), ("sub", 0, None));
    assert!(w.last_activity.as_deref().is_some_and(|t| t.contains('T')), "{:?}", w.last_activity);
}

#[test]
fn ahead_and_behind_are_read_against_the_upstream() {
    let d = tempfile::tempdir().unwrap();
    let origin = d.path().join("origin");
    repo(&origin, None);
    let clone = d.path().join("clone");
    git(d.path(), &["clone", "-q", origin.to_str().unwrap(), clone.to_str().unwrap()]);
    std::fs::write(clone.join("b.txt"), "b").unwrap();
    git(&clone, &["add", "."]);
    git(&clone, &["commit", "-q", "-m", "local"]);

    let r = gather(&projects(&clone));
    let f = &r.workspaces[0].git[0];
    assert_eq!((f.ahead, f.behind, f.dirty), (Some(1), Some(0), 0));
    // The remote is a local path here; it is reported as written (no userinfo to strip).
    assert_eq!(f.remote.as_deref(), Some(origin.to_str().unwrap()));
}

#[test]
fn a_project_without_git_reports_an_empty_list_and_a_missing_root_is_skipped_quietly() {
    let d = tempfile::tempdir().unwrap();
    let plain = d.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let r = gather(&[(ULID.to_owned(), plain), ("01K00000000000000000000001".into(), d.path().join("gone"))]);
    assert_eq!(r.workspaces.len(), 2);
    assert!(r.workspaces.iter().all(|w| w.git.is_empty() && w.last_activity.is_none()));
}

#[test]
fn caps_on_projects_and_repositories_set_the_flag() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("many");
    for i in 0..(MAX_REPOS + 2) {
        repo(&root.join(format!("r{i:02}")), None);
    }
    let r = gather(&projects(&root));
    assert_eq!(r.workspaces[0].git.len(), MAX_REPOS);
    assert!(r.truncated);

    let many: Vec<_> = (0..MAX_PROJECTS + 5).map(|i| (format!("P{i}"), d.path().join("none"))).collect();
    let r = gather(&many);
    assert_eq!(r.workspaces.len(), MAX_PROJECTS);
    assert!(r.truncated);
}

#[test]
fn the_report_is_trimmed_to_fit_the_size_limit_and_says_so() {
    let big = |i: usize| WorkspaceFacts {
        ulid: format!("P{i}"),
        root: "/x".repeat(100),
        git: (0..8)
            .map(|k| RepoFacts {
                path: format!("repo{k}"),
                remote: Some("https://example.com/".to_owned() + &"a".repeat(200)),
                branch: Some("main".into()),
                head: Some("deadbeef".into()),
                dirty: 1,
                ahead: None,
                behind: None,
            })
            .collect(),
        last_activity: None,
    };
    let mut report = serde_json::json!({ "host": "h", "observed": {} });
    attach(&mut report, WorkspaceReport { workspaces: (0..MAX_PROJECTS).map(big).collect(), truncated: false });
    assert!(report.to_string().len() <= MAX_REPORT_BYTES, "{}", report.to_string().len());
    assert_eq!(report["workspaces_truncated"], true);
    let kept = report["workspaces"].as_array().unwrap().len();
    assert!(kept > 0 && kept < MAX_PROJECTS, "{kept}");

    let mut small = serde_json::json!({ "host": "h" });
    attach(&mut small, WorkspaceReport::default());
    assert_eq!((small["workspaces"].as_array().unwrap().len(), &small["workspaces_truncated"]), (0, &Value::Bool(false)));
}

struct Fixed(Vec<(String, std::path::PathBuf)>);

#[async_trait]
impl WorkspaceSource for Fixed {
    async fn workspaces(&self) -> WorkspaceReport {
        let p = self.0.clone();
        tokio::task::spawn_blocking(move || gather(&p)).await.unwrap()
    }
}

#[tokio::test]
async fn the_heartbeat_carries_workspaces_and_the_switch_turns_them_off() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("proj");
    repo(&root, Some("https://tok:en@example.com/o/r.git"));
    let fake = FakeDash::start().await;
    fake.answer(HB, &[(200, "{}")]);
    let (_t, tpath) = token_dir(&token('a'));
    let src: Arc<dyn WorkspaceSource> = Arc::new(Fixed(projects(&root)));
    let mk = |on: bool| {
        let mut cfg = config(&fake.url, &tpath);
        cfg.report_workspaces = on;
        crate::dashboard_report::Dashboard::with_workspaces(
            cfg,
            Default::default(),
            Arc::new(Kids(vec![(ULID.into(), "running".into())])),
            Some(src.clone()),
        )
        .unwrap()
    };

    mk(true).heartbeat_once().await;
    let body: Value = serde_json::from_str(&fake.requests(HB)[0].body).unwrap();
    let w = &body["report"]["workspaces"][0];
    assert_eq!(w["ulid"], ULID);
    assert_eq!(w["git"][0]["remote"], "https://example.com/o/r.git");
    assert_eq!(w["git"][0]["branch"], "main");
    assert_eq!(body["report"]["workspaces_truncated"], false);
    let raw = &fake.requests(HB)[0].body;
    assert!(!raw.contains("tok:en") && !raw.contains("a.txt"), "no credentials or file names: {raw}");

    mk(false).heartbeat_once().await;
    let body: Value = serde_json::from_str(&fake.requests(HB)[1].body).unwrap();
    assert!(body["report"].get("workspaces").is_none());
}

#[tokio::test]
async fn manifest_index_projects_are_found_from_a_temp_manifests_dir() {
    let d = tempfile::tempdir().unwrap();
    let manifests = d.path().join("projects");
    std::fs::create_dir_all(&manifests).unwrap();
    let root = d.path().join("proj");
    repo(&root, None);
    let now = "2026-10-05T12:00:00Z";
    std::fs::write(
        manifests.join(format!("{ULID}.toml")),
        format!("schema = 1\nid = \"{ULID}\"\nname = \"proj\"\nroot = {:?}\ncreated = \"{now}\"\nlast_seen = \"{now}\"\n", root),
    )
    .unwrap();
    let r = ManifestWorkspaces { manifests_dir: manifests }.workspaces().await;
    assert_eq!(r.workspaces.len(), 1, "{r:?}");
    assert_eq!(r.workspaces[0].git[0].branch.as_deref(), Some("main"));
}
