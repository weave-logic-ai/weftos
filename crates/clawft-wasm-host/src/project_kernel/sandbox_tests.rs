//! Negative tests for the guest boundary: forbidden parent relays and
//! filesystem escapes from the single `.weftos` preopen.
use super::*;
use std::os::unix::fs::symlink;
use wasmtime::{Instance, TypedFunc};

const PROJECT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const OTHER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

fn request(project: &str, method: &str, params: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"project": project, "method": method, "params": params})).unwrap()
}

#[test]
fn only_mesh_and_anchor_methods_reach_the_parent() {
    for ok in ["mesh.challenge", "mesh.register", "mesh.heartbeat", "mesh.unregister", "project.anchor.submit"] {
        check_parent_request(PROJECT, &request(PROJECT, ok, serde_json::json!({}))).unwrap();
    }
    for forbidden in [
        "chain.append",
        "kernel.shutdown",
        "kernel.stop",
        "project.start",
        "project.stop",
        "governance.parent.update",
        "fs.read",
        "exec",
        "",
    ] {
        assert!(
            check_parent_request(PROJECT, &request(PROJECT, forbidden, serde_json::json!({}))).is_err(),
            "{forbidden} must not be relayed"
        );
    }
    assert!(check_parent_request(PROJECT, br#"{"project":"p","params":{}}"#).is_err(), "missing method");
    assert!(check_parent_request(PROJECT, b"not json").is_err());
}

#[test]
fn parent_relay_is_scoped_to_the_guests_own_project() {
    assert!(check_parent_request(PROJECT, &request(OTHER, "mesh.heartbeat", serde_json::json!({}))).is_err());
    let crossing = request(PROJECT, "mesh.register", serde_json::json!({"project_id": OTHER}));
    assert!(check_parent_request(PROJECT, &crossing).is_err());
    let own = request(PROJECT, "mesh.register", serde_json::json!({"project_id": PROJECT}));
    check_parent_request(PROJECT, &own).unwrap();
}

const PROBE: &str = r#"(module
  (import "wasi_snapshot_preview1" "path_open"
    (func $path_open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_prestat_get" (func $prestat (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "environ_sizes_get" (func $env (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 100) "../escape")
  (data (i32.const 120) "/etc/hosts")
  (data (i32.const 140) "inside.txt")
  (data (i32.const 160) "sub/../../escape2")
  (data (i32.const 190) "link")
  (func (export "open") (param $ptr i32) (param $len i32) (param $oflags i32) (result i32)
    (call $path_open (i32.const 3) (i32.const 0) (local.get $ptr) (local.get $len) (local.get $oflags)
      (i64.const 66) (i64.const 66) (i32.const 0) (i32.const 8)))
  (func (export "prestat") (param i32) (result i32) (call $prestat (local.get 0) (i32.const 16)))
  (func (export "env_count") (result i32)
    (drop (call $env (i32.const 32) (i32.const 36))) (i32.load (i32.const 32))))"#;

struct Probe {
    store: Store<WasiP1Ctx>,
    instance: Instance,
}

impl Probe {
    fn new(state: &Path) -> Self {
        let engine = Engine::default();
        let module = Module::new(&engine, PROBE).unwrap();
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |c: &mut WasiP1Ctx| c).unwrap();
        let mut store = Store::new(&engine, state_wasi(state).unwrap());
        let instance = linker.instantiate(&mut store, &module).unwrap();
        Probe { store, instance }
    }

    fn open(&mut self, ptr: i32, len: i32, oflags: i32) -> i32 {
        let f: TypedFunc<(i32, i32, i32), i32> = self.instance.get_typed_func(&mut self.store, "open").unwrap();
        f.call(&mut self.store, (ptr, len, oflags)).unwrap()
    }
}

const CREATE: i32 = 1;
const OK: i32 = 0;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let base = t.path().canonicalize().unwrap();
    let state = base.join(".weftos");
    fs::create_dir(&state).unwrap();
    let outside = base.join("outside.txt");
    fs::write(&outside, b"secret").unwrap();
    (t, state, outside)
}

#[test]
fn guest_cannot_leave_its_state_directory() {
    let (_t, state, outside) = fixture();
    symlink(&outside, state.join("link")).unwrap();
    let mut p = Probe::new(&state);
    // Inside the preopen works, and really lands in the project's state dir.
    assert_eq!(p.open(140, 10, CREATE), OK);
    assert!(state.join("inside.txt").exists());
    // `..`, an absolute path, a normalized-away `..` and a symlink out all fail.
    assert_ne!(p.open(100, 9, CREATE), OK, "dot-dot escape");
    assert_ne!(p.open(120, 10, 0), OK, "absolute path");
    assert_ne!(p.open(160, 17, CREATE), OK, "nested dot-dot escape");
    assert_ne!(p.open(190, 4, 0), OK, "symlink to a file outside the root");
    assert!(!outside.parent().unwrap().join("escape").exists());
    assert!(!outside.parent().unwrap().join("escape2").exists());
    assert_eq!(fs::read(&outside).unwrap(), b"secret");
}

#[test]
fn guest_sees_exactly_one_preopen_and_no_environment() {
    let (_t, state, _outside) = fixture();
    let mut p = Probe::new(&state);
    let prestat: TypedFunc<i32, i32> = p.instance.get_typed_func(&mut p.store, "prestat").unwrap();
    assert_eq!(prestat.call(&mut p.store, 3).unwrap(), 0, "the state preopen");
    assert_ne!(prestat.call(&mut p.store, 4).unwrap(), 0, "no second preopen (HOME, run dir, root)");
    let env: TypedFunc<(), i32> = p.instance.get_typed_func(&mut p.store, "env_count").unwrap();
    assert_eq!(env.call(&mut p.store, ()).unwrap(), 0, "no inherited environment");
}
