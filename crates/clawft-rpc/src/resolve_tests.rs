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

/// Write a manifest for `ID_A` rooted at `root` with `[serve] runtime_dir`.
fn write_manifest_for(w: &World, root: &Path, rt: &str) {
    let mdir = manifests_dir(&w.home);
    fs::create_dir_all(&mdir).unwrap();
    fs::write(
        mdir.join(format!("{ID_A}.toml")),
        format!(
            "schema = 1\nid = \"{ID_A}\"\nname = \"app\"\nroot = \"{}\"\n\
             created = 2026-01-01T00:00:00Z\nlast_seen = 2026-01-01T00:00:00Z\n\
             [serve]\nruntime_dir = \"{rt}\"\n",
            root.display()
        ),
    )
    .unwrap();
}

#[test]
fn clone_in_another_dir_does_not_reuse_the_original_kernel() {
    // Manifest root is the original; the clone has the same project.toml id.
    let w = world(Some("/rt/manifest"));
    let clone = w.home.join("work").join("app-clone");
    fs::create_dir_all(clone.join(".weftos")).unwrap();
    fs::copy(w.proj.join(".weftos/project.toml"), clone.join(".weftos/project.toml")).unwrap();
    let mut i = inputs(&w);
    i.cwd = Some(clone);
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.source, ResolveSource::Default, "override must not apply");
    let note = &r.tried.iter().find(|a| a.level == ResolveSource::Manifest).unwrap().detail;
    assert!(note.contains("ignored runtime_dir override"), "{note}");
    // The original still gets its override.
    assert_eq!(resolve_with(&inputs(&w)).unwrap().source, ResolveSource::Manifest);
}

#[test]
fn relative_manifest_runtime_dir_is_rejected() {
    let w = world(None);
    write_manifest_for(&w, &w.proj, "rel/run");
    let r = resolve_with(&inputs(&w)).unwrap();
    assert_eq!(r.source, ResolveSource::Default);
    let note = &r.tried.iter().find(|a| a.level == ResolveSource::Manifest).unwrap().detail;
    assert!(note.contains("relative"), "{note}");
}

#[cfg(unix)]
#[test]
fn symlinked_home_still_stops_the_walk() {
    let w = world(None);
    // HOME itself carries a project.toml; it is reached through a symlink
    // while the injected home is the real path.
    fs::create_dir_all(w.home.join(".weftos")).unwrap();
    fs::write(
        w.home.join(".weftos/project.toml"),
        format!("schema = 1\nid = \"{ID_B}\"\nname = \"home\"\ncreated = 2026-01-01T00:00:00Z\n"),
    )
    .unwrap();
    let link = w._dir.path().join("homelink");
    std::os::unix::fs::symlink(&w.home, &link).unwrap();
    let i = ResolveInputs {
        cwd: Some(link.join("elsewhere")),
        home: Some(w.home.clone()),
        ..ResolveInputs::default()
    };
    assert_eq!(resolve_with(&i).unwrap().project_id, None);
}

/// Write `ID_A`'s manifest with an arbitrary `[serve]` body.
fn write_manifest_serve(w: &World, serve_body: &str) {
    let mdir = manifests_dir(&w.home);
    fs::create_dir_all(&mdir).unwrap();
    fs::write(
        mdir.join(format!("{ID_A}.toml")),
        format!(
            "schema = 1\nid = \"{ID_A}\"\nname = \"app\"\nroot = \"{}\"\n\
             created = 2026-01-01T00:00:00Z\nlast_seen = 2026-01-01T00:00:00Z\n\
             [serve]\n{serve_body}\n",
            w.proj.display()
        ),
    )
    .unwrap();
}

/// Like [`world`], with an arbitrary `[serve]` body.
fn world_serve(serve_body: &str) -> World {
    let w = world(None);
    write_manifest_serve(&w, serve_body);
    w
}

/// Manifest for `ID_A` with `[serve] via = "user-daemon"` and no runtime_dir,
/// as `weft project init` writes it.
fn write_user_daemon_manifest(w: &World) {
    write_manifest_serve(w, "via = \"user-daemon\"");
}

#[test]
fn child_kernel_manifest_resolves_to_the_run_dir_and_asks_for_ensure_running() {
    let w = world_serve("via = \"child-kernel\"");
    let r = resolve_with(&inputs(&w)).unwrap();
    let run = w.home.join(".weftos/run").join(ID_A);
    assert_eq!(r.source, ResolveSource::Manifest);
    assert_eq!(r.socket, run.join("kernel.sock"));
    assert_eq!(r.runtime_root, run);
    let e = r.ensure.expect("a supervised child is started on demand");
    assert_eq!(e.project_id, ID_A);
    // The default parent socket of a child is `<run>/../kernel.sock`: the
    // user daemon's own socket.
    assert_eq!(e.user_socket, w.home.join(".weftos/run/kernel.sock"));
    assert_eq!(e.user_socket, run.parent().unwrap().join("kernel.sock"));
}

#[test]
fn an_explicit_endpoint_never_starts_a_child() {
    let w = world_serve("via = \"child-kernel\"");
    let mut i = inputs(&w);
    i.env_runtime = Some("/rt/env".into());
    assert!(resolve_with(&i).unwrap().ensure.is_none());
    let mut i = inputs(&w);
    i.flags.runtime = Some("/rt/flag".into());
    assert!(resolve_with(&i).unwrap().ensure.is_none());
}

#[test]
fn runtime_dir_override_beats_child_kernel() {
    let w = world_serve("via = \"child-kernel\"\nruntime_dir = \"/rt/pinned\"");
    let r = resolve_with(&inputs(&w)).unwrap();
    assert_eq!(r.socket, PathBuf::from("/rt/pinned/kernel.sock"));
    assert!(r.ensure.is_none());
}

