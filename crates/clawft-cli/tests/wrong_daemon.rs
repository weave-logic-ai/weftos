//! A daemon that is the wrong one must never be treated as "no daemon":
//! commands stop with the remedy instead of falling back to local work.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Output};

use clawft_rpc::handshake::{BoundVia, handshake_value};
use clawft_rpc::{Handshake, ProtoRange, Response};

const ID_A: &str = "01J0000000000000000000000A";
const ID_B: &str = "01J0000000000000000000000B";

/// Fake daemon serving a handshake for `project` on `<dir>/kernel.sock`.
fn fake_daemon(dir: &Path, project: Option<&str>) {
    let hs = Handshake {
        proto: ProtoRange::supported(),
        node_id: "n".into(),
        user_id: None,
        project_id: project.map(String::from),
        bound_via: if project.is_some() { BoundVia::Project } else { BoundVia::None },
        depth: 0,
        parent: None,
        runtime_dir: dir.display().to_string(),
        pid: 1,
        version: "0.8.1".into(),
        sha: "abcd1234".into(),
        binary: None,
    };
    let l = UnixListener::bind(dir.join("kernel.sock")).unwrap();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let hs = hs.clone();
            std::thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                for line in BufReader::new(s).lines().map_while(Result::ok) {
                    let _ = line;
                    let mut out =
                        serde_json::to_string(&Response::success(handshake_value(&hs))).unwrap();
                    out.push('\n');
                    if w.write_all(out.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
}

fn weft(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_weft"))
        .args(args)
        .env("HOME", home)
        .env("WEFTOS_MANIFESTS_DIR", home.join("manifests"))
        .env_remove("WEFTOS_RUNTIME_DIR")
        .env_remove("WEFTOS_PROJECT")
        .env("CLAWFT_CONFIG", home.join("none.json"))
        .env("RUST_LOG", "off")
        .output()
        .unwrap()
}

fn short_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("wd")
        .tempdir_in(std::env::temp_dir())
        .unwrap()
}

#[test]
fn state_changes_stop_on_a_wrong_project_daemon() {
    let home = tempfile::tempdir().unwrap();
    let run = short_dir();
    fake_daemon(run.path(), Some(ID_B));
    let ws = home.path().join("wsroot");
    std::fs::create_dir_all(&ws).unwrap();
    let r = run.path().to_str().unwrap();
    let ws_s = ws.to_str().unwrap();
    for args in [
        vec!["--project", ID_A, "--runtime", r, "workspace", "create", "foo", "--dir", ws_s],
        vec!["--project", ID_A, "--runtime", r, "workspace", "config", "set", "a.b", "1"],
        vec!["--project", ID_A, "--runtime", r, "workspace", "load", "foo"],
    ] {
        let o = weft(home.path(), &args);
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{args:?} must fail: {err}");
        assert!(err.contains(ID_B) && err.contains(&format!("--project {ID_A}")), "{err}");
        assert!(!err.contains("no kernel reachable"), "{err}");
    }
    assert!(!ws.join("foo").exists(), "no local workspace may be created");
    assert!(!home.path().join(".clawft/workspaces.json").exists());
}

#[test]
fn unbound_daemon_on_a_runtime_flag_is_wrong_too() {
    let home = tempfile::tempdir().unwrap();
    let run = short_dir();
    fake_daemon(run.path(), None);
    let o = weft(
        home.path(),
        &["--project", ID_A, "--runtime", run.path().to_str().unwrap(), "workspace", "list"],
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{err}");
    assert!(err.contains("serves none"), "{err}");
}

#[test]
fn runtime_flag_reaches_env_readers() {
    // `socket_path()` reads WEFTOS_RUNTIME_DIR; `--runtime` must be visible to it.
    let home = tempfile::tempdir().unwrap();
    let run = short_dir();
    let r = run.path().to_str().unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_weft"))
        .args(["--runtime", r, "mcp-server", "--attach"])
        .env("HOME", home.path())
        .env_remove("WEFTOS_RUNTIME_DIR")
        .env("CLAWFT_CONFIG", home.path().join("none.json"))
        .env("RUST_LOG", "off")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(all.contains(&format!("{r}/kernel.sock")), "{all}");
}
