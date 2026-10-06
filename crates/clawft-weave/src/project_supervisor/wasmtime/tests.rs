//! Wasmtime driver selection and artifact pinning.
use super::*;
use clawft_types::project::ServeSection;

#[test]
fn explicit_selection_never_downgrades() {
    let run = Path::new("/nonexistent-weftos-wasm-selection-test");
    let mut serve = ServeSection::default();
    assert_eq!(selected_serve(&serve, run).unwrap(), "logical");
    serve.sandbox = ProjectSandbox::Wasmtime;
    assert!(selected_serve(&serve, run).is_err());
    serve.adapter = Some("logical".into());
    assert!(selected_serve(&serve, run).is_err());
    serve.adapter = Some(ADAPTER.into());
    assert_eq!(selected_serve(&serve, run).is_ok(), cfg!(feature = "wasmtime-project"));
    serve.sandbox = ProjectSandbox::Logical;
    assert!(selected_serve(&serve, run).is_err());
    serve.adapter = None;
    serve.sandbox = ProjectSandbox::LinuxContainer;
    // Container and Seatbelt projects keep the native kernel; the container
    // lane owns their driver, so selection stays `logical` and never wasmtime.
    assert_eq!(selected_serve(&serve, run).unwrap(), "logical");
    serve.adapter = Some(ADAPTER.into());
    assert!(selected_serve(&serve, run).is_err());
}

#[test]
fn malformed_receipt_blocks_native_fallback() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join(RECEIPT), b"broken").unwrap();
    assert!(selected_serve(&ServeSection::default(), t.path()).is_err());
    assert!(receipt(t.path()).is_err());
}

#[test]
fn artifact_substitution_and_symlink_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let base = t.path().canonicalize().unwrap();
    let artifact = base.join("guest.wasm");
    fs::write(&artifact, b"original").unwrap();
    fs::set_permissions(&artifact, fs::Permissions::from_mode(0o600)).unwrap();
    let hash = digest(b"original");
    assert_eq!(pinned(&artifact, &hash).unwrap(), b"original");
    fs::write(&artifact, b"substituted").unwrap();
    assert!(pinned(&artifact, &hash).is_err());
    let alias = base.join("alias");
    std::os::unix::fs::symlink(&artifact, &alias).unwrap();
    assert!(pinned(&alias, &digest(b"substituted")).is_err());
}

struct Fixture {
    _t: tempfile::TempDir,
    cfg: SupervisorConfig,
    root: PathBuf,
}

const GOOD: &str = r#"{"adapter":"wasmtime-project-v1","runner":"/opt/weftos/runner","runner_sha256":"RUNNER","artifact":"/opt/weftos/guest.wasm","artifact_sha256":"GUEST","lifetime_fuel":1000000,"memory_bytes":268435456,"lifetime_secs":600}"#;

fn config_json(edit: impl FnOnce(String) -> String) -> String {
    edit(GOOD.replace("RUNNER", &"a".repeat(64)).replace("GUEST", &"b".repeat(64)))
}

/// A private home with `~/.weftos`, a canonical project root beside it and no
/// operator configuration yet. Nothing here is the real `~/.weftos`.
fn fixture() -> Fixture {
    let t = tempfile::tempdir().unwrap();
    let base = t.path().canonicalize().unwrap();
    let home = base.join("home");
    let root = base.join("project");
    fs::create_dir_all(home.join(".weftos")).unwrap();
    fs::create_dir_all(root.join(".weftos")).unwrap();
    for d in [&home.join(".weftos"), &root.join(".weftos")] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let cfg = SupervisorConfig::new(&home, PathBuf::from("/nonexistent/weaver"));
    Fixture { _t: t, cfg, root }
}

