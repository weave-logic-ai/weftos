//! `weft project` against a temp HOME. No daemon, never the real `~/.weftos`.

use std::path::Path;
use std::process::{Command, Output};

struct Sandbox {
    home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn dir(&self, name: &str) -> std::path::PathBuf {
        let p = self.home.path().join(name);
        std::fs::create_dir_all(&p).unwrap();
        p.canonicalize().unwrap()
    }

    fn weft(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_weft"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", self.home.path())
            .env("WEFTOS_MANIFESTS_DIR", self.home.path().join("manifests"))
            .env("WEFTOS_RUNTIME_DIR", self.home.path().join("run"))
            .env("CLAWFT_CONFIG", self.home.path().join("none.json"))
            .env("RUST_LOG", "off")
            .output()
            .expect("run weft")
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let o = self.weft(cwd, args);
        assert!(
            o.status.success(),
            "weft {args:?} failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    fn err(&self, cwd: &Path, args: &[&str]) -> String {
        let o = self.weft(cwd, args);
        assert!(!o.status.success(), "weft {args:?} should fail");
        String::from_utf8_lossy(&o.stderr).into_owned()
    }
}

fn id_of(json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    v[0]["id"].as_str().unwrap().to_owned()
}

#[test]
fn init_is_idempotent_and_reports_id_root_manifest() {
    let sb = Sandbox::new();
    let proj = sb.dir("alpha");
    let out = sb.ok(&proj, &["project", "init", "--name", "alpha"]);
    assert!(out.contains("id:") && out.contains("manifest:"), "{out}");
    assert!(proj.join(".weftos/project.toml").exists());
    let first = id_of(&sb.ok(&proj, &["project", "list", "--json"]));
    assert!(out.contains(&first));

    sb.ok(&proj, &["project", "init"]);
    let listing = sb.ok(&proj, &["project", "list", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&listing).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(id_of(&listing), first);
}

#[test]
fn init_from_a_subdirectory_uses_the_project_root() {
    let sb = Sandbox::new();
    let proj = sb.dir("alpha");
    sb.ok(&proj, &["project", "init"]);
    let sub = proj.join("src/deep");
    std::fs::create_dir_all(&sub).unwrap();
    sb.ok(&sub, &["project", "init"]);
    assert!(!sub.join(".weftos").exists());
}

#[test]
fn init_refuses_home_itself() {
    let sb = Sandbox::new();
    let home = sb.home.path().canonicalize().unwrap();
    let e = sb.err(&home, &["project", "init"]);
    assert!(e.contains("home directory"), "{e}");
}

#[test]
fn init_updates_an_existing_gitignore_only() {
    let sb = Sandbox::new();
    let with = sb.dir("with");
    std::fs::write(with.join(".gitignore"), "target/").unwrap();
    sb.ok(&with, &["project", "init"]);
    let gi = std::fs::read_to_string(with.join(".gitignore")).unwrap();
    assert!(
        gi.contains("target/\n.weftos/chain/\n.weftos/project.key\n.weftos/project.cert.json\n.weftos/state/\n"),
        "{gi}"
    );
    let without = sb.dir("without");
    sb.ok(&without, &["project", "init"]);
    assert!(!without.join(".gitignore").exists());
}

#[test]
fn clone_is_refused_and_fork_gives_a_new_identity() {
    let sb = Sandbox::new();
    let orig = sb.dir("orig");
    sb.ok(&orig, &["project", "init", "--name", "orig"]);
    let orig_id = id_of(&sb.ok(&orig, &["project", "list", "--json"]));

    let copy = sb.dir("copy");
    std::fs::create_dir_all(copy.join(".weftos")).unwrap();
    std::fs::copy(
        orig.join(".weftos/project.toml"),
        copy.join(".weftos/project.toml"),
    )
    .unwrap();

    let e = sb.err(&copy, &["project", "init"]);
    assert!(e.contains("--fork") && e.contains(&orig_id), "{e}");

    let out = sb.ok(&copy, &["project", "init", "--fork", "--name", "copy"]);
    assert!(out.contains("forked"), "{out}");
    let listing: serde_json::Value =
        serde_json::from_str(&sb.ok(&copy, &["project", "list", "--json"])).unwrap();
    let ids: Vec<_> = listing
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&orig_id.as_str()));
    let toml = std::fs::read_to_string(copy.join(".weftos/project.toml")).unwrap();
    assert!(toml.contains(&orig_id), "parent recorded: {toml}");
}

#[test]
fn fork_of_the_registered_home_needs_force() {
    let sb = Sandbox::new();
    let p = sb.dir("p");
    sb.ok(&p, &["project", "init"]);
    let e = sb.err(&p, &["project", "init", "--fork"]);
    assert!(e.contains("registered home"), "{e}");
    sb.ok(&p, &["project", "init", "--fork", "--force"]);
}

#[test]
fn force_requires_fork() {
    let sb = Sandbox::new();
    let p = sb.dir("p");
    let e = sb.err(&p, &["project", "init", "--force"]);
    assert!(e.contains("--fork"), "{e}");
}

