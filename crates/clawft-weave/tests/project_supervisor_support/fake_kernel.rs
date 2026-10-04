//! The test binary acting as a project kernel: `main` calls [`run`] when it
//! is started with the real child argv (`kernel start --foreground
//! --profile project --project <id>`). It holds `kernel.lock`, writes
//! `kernel.pid`, serves `kernel.handshake` and `kernel.shutdown` on
//! `kernel.sock`, and records what it was given (argv, environment, cwd,
//! `spawn.json`) for the tests to read. Behaviour comes from the file
//! `<run>/behavior`: `serve` (default), `crash`, `exit0`,
//! `crash-after-ready`, `ignore-shutdown`, `ignore-term` (ignores `kernel.shutdown`
//! and `SIGTERM`, writing `term.seen` when one arrives), `nolock`, `wrong-project`,
//! `wrong-pid`. Optional files: `start_delay_ms` (wait that long between writing
//! `kernel.pid` and binding the socket: a child still booting) and `sha`
//! (the build the handshake reports).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

use clawft_types::runtime_paths::RuntimePaths;
use clawft_weave::instance_lock::InstanceLock;
use serde_json::{Value, json};

pub fn run(args: &[String]) -> ! {
    let run = PathBuf::from(std::env::var("WEFTOS_RUNTIME_DIR").expect("WEFTOS_RUNTIME_DIR"));
    let id = std::env::var("WEFTOS_PROJECT_ID").unwrap_or_default();
    let _ = std::fs::write(run.join("argv.txt"), args.join("\n"));
    let env: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    let _ = std::fs::write(run.join("env.txt"), env.join("\n"));
    let _ = std::fs::write(
        run.join("cwd.txt"),
        std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
    );
    let _ = std::fs::copy(run.join("spawn.json"), run.join("spawn.seen.json"));
    let mode = std::fs::read_to_string(run.join("behavior"))
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "serve".into());
    if mode == "sandbox-probe" {
        let allowed = std::fs::read_to_string(run.join("probe-allowed-target")).unwrap();
        let allowed_read = std::fs::read_to_string(allowed.trim()).is_ok_and(|s| s == "project-only");
        // The target belongs to the test's temporary HOME. Only the probe
        // result is recorded; never log file content or a real user path.
        let target = std::fs::read_to_string(run.join("probe-target")).unwrap();
        let target = PathBuf::from(target.trim());
        let read = std::fs::read(&target).is_ok();
        let write = std::fs::write(&target, b"overwritten").is_ok();
        let mesh = std::fs::read_to_string(run.join("probe-mesh-socket")).unwrap();
        let mesh = std::os::unix::net::UnixStream::connect(mesh.trim()).is_ok();
        let owner_path = std::fs::read_to_string(run.join("probe-owner-socket")).unwrap();
        let owner_path = owner_path.trim();
        let owner_direct = std::os::unix::net::UnixStream::connect(owner_path).is_ok();
        let root = PathBuf::from(serde_json::from_slice::<Value>(&std::fs::read(run.join("spawn.json")).unwrap()).unwrap()["root"].as_str().unwrap());
        let owner_alias = root.join("owner-alias.sock");
        let _ = std::os::unix::fs::symlink(owner_path, &owner_alias);
        let owner_alias = std::os::unix::net::UnixStream::connect(owner_alias).is_ok();
        let sibling = std::fs::read_to_string(run.join("probe-sibling-pid")).unwrap();
        let sibling: i32 = sibling.trim().parse().unwrap();
        // Signal 0 is a harmless permission probe against a test-owned
        // sibling, never a signal to a real daemon.
        let signal = nix::sys::signal::kill(nix::unistd::Pid::from_raw(sibling), None).is_ok();
        let pin_write = std::fs::write(run.join("user.pub"), b"forged").is_ok();
        let replacement = run.join("replacement");
        std::fs::write(&replacement, b"forged").unwrap();
        let pin_replace = std::fs::rename(replacement, run.join("user.pub")).is_ok();
        let moved_run = run.with_file_name("moved-by-sandbox-probe");
        let run_dir_rename = std::fs::rename(&run, &moved_run).is_ok();
        if run_dir_rename {
            let _ = std::fs::rename(&moved_run, &run);
        }
        let escape_admin_denied = probe_escaped_admin(&run);
        std::fs::write(run.join("probe-result"), format!("allowed_read={allowed_read} read={read} write={write} mesh={mesh} owner_direct={owner_direct} owner_alias={owner_alias} signal={signal} pin_write={pin_write} pin_replace={pin_replace} run_dir_rename={run_dir_rename} escape_admin_denied={escape_admin_denied}"))
            .unwrap();
    }
    match mode.as_str() {
        "crash" => std::process::exit(1),
        "exit0" => std::process::exit(0),
        _ => serve(&run, &id, &mode),
    }
}