fn write_config(f: &Fixture, text: &str, mode: u32) {
    let path = f.cfg.home.join(".weftos/project-wasmtime.json");
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn operator_configuration_loads_only_when_private_pinned_and_bounded() {
    let f = fixture();
    write_config(&f, &config_json(|s| s), 0o600);
    let c = OperatorConfig::load(&f.cfg, &f.root).unwrap();
    assert_eq!((c.lifetime_fuel, c.lifetime_secs), (1_000_000, 600));
    let refused: [(&str, String, u32); 8] = [
        ("group/world readable file", config_json(|s| s), 0o644),
        ("zero fuel", config_json(|s| s.replace("1000000", "0")), 0o600),
        ("tiny memory", config_json(|s| s.replace("268435456", "1024")), 0o600),
        ("a logical adapter", config_json(|s| s.replace("wasmtime-project-v1", "logical")), 0o600),
        ("short hash pin", config_json(|s| s.replace(&"a".repeat(64), "abc")), 0o600),
        ("uppercase hash pin", config_json(|s| s.replace(&"a".repeat(64), &"A".repeat(64))), 0o600),
        ("unknown field", config_json(|s| s.replace("\"adapter\"", "\"extra\":1,\"adapter\"")), 0o600),
        ("artifact inside the project", config_json(|s| s.replace("/opt/weftos/guest.wasm", "PROJECT/guest.wasm")), 0o600),
    ];
    for (why, text, mode) in refused {
        write_config(&f, &text.replace("PROJECT", f.root.to_str().unwrap()), mode);
        assert!(OperatorConfig::load(&f.cfg, &f.root).is_err(), "{why} must be refused");
    }
}

#[test]
fn operator_configuration_must_not_come_from_the_project() {
    let f = fixture();
    // A manifest cannot choose artifacts: only the operator file is read, and
    // it must sit in the private operator directory, never under the project.
    fs::write(f.root.join(".weftos/project-wasmtime.json"), config_json(|s| s)).unwrap();
    assert!(OperatorConfig::load(&f.cfg, &f.root).is_err(), "no operator file");
    write_config(&f, &config_json(|s| s), 0o600);
    // A project root that contains, or is contained by, supervisor authority is refused.
    assert!(OperatorConfig::load(&f.cfg, &f.cfg.home).is_err());
    assert!(OperatorConfig::load(&f.cfg, &f.cfg.home.join(".weftos")).is_err());
    fs::set_permissions(f.cfg.home.join(".weftos"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(OperatorConfig::load(&f.cfg, &f.root).is_err(), "operator directory must be 0700");
}

#[test]
fn preflight_refuses_unsafe_runtime_and_state_directories() {
    let f = fixture();
    write_config(&f, &config_json(|s| s), 0o600);
    fs::create_dir_all(&f.cfg.run_root).unwrap();
    fs::set_permissions(&f.cfg.run_root, fs::Permissions::from_mode(0o700)).unwrap();
    let run = f.cfg.run_root.join("01ARZ3NDEKTSV4RRFFQ69G5FAV");
    preflight(&f.cfg, &f.root, &run).unwrap();
    assert_eq!(fs::metadata(&run).unwrap().mode() & 0o777, 0o700);
    // A run directory outside the supervisor's run root is refused.
    assert!(preflight(&f.cfg, &f.root, &f.root.join("run")).is_err());
    // The guest's `.weftos` must already be private; it is never silently chmodded.
    fs::set_permissions(f.root.join(".weftos"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(preflight(&f.cfg, &f.root, &run).is_err());
    assert_eq!(fs::metadata(f.root.join(".weftos")).unwrap().mode() & 0o777, 0o755);
}

#[test]
fn a_symlinked_or_widened_kernel_log_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let run = t.path().canonicalize().unwrap();
    validate_log(&run).unwrap();
    let log = run.join("kernel.log");
    fs::write(&log, b"").unwrap();
    fs::set_permissions(&log, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(validate_log(&run).is_err());
    fs::set_permissions(&log, fs::Permissions::from_mode(0o600)).unwrap();
    validate_log(&run).unwrap();
    fs::remove_file(&log).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", &log).unwrap();
    assert!(validate_log(&run).is_err());
}