#[test]
fn user_daemon_via_has_no_ensure() {
    let w = world_serve("via = \"user-daemon\"");
    let r = resolve_with(&inputs(&w)).unwrap();
    assert!(r.ensure.is_none());
    assert_eq!(r.source, ResolveSource::Manifest);
    assert_eq!(r.runtime_root, user_runtime_root(&w.home));
}

#[test]
fn a_copied_project_never_reuses_the_originals_child() {
    // The manifest's root is another directory than this tree's: the copy
    // must not be pointed at the original's kernel.
    let w = world_serve("via = \"child-kernel\"");
    let mdir = manifests_dir(&w.home);
    let p = mdir.join(format!("{ID_A}.toml"));
    let text = fs::read_to_string(&p).unwrap().replace(
        &format!("root = \"{}\"", w.proj.display()),
        "root = \"/somewhere/else\"",
    );
    fs::write(&p, text).unwrap();
    let r = resolve_with(&inputs(&w)).unwrap();
    assert!(r.ensure.is_none());
    assert_eq!(r.source, ResolveSource::Default);
}

fn make_user_root(w: &World, file: &str) -> PathBuf {
    let root = user_runtime_root(&w.home);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(file), b"").unwrap();
    root
}

#[test]
fn manifest_via_user_daemon_resolves_to_the_user_root() {
    let w = world(None);
    write_user_daemon_manifest(&w);
    let r = resolve_with(&inputs(&w)).unwrap();
    assert_eq!(r.source, ResolveSource::Manifest);
    assert_eq!(r.runtime_root, user_runtime_root(&w.home));
    assert_eq!(r.socket, user_runtime_root(&w.home).join(SOCKET_NAME));
    assert_eq!(r.project_id.as_deref(), Some(ID_A));
}

#[test]
fn project_flag_with_user_daemon_manifest_resolves_to_the_user_root() {
    let w = world(None);
    write_user_daemon_manifest(&w);
    let mut i = ResolveInputs {
        cwd: Some(w.home.clone()),
        home: Some(w.home.clone()),
        ..ResolveInputs::default()
    };
    i.flags.project = Some(ID_A.into());
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.source, ResolveSource::Manifest);
    assert_eq!(r.runtime_root, user_runtime_root(&w.home));
}

#[test]
fn explicit_runtime_runtime_dir_still_beats_user_daemon_manifest() {
    let w = world(None);
    write_user_daemon_manifest(&w);
    let mut i = inputs(&w);
    i.env_runtime = Some("/rt/env".into());
    assert_eq!(resolve_with(&i).unwrap().socket, PathBuf::from("/rt/env/kernel.sock"));
    i.flags.runtime = Some("/rt/flag".into());
    let r = resolve_with(&i).unwrap();
    assert_eq!(r.source, ResolveSource::Flag);
    assert_eq!(r.socket, PathBuf::from("/rt/flag/kernel.sock"));
}

#[test]
fn no_project_and_user_root_present_resolves_to_the_user_root() {
    for file in [SOCKET_NAME, LOCK_FILE_NAME] {
        let w = world(None);
        let root = make_user_root(&w, file);
        let i = ResolveInputs {
            cwd: Some(w.home.clone()),
            home: Some(w.home.clone()),
            ..ResolveInputs::default()
        };
        let r = resolve_with(&i).unwrap();
        assert_eq!(r.project_id, None);
        assert_eq!(r.source, ResolveSource::Default);
        assert_eq!(r.runtime_root, root, "{file}");
        let text = r.tried.last().unwrap().detail.clone();
        assert!(text.contains("user daemon root"), "{text}");
        assert!(text.contains("else runtime root"), "{text}");
    }
}

#[test]
fn no_project_and_no_user_root_keeps_the_phase0_default() {
    let w = world(None);
    let i = ResolveInputs {
        cwd: Some(w.home.clone()),
        home: Some(w.home.clone()),
        ..ResolveInputs::default()
    };
    let r = resolve_with(&i).unwrap();
    let want = RuntimePaths::resolve_with(None, Some(&w.home), Some(&w.home));
    assert_eq!(r.runtime_root, want.root());
    assert_eq!(r.source, ResolveSource::Default);
    let text = r.tried.last().unwrap().detail.clone();
    assert!(text.contains("no user daemon at"), "{text}");
}

#[test]
fn known_project_without_user_daemon_manifest_ignores_the_user_root() {
    // A project with no manifest keeps the Phase 0 answer even when a user
    // root exists: only `via = "user-daemon"` opts in.
    let w = world(None);
    make_user_root(&w, SOCKET_NAME);
    let r = resolve_with(&inputs(&w)).unwrap();
    let want = RuntimePaths::resolve_with(None, Some(&w.proj), Some(&w.home));
    assert_eq!(r.runtime_root, want.root());
}

#[cfg(unix)]
#[tokio::test]
async fn socket_readers_follow_a_manifest_runtime_dir_without_env() {
    let w = world(None);
    let rt = w._dir.path().join("rt-manifest");
    fs::create_dir_all(&rt).unwrap();
    write_manifest_for(&w, &w.proj, rt.to_str().unwrap());
    let i = inputs(&w); // no env, no flags: only the manifest names the endpoint
    assert_eq!(socket_for(&i), rt.join("kernel.sock"));
    assert!(!crate::is_daemon_running_for(&i).await);
    let _live = tokio::net::UnixListener::bind(rt.join("kernel.sock")).unwrap();
    assert!(crate::is_daemon_running_for(&i).await, "must dial the manifest endpoint");
}
