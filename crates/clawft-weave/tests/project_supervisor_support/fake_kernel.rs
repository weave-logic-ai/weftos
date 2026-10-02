//! The test binary acting as a project kernel: `main` calls [`run`] when it
//! is started with the real child argv (`kernel start --foreground
//! --profile project --project <id>`). It holds `kernel.lock`, writes
//! `kernel.pid`, serves `kernel.handshake` and `kernel.shutdown` on
//! `kernel.sock`, and records what it was given (argv, environment, cwd,
//! `spawn.json`) for the tests to read. Behaviour comes from the file
//! `<run>/behavior`: `serve` (default), `crash`, `exit0`,
//! `crash-after-ready`, `ignore-shutdown`, `nolock`, `wrong-project`.

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
    match mode.as_str() {
        "crash" => std::process::exit(1),
        "exit0" => std::process::exit(0),
        _ => serve(&run, &id, &mode),
    }
}

fn serve(run: &PathBuf, id: &str, mode: &str) -> ! {
    let paths = RuntimePaths::at(run);
    let _lock = (mode != "nolock").then(|| InstanceLock::acquire(&paths).expect("kernel.lock"));
    std::fs::write(paths.pid(), std::process::id().to_string()).unwrap();
    let _ = std::fs::remove_file(paths.socket());
    let listener = UnixListener::bind(paths.socket()).expect("bind kernel.sock");
    std::fs::write(run.join("ready"), "1").unwrap();
    if mode == "crash-after-ready" {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            std::process::exit(3);
        });
    }
    let reported = if mode == "wrong-project" { "01JB8Z3Q0V6X9KQ4M2N7T5R1WD".to_owned() } else { id.to_owned() };
    for conn in listener.incoming().flatten() {
        let (run, mode, reported) = (run.clone(), mode.to_owned(), reported.clone());
        std::thread::spawn(move || {
            let mut out = conn.try_clone().unwrap();
            for line in BufReader::new(conn).lines().map_while(Result::ok) {
                let req: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                let reply = match req["method"].as_str() {
                    Some("kernel.handshake") => json!({"ok": true, "result": {
                        "proto": {"current": 1, "min": 1},
                        "node_id": "0".repeat(32),
                        "runtime_dir": run.display().to_string(),
                        "pid": std::process::id(),
                        "project_id": reported,
                    }}),
                    Some("kernel.shutdown") => {
                        let _ = std::fs::write(run.join("shutdown.seen"), line.as_bytes());
                        if mode != "ignore-shutdown" {
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