#[test]
fn show_by_id_name_and_here() {
    let sb = Sandbox::new();
    let p = sb.dir("proj");
    sb.ok(&p, &["project", "init", "--name", "proj"]);
    let id = id_of(&sb.ok(&p, &["project", "list", "--json"]));
    for args in [
        vec!["project", "show"],
        vec!["project", "show", "--here"],
        vec!["project", "show", "."],
        vec!["project", "show", "proj"],
        vec!["project", "show", id.as_str()],
    ] {
        let out = sb.ok(&p, &args);
        assert!(out.contains(&id), "{args:?}: {out}");
        assert!(out.contains("not verified"), "no daemon expected: {out}");
    }
    let j: serde_json::Value =
        serde_json::from_str(&sb.ok(&p, &["project", "show", "--json"])).unwrap();
    assert_eq!(j["id"], id);
    assert!(
        j["daemon"]["error"]
            .as_str()
            .unwrap()
            .contains("weaver kernel start")
    );
}

#[test]
fn show_ambiguous_name_lists_ids() {
    let sb = Sandbox::new();
    let a = sb.dir("a");
    let b = sb.dir("b");
    sb.ok(&a, &["project", "init", "--name", "same"]);
    sb.ok(&b, &["project", "init", "--name", "same"]);
    let e = sb.err(&a, &["project", "show", "same"]);
    assert!(e.contains("matches 2 projects"), "{e}");
}

#[test]
fn show_outside_a_project_says_what_to_do() {
    let sb = Sandbox::new();
    let d = sb.dir("nothing");
    let e = sb.err(&d, &["project", "show"]);
    assert!(e.contains("weft project init"), "{e}");
}

#[test]
fn seed_then_list_marks_missing() {
    let sb = Sandbox::new();
    let live = sb.dir("live");
    let gone = sb.home.path().join("gone");
    let reg = format!(
        r#"{{"version":1,"workspaces":[
            {{"name":"live","path":{live:?},"last_accessed":null,"created_at":"2026-01-01T00:00:00Z"}},
            {{"name":"gone","path":{gone:?},"last_accessed":null,"created_at":"2026-01-01T00:00:00Z"}}]}}"#,
        live = live.display().to_string(),
        gone = gone.display().to_string(),
    );
    std::fs::create_dir_all(sb.home.path().join(".clawft")).unwrap();
    std::fs::write(sb.home.path().join(".clawft/workspaces.json"), reg).unwrap();

    let out = sb.ok(sb.home.path(), &["project", "seed"]);
    assert!(out.contains("seeded:"), "{out}");
    let table = sb.ok(sb.home.path(), &["project", "list"]);
    assert!(table.contains("live") && table.contains("gone"), "{table}");
    let listing: serde_json::Value =
        serde_json::from_str(&sb.ok(sb.home.path(), &["project", "list", "--json"])).unwrap();
    let status = |n: &str| {
        listing
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == n)
            .map(|m| m["status"].as_str().unwrap().to_owned())
    };
    assert_eq!(status("live").as_deref(), Some("active"));
    assert_eq!(status("gone").as_deref(), Some("missing"));

    // Seeding again changes nothing; init adopts the seeded id.
    let again = sb.ok(sb.home.path(), &["project", "seed"]);
    assert!(again.contains("0 created"), "{again}");
    let seeded_id = listing
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "live")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    sb.ok(&live, &["project", "init"]);
    let toml = std::fs::read_to_string(live.join(".weftos/project.toml")).unwrap();
    assert!(toml.contains(&seeded_id), "{toml}");
}

#[test]
fn list_empty_points_at_init_and_seed() {
    let sb = Sandbox::new();
    let out = sb.ok(sb.home.path(), &["project", "list"]);
    assert!(
        out.contains("weft project init") && out.contains("seed"),
        "{out}"
    );
}

#[test]
fn global_flags_parse_and_validate() {
    let sb = Sandbox::new();
    let d = sb.dir("d");
    let e = sb.err(&d, &["--project", "not-a-ulid", "project", "list"]);
    assert!(e.contains("ULID"), "{e}");
    // Valid ULID and --runtime accepted anywhere on the line.
    sb.ok(
        &d,
        &[
            "project",
            "list",
            "--project",
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "--runtime",
            "/nonexistent",
        ],
    );
}

#[test]
fn init_refuses_ancestors_of_home_and_state_dirs() {
    let sb = Sandbox::new();
    let parent = sb.home.path().canonicalize().unwrap();
    let home = sb.dir("h");
    let run = |cwd: &Path| {
        Command::new(env!("CARGO_BIN_EXE_weft"))
            .args(["project", "init"])
            .current_dir(cwd)
            .env("HOME", &home)
            .env("WEFTOS_MANIFESTS_DIR", parent.join("manifests"))
            .env("RUST_LOG", "off")
            .output()
            .unwrap()
    };
    let o = run(&parent);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("parent of your home"));
    let state = home.join(".weftos/x");
    std::fs::create_dir_all(&state).unwrap();
    let o = run(&state);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("state directories"));
    assert!(!state.join(".weftos").exists());
}

#[test]
fn init_from_a_subdirectory_says_it_used_the_enclosing_root() {
    let sb = Sandbox::new();
    let proj = sb.dir("alpha");
    sb.ok(&proj, &["project", "init"]);
    let sub = proj.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    let o = sb.weft(&sub, &["project", "init"]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("enclosing project root"));
}