fn probe_escaped_admin(run: &PathBuf) -> bool {
    let spawn: Value = serde_json::from_slice(&std::fs::read(run.join("spawn.json")).unwrap()).unwrap();
    let socket = spawn["parent_socket"].as_str().unwrap().to_owned();
    let owner_token = std::fs::read_to_string(run.join("probe-owner-token")).unwrap();
    // The fake kernel is a process-group leader. Fork once so the test-owned
    // grandchild can enter a new session; never touch any other process.
    let pid = unsafe { nix::libc::fork() };
    if pid < 0 { return false }
    if pid == 0 {
        let accepted = (|| -> std::io::Result<bool> {
            if unsafe { nix::libc::setsid() } < 0 { return Ok(false) }
            let stream = std::os::unix::net::UnixStream::connect(socket)?;
            stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
            let mut writer = stream.try_clone()?;
            let mut reader = BufReader::new(stream);
            for method in ["auth.token.issue", "project.revoke", "kernel.shutdown"] {
                for auth in ["admin", owner_token.trim()] {
                    let req = json!({"id":"escape","proto":1,"method":method,"params":{},"auth":auth});
                    writer.write_all(format!("{req}\n").as_bytes())?;
                    let mut line = String::new();
                    reader.read_line(&mut line)?;
                    let answer: Value = serde_json::from_str(&line).map_err(std::io::Error::other)?;
                    if answer["error_kind"] != "child_endpoint_method_denied" { return Ok(false) }
                }
            }
            Ok(true)
        })().unwrap_or(false);
        unsafe { nix::libc::_exit(if accepted { 0 } else { 1 }) };
    }
    let mut status = 0;
    (unsafe { nix::libc::waitpid(pid, &mut status, 0) }) == pid
        && nix::libc::WIFEXITED(status) && nix::libc::WEXITSTATUS(status) == 0
}

fn serve(run: &PathBuf, id: &str, mode: &str) -> ! {
    let paths = RuntimePaths::at(run);
    let _lock = (mode != "nolock").then(|| InstanceLock::acquire(&paths).expect("kernel.lock"));
    clawft_weave::instance_lock::write_pid_file(&paths.pid(), std::process::id()).unwrap();
    if let Some(ms) = std::fs::read_to_string(run.join("start_delay_ms")).ok().and_then(|s| s.trim().parse().ok()) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    if mode == "ignore-term" {
        ignore_term(run);
    }
    let _ = std::fs::remove_file(paths.socket());
    let listener = UnixListener::bind(paths.socket()).expect("bind kernel.sock");
    std::fs::write(run.join("ready"), "1").unwrap();
    if mode == "crash-after-ready" {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            std::process::exit(3);
        });
    }
    let wrong_pid = mode == "wrong-pid";
    let sha = std::fs::read_to_string(run.join("sha")).map(|s| s.trim().to_owned()).unwrap_or_default();
    let reported = if mode == "wrong-project" { "01JB8Z3Q0V6X9KQ4M2N7T5R1WD".to_owned() } else { id.to_owned() };
    for conn in listener.incoming().flatten() {
        let (run, mode, reported, sha) = (run.clone(), mode.to_owned(), reported.clone(), sha.clone());
        let shown_pid = std::process::id() + u32::from(wrong_pid);
        std::thread::spawn(move || {
            let mut out = conn.try_clone().unwrap();
            for line in BufReader::new(conn).lines().map_while(Result::ok) {
                let req: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                let reply = match req["method"].as_str() {
                    Some("kernel.handshake") => json!({"ok": true, "result": {
                        "proto": {"current": 1, "min": 1},
                        "node_id": "0".repeat(32),
                        "runtime_dir": run.display().to_string(),
                        "pid": shown_pid,
                        "project_id": reported,
                        "sha": sha,
                        "version": if sha.is_empty() { "" } else { "0.0.0-fake" },
                    }}),
                    Some("kernel.shutdown") => {
                        let _ = std::fs::write(run.join("shutdown.seen"), line.as_bytes());
                        if mode != "ignore-shutdown" && mode != "ignore-term" {
                            let _ = writeln!(out, "{}", json!({"ok": true, "result": {}}));
                            let _ = std::fs::remove_file(run.join("kernel.pid"));
                            std::process::exit(0);
                        }
                        json!({"ok": true, "result": {}})
                    }
                    _ => json!({"ok": false, "error": "unknown method"}),
                };
                if writeln!(out, "{reply}").is_err() {
                    break;
                }
            }
        });
    }
    std::process::exit(0)
}

static TERM_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_term(_: i32) {
    TERM_SEEN.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Survive `SIGTERM` (the handler only sets a flag; a thread writes `term.seen`).
fn ignore_term(run: &PathBuf) {
    use nix::sys::signal::{SigHandler, Signal, signal};
    // SAFETY: the handler only stores to an atomic.
    unsafe { signal(Signal::SIGTERM, SigHandler::Handler(on_term)) }.expect("install SIGTERM handler");
    let run = run.clone();
    std::thread::spawn(move || {
        while !TERM_SEEN.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = std::fs::write(run.join("term.seen"), "1");
    });
}
