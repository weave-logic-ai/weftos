//! Real git against temp bare repositories. `protocol.file.allow=never` is the
//! production setting, so these fetchers are built with a test-only runner that
//! allows `file`; one test shows the default runner refusing the same clone.

use super::*;
use crate::project_git::GitRunner;
use crate::project_install::SourceRepo;
use crate::project_install_test_support::*;

fn src(url: &Path, dir: &str, branch: Option<&str>) -> SourceRepo {
    SourceRepo { url: url.to_str().unwrap().into(), branch: branch.map(Into::into), dir: dir.into() }
}

fn req(sources: Vec<SourceRepo>) -> InstallRequest {
    InstallRequest { project_ulid: ULID.into(), target_path: None, slug: Some("demo".into()), sources, primary: None }
}

fn fetcher() -> GitRemoteFetcher {
    GitRemoteFetcher::with_runner(test_runner())
}

#[tokio::test]
async fn root_and_sibling_land_beside_each_other() {
    let t = tempfile::tempdir().unwrap();
    let a = bare_repo(t.path(), "a", "a.txt");
    let b = bare_repo(t.path(), "b", "b.txt");
    let target = t.path().join("home/Projects/demo");
    let r = fetcher().fetch(&req(vec![src(&a, ".", None), src(&b, "tools", Some("main"))]), &target).await.unwrap();
    assert_eq!(r.fetcher, "git-remote");
    assert!(target.join("a.txt").is_file());
    assert!(t.path().join("home/Projects/tools/b.txt").is_file());
    assert_eq!(r.repos.len(), 2);
    assert_eq!(r.repos[0].dir, ".");
    assert_eq!(r.repos[0].head.as_deref().map(str::len), Some(8));
    assert_eq!(r.repos[1].remote.as_deref(), b.to_str());
}

#[tokio::test]
async fn without_a_root_source_every_repo_goes_under_the_target() {
    let t = tempfile::tempdir().unwrap();
    let a = bare_repo(t.path(), "a", "a.txt");
    let b = bare_repo(t.path(), "b", "b.txt");
    let target = t.path().join("home/Projects/demo");
    fetcher().fetch(&req(vec![src(&a, "one", None), src(&b, "two", None)]), &target).await.unwrap();
    assert!(target.join("one/a.txt").is_file() && target.join("two/b.txt").is_file());
    assert!(!target.join(".git").exists(), "the target is a plain directory");
}

#[tokio::test]
async fn an_existing_empty_target_is_used_and_a_non_empty_one_refused() {
    let t = tempfile::tempdir().unwrap();
    let a = bare_repo(t.path(), "a", "a.txt");
    let empty = t.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    fetcher().fetch(&req(vec![src(&a, ".", None)]), &empty).await.unwrap();
    assert!(empty.join("a.txt").is_file());

    let full = t.path().join("full");
    std::fs::create_dir_all(&full).unwrap();
    std::fs::write(full.join("mine.txt"), "keep").unwrap();
    let e = fetcher().fetch(&req(vec![src(&a, ".", None)]), &full).await.unwrap_err();
    assert!(e.contains("not empty"), "{e}");
    assert_eq!(std::fs::read_to_string(full.join("mine.txt")).unwrap(), "keep");
}

#[tokio::test]
async fn a_non_empty_sibling_destination_is_refused_before_anything_is_cloned() {
    let t = tempfile::tempdir().unwrap();
    let a = bare_repo(t.path(), "a", "a.txt");
    let b = bare_repo(t.path(), "b", "b.txt");
    let sibling = t.path().join("home/tools");
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("mine.txt"), "keep").unwrap();
    let target = t.path().join("home/demo");
    let e = fetcher().fetch(&req(vec![src(&a, ".", None), src(&b, "tools", None)]), &target).await.unwrap_err();
    assert!(e.contains("not empty"), "{e}");
    assert!(!target.exists(), "root must not be cloned when a sibling is blocked");
}

#[tokio::test]
async fn a_sibling_dir_named_like_the_target_collides() {
    let t = tempfile::tempdir().unwrap();
    let a = bare_repo(t.path(), "a", "a.txt");
    let b = bare_repo(t.path(), "b", "b.txt");
    let target = t.path().join("home/demo");
    let e = fetcher().fetch(&req(vec![src(&a, ".", None), src(&b, "demo", None)]), &target).await.unwrap_err();
    assert!(e.contains("collides"), "{e}");
}

#[tokio::test]
async fn a_failed_clone_removes_only_what_this_install_created() {
    let t = tempfile::tempdir().unwrap();
    let a = bare_repo(t.path(), "a", "a.txt");
    let missing = t.path().join("nope.git");
    let home = t.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("other.txt"), "keep").unwrap();
    let target = home.join("Projects/demo");
    let e = fetcher().fetch(&req(vec![src(&a, ".", None), src(&missing, "tools", None)]), &target).await.unwrap_err();
    assert!(e.contains("tools"), "{e}");
    assert!(!target.exists(), "the root clone is rolled back");
    assert!(!home.join("Projects").exists(), "created parents are removed");
    assert!(home.join("other.txt").is_file(), "unrelated files stay");

    // An existing empty target survives, emptied.
    let empty = home.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    fetcher().fetch(&req(vec![src(&missing, ".", None)]), &empty).await.unwrap_err();
    assert!(empty.is_dir() && std::fs::read_dir(&empty).unwrap().next().is_none());
}

#[tokio::test]
async fn the_production_runner_refuses_file_urls() {
    let t = tempfile::tempdir().unwrap();
    let a = bare_repo(t.path(), "a", "a.txt");
    let target = t.path().join("home/demo");
    let hardened = GitRemoteFetcher::with_runner(GitRunner::default());
    let url = format!("file://{}", a.display());
    let r = req(vec![SourceRepo { url, branch: None, dir: ".".into() }]);
    let e = hardened.fetch(&r, &target).await.unwrap_err();
    assert!(e.contains("failed"), "{e}");
    assert!(!target.exists());
}

#[test]
fn can_fetch_needs_sources() {
    let f = GitRemoteFetcher::new();
    assert_eq!(f.name(), "git-remote");
    assert!(!f.can_fetch(&req(vec![])));
    assert!(f.can_fetch(&req(vec![src(Path::new("x"), ".", None)])));
}
