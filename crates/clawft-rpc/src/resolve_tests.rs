//! Resolver precedence tests: every input is injected, nothing reads the
//! real HOME, env or cwd.

use super::*;
use std::fs;

const ID_A: &str = "01J0000000000000000000000A";
const ID_B: &str = "01J0000000000000000000000B";

struct World {
    _dir: tempfile::TempDir,
    home: PathBuf,
    proj: PathBuf,
}

/// Fake HOME with a project at `home/work/app` whose project.toml carries
/// `ID_A`. `serve_rt` adds a manifest with that `[serve] runtime_dir`.
fn world(serve_rt: Option<&str>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let proj = home.join("work").join("app");
    fs::create_dir_all(proj.join(".weftos")).unwrap();
    fs::write(
        proj.join(".weftos/project.toml"),
        format!("schema = 1\nid = \"{ID_A}\"\nname = \"app\"\ncreated = 2026-01-01T00:00:00Z\n"),
    )
    .unwrap();
    if let Some(rt) = serve_rt {
        let mdir = manifests_dir(&home);
        fs::create_dir_all(&mdir).unwrap();
        fs::write(
            mdir.join(format!("{ID_A}.toml")),
            format!(
                "schema = 1\nid = \"{ID_A}\"\nname = \"app\"\nroot = \"{}\"\n\
                 created = 2026-01-01T00:00:00Z\nlast_seen = 2026-01-01T00:00:00Z\n\
                 [serve]\nruntime_dir = \"{rt}\"\n",
                proj.display()
            ),
        )
        .unwrap();
    }
    World { _dir: dir, home, proj }
}

fn inputs(w: &World) -> ResolveInputs {
    ResolveInputs {
        cwd: Some(w.proj.clone()),
        home: Some(w.home.clone()),
        ..ResolveInputs::default()
    }
}

#[test]
fn flag_beats_env_beats_manifest_beats_default() {
    let w = world(Some("/rt/manifest"));
    let mut i = inputs(&w);

    // Manifest alone.
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.source, ResolveSource::Manifest);
    assert_eq!(r.socket, PathBuf::from("/rt/manifest/kernel.sock"));
    assert_eq!(r.project_id.as_deref(), Some(ID_A));

    // Env beats manifest.
    i.env_runtime = Some("/rt/env".into());
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.source, ResolveSource::Env);
    assert_eq!(r.socket, PathBuf::from("/rt/env/kernel.sock"));
    assert_eq!(r.project_id.as_deref(), Some(ID_A));

    // Flag beats env.
    i.flags.runtime = Some("/rt/flag".into());
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.source, ResolveSource::Flag);
    assert_eq!(r.socket, PathBuf::from("/rt/flag/kernel.sock"));
    assert_eq!(r.tried.iter().filter(|a| a.used).count(), 1);
}

#[test]
fn default_when_nothing_else_applies() {
    let w = world(None);
    let i = inputs(&w);
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.source, ResolveSource::Default);
    // Manifest-less project: default is the P0 resolver's answer.
    let want = RuntimePaths::resolve_with(None, Some(&w.proj), Some(&w.home));
    assert_eq!(r.socket, want.socket());
    assert_eq!(r.project_id.as_deref(), Some(ID_A));
    assert_eq!(r.tried.len(), 4);
}

#[test]
fn project_flag_and_env_choose_expected_project_and_manifest_runtime() {
    let w = world(Some("/rt/manifest"));
    // cwd outside the project: only the flag/env names it.
    let mut i = ResolveInputs {
        cwd: Some(w.home.clone()),
        home: Some(w.home.clone()),
        ..ResolveInputs::default()
    };
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.project_id, None);
    assert_eq!(r.source, ResolveSource::Default);

    i.env_project = Some(ID_A.into());
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.project_id.as_deref(), Some(ID_A));
    assert_eq!(r.source, ResolveSource::Manifest);
    assert_eq!(r.socket, PathBuf::from("/rt/manifest/kernel.sock"));

    // Flag project wins over env project.
    i.flags.project = Some(ID_B.into());
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.project_id.as_deref(), Some(ID_B));
    assert_eq!(r.source, ResolveSource::Default, "B has no manifest");
}

#[test]
fn home_is_never_a_project() {
    let w = world(Some("/rt/manifest"));
    // A project.toml directly in HOME must not be picked up.
    fs::create_dir_all(w.home.join(".weftos")).unwrap();
    fs::write(
        w.home.join(".weftos/project.toml"),
        format!("schema = 1\nid = \"{ID_B}\"\nname = \"home\"\ncreated = 2026-01-01T00:00:00Z\n"),
    )
    .unwrap();
    let i = ResolveInputs {
        cwd: Some(w.home.join("elsewhere")),
        home: Some(w.home.clone()),
        ..ResolveInputs::default()
    };
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.project_id, None);
    assert_eq!(r.source, ResolveSource::Default);
}

#[test]
fn invalid_project_is_an_error_naming_the_source() {
    let w = world(None);
    let mut i = inputs(&w);
    i.env_project = Some("../etc".into());
    let e = resolve_with(&i).unwrap_err();
    assert_eq!(
        e,
        ResolveError::InvalidProject {
            from: ResolveSource::Env,
            value: "../etc".into()
        }
    );
    assert!(e.to_string().contains("ULID"));
    i.env_project = None;
    i.flags.project = Some("nope".into());
    assert!(matches!(
        resolve_with(&i).unwrap_err(),
        ResolveError::InvalidProject { from: ResolveSource::Flag, .. }
    ));
}

#[test]
fn blank_env_counts_as_unset() {
    let w = world(None);
    let mut i = inputs(&w);
    i.env_runtime = Some("  ".into());
    i.env_project = Some(String::new());
    assert_eq!(resolve_with(&i).unwrap().source, ResolveSource::Default);
}

#[test]
fn display_lists_every_level_tried() {
    let w = world(Some("/rt/manifest"));
    let mut i = inputs(&w);
    i.env_runtime = Some("/rt/env".into());
    let r = resolve_with(&i).unwrap();
    let text = r.to_string();
    for needle in ["flag", "env", "manifest", "default", "/rt/env", "expected project"] {
        assert!(text.contains(needle), "{needle} missing in:\n{text}");
    }
    let msg = r.unreachable_message(&SocketState::Stale);
    assert!(msg.contains("stale socket"), "{msg}");
    assert!(msg.contains("tried, in order"), "{msg}");
    assert!(msg.contains("weaver kernel start"), "{msg}");
}
