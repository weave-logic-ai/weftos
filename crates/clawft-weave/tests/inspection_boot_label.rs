//! ADR-103 Phase 0 R5: with no daemon running, `weaver kernel status|ps|
//! services|logs` boot a throwaway inspection kernel. Its output must say
//! so, on stdout (so a pipe or a capture still carries it) and on stderr.
//!
//! The binary runs with a cleared environment pointing at throwaway dirs
//! only: nothing under the real home or runtime root is read or written.
#![cfg(unix)]

use std::process::Command;

fn run(view: &str) -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let run = dir.path().join("run");
    std::fs::create_dir_all(&home).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_weaver"))
        .args(["kernel", view])
        .env_clear()
        .env("HOME", &home)
        .env("WEFTOS_RUNTIME_DIR", &run)
        .current_dir(dir.path())
        .output()
        .expect("run weaver");
    assert!(out.status.success(), "{view}: {out:?}");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn every_inspection_view_labels_itself_as_not_the_daemon() {
    for (view, tag) in [
        ("status", "status"),
        ("ps", "processes"),
        ("services", "services"),
        ("logs", "boot log"),
    ] {
        let (stdout, stderr) = run(view);
        let first = stdout.lines().next().unwrap_or_default();
        assert_eq!(
            first,
            format!("[ephemeral inspection kernel, not the daemon: {tag}]"),
            "{view} stdout: {stdout}"
        );
        assert!(stderr.contains("NOT the daemon"), "{view} stderr: {stderr}");
        assert!(stderr.contains("mesh and chain off"), "{view} stderr: {stderr}");
    }
}
