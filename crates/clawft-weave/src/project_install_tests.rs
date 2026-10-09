use super::*;
use std::path::Path;
use std::sync::Arc;

const ULID: &str = "01K6ZQ8N3T4V5W6X7Y8Z9A0B1C";

fn req(json: serde_json::Value) -> Result<InstallRequest, serde_json::Error> {
    serde_json::from_value(json)
}

fn base() -> serde_json::Value {
    serde_json::json!({"project_ulid": ULID, "slug": "demo",
        "sources": [{"url": "https://github.com/org/demo.git", "branch": "main"}]})
}

#[test]
fn default_path_is_home_projects_slug() {
    let home = Path::new("/Users/m");
    assert_eq!(req(base()).unwrap().validate(home).unwrap(), Path::new("/Users/m/Projects/demo"));
}

#[test]
fn tilde_and_absolute_paths_inside_home_are_accepted() {
    let home = Path::new("/Users/m");
    let mut v = base();
    v["target_path"] = "~/code/demo".into();
    assert_eq!(req(v.clone()).unwrap().validate(home).unwrap(), Path::new("/Users/m/code/demo"));
    v["target_path"] = "/Users/m/x".into();
    assert!(req(v).unwrap().validate(home).is_ok());
}

#[test]
fn paths_outside_home_or_with_dotdot_are_refused() {
    let home = Path::new("/Users/m");
    for p in ["/etc/x", "~/../other", "relative/x", "~/", "/Users/m", "/Users/mallory/x"] {
        let mut v = base();
        v["target_path"] = p.into();
        assert_eq!(req(v).unwrap().validate(home), Err(InstallRequestError::Path), "{p}");
    }
}

#[test]
fn bad_urls_are_refused() {
    let home = Path::new("/Users/m");
    for u in ["https://user:tok@github.com/o/r", "https://tok@github.com/o/r", "--upload-pack=x", "file:///etc", "ext::sh -c x", "https://", "http://github.com/o/r", "a b"] {
        let mut v = base();
        v["sources"][0]["url"] = u.into();
        assert!(matches!(req(v).unwrap().validate(home), Err(InstallRequestError::Source(0, _))), "{u}");
    }
    for u in ["git@github.com:org/r.git", "ssh://git@github.com/org/r", "https://github.com:443/o/r"] {
        let mut v = base();
        v["sources"][0]["url"] = u.into();
        assert!(req(v).unwrap().validate(home).is_ok(), "{u}");
    }
}

#[test]
fn dirs_branches_and_counts_are_checked() {
    let home = Path::new("/Users/m");
    let mut v = base();
    v["sources"][0]["dir"] = "../x".into();
    assert!(req(v).unwrap().validate(home).is_err());
    let mut v = base();
    v["sources"][0]["branch"] = "--force".into();
    assert!(req(v).unwrap().validate(home).is_err());
    let mut v = base();
    v["sources"] = serde_json::json!([{"url": "https://h/a"}, {"url": "https://h/b"}]);
    assert!(req(v).unwrap().validate(home).is_err(), "two sources on '.'");
    let mut v = base();
    v["sources"] = serde_json::Value::Array((0..9).map(|i| serde_json::json!({"url": "https://h/a", "dir": format!("r{i}")})).collect());
    assert_eq!(req(v).unwrap().validate(home), Err(InstallRequestError::TooManyRepos));
}

#[test]
fn ulid_slug_unknown_fields_and_empty_requests_are_refused() {
    let home = Path::new("/Users/m");
    let mut v = base();
    v["project_ulid"] = "not-a-ulid".into();
    assert_eq!(req(v).unwrap().validate(home), Err(InstallRequestError::Ulid));
    let mut v = base();
    v["slug"] = "Demo/../x".into();
    assert_eq!(req(v).unwrap().validate(home), Err(InstallRequestError::Slug));
    let mut v = base();
    v["extra"] = 1.into();
    assert!(req(v).is_err());
    let v = serde_json::json!({"project_ulid": ULID, "slug": "demo"});
    assert_eq!(req(v).unwrap().validate(home), Err(InstallRequestError::NothingToFetch));
    let v = serde_json::json!({"project_ulid": ULID, "sources": [{"url": "https://h/a"}]});
    assert_eq!(req(v).unwrap().validate(home), Err(InstallRequestError::Path), "no path and no slug");
    let v = serde_json::json!({"project_ulid": ULID, "target_path": "~/Projects/demo", "primary": {"node_id": "abc"}});
    assert!(req(v).unwrap().validate(home).is_ok(), "the dashboard's current payload plus a primary");
}

struct Fake(&'static str, bool);

#[async_trait]
impl ProjectFetcher for Fake {
    fn name(&self) -> &'static str {
        self.0
    }
    fn can_fetch(&self, _: &InstallRequest) -> bool {
        self.1
    }
    async fn fetch(&self, _: &InstallRequest, _: &Path) -> Result<FetchReport, String> {
        Ok(FetchReport { fetcher: self.0, ..Default::default() })
    }
}

#[test]
fn mesh_is_preferred_when_it_can_fetch() {
    let r = req(base()).unwrap();
    let both: Vec<Arc<dyn ProjectFetcher>> = vec![Arc::new(Fake("git-remote", true)), Arc::new(Fake("mesh", true))];
    assert_eq!(fetch_order(&both, &r).unwrap().name(), "mesh");
    let git_only: Vec<Arc<dyn ProjectFetcher>> = vec![Arc::new(Fake("git-remote", true)), Arc::new(Fake("mesh", false))];
    assert_eq!(fetch_order(&git_only, &r).unwrap().name(), "git-remote");
    let none: Vec<Arc<dyn ProjectFetcher>> = vec![Arc::new(Fake("mesh", false))];
    assert!(fetch_order(&none, &r).is_none());
}
