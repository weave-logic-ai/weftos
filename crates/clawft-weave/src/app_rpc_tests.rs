//! Unit tests for `app_rpc` manifest loading and the kernel-free handlers.

use super::*;

fn noop() -> impl Fn(&str, Value) {
    |_: &str, _: Value| {}
}

fn app_dir(toml: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("weftapp.toml"), toml).unwrap();
    dir
}

const TOML: &str = "name = \"demo-app\"\nversion = \"1.2.3\"\n\n[[agents]]\nid = \"worker\"\n";

#[test]
fn loads_toml_from_directory() {
    let dir = app_dir(TOML);
    let m = load_manifest(dir.path().to_str().unwrap()).unwrap();
    assert_eq!(m.name, "demo-app");
    assert_eq!(m.agents.len(), 1);
}

#[test]
fn loads_json_file_and_falls_back_to_json_in_dir() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("weftapp.json");
    std::fs::write(&f, r#"{"name":"j-app","version":"0.1.0"}"#).unwrap();
    assert_eq!(load_manifest(f.to_str().unwrap()).unwrap().name, "j-app");
    assert_eq!(load_manifest(dir.path().to_str().unwrap()).unwrap().name, "j-app");
}

#[test]
fn rejects_bad_paths_and_content() {
    assert!(load_manifest("").is_err());
    assert!(load_manifest("relative/dir").unwrap_err().contains("absolute"));
    let empty = tempfile::tempdir().unwrap();
    assert!(load_manifest(empty.path().to_str().unwrap()).unwrap_err().contains("no weftapp"));
    let other = empty.path().join("manifest.yaml");
    std::fs::write(&other, "name: x").unwrap();
    assert!(load_manifest(other.to_str().unwrap()).unwrap_err().contains("must be named"));
    let big = empty.path().join("weftapp.json");
    std::fs::write(&big, vec![b' '; (MAX_MANIFEST_BYTES + 1) as usize]).unwrap();
    assert!(load_manifest(big.to_str().unwrap()).unwrap_err().contains("too large"));
    let bad = app_dir("name = \"\"\nversion = \"1\"\n");
    assert!(load_manifest(bad.path().to_str().unwrap()).is_err());
}

/// WEFT card 46ea52d3: `app.install` must not read arbitrary `.toml` / `.json`
/// files or quote their content back in errors.
#[test]
fn install_does_not_disclose_arbitrary_files() {
    const SECRET: &str = "SUPER-SECRET-TOKEN-4242";
    let dir = tempfile::tempdir().unwrap();

    // An arbitrary readable .json / .toml file is refused by name.
    let creds = dir.path().join("credentials.json");
    std::fs::write(&creds, format!("{{\"token\": \"{SECRET}\"")).unwrap();
    let err = load_manifest(creds.to_str().unwrap()).unwrap_err();
    assert!(err.contains("must be named"), "{err}");
    assert!(!err.contains(SECRET), "{err}");
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, format!("token = \"{SECRET}\"\n")).unwrap();
    assert!(!load_manifest(cfg.to_str().unwrap()).unwrap_err().contains(SECRET));

    // A weftapp.* symlink pointing at such a file is judged by its real name.
    #[cfg(unix)]
    {
        let app = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&creds, app.path().join("weftapp.json")).unwrap();
        let err = load_manifest(app.path().to_str().unwrap()).unwrap_err();
        assert!(err.contains("must be named") && !err.contains(SECRET), "{err}");
    }
}

#[test]
fn manifest_parse_errors_do_not_quote_file_content() {
    const SECRET: &str = "SUPER-SECRET-TOKEN-4242";
    // Malformed TOML whose offending line carries the secret.
    let toml_dir = app_dir(&format!("name = \"demo\"\nversion = {SECRET}\n"));
    let err = load_manifest(toml_dir.path().to_str().unwrap()).unwrap_err();
    assert!(!err.contains(SECRET), "{err}");
    assert!(err.contains("line 2"), "location kept: {err}");
    // Well-formed JSON of the wrong shape: serde quotes the value.
    let json_dir = tempfile::tempdir().unwrap();
    std::fs::write(json_dir.path().join("weftapp.json"), format!("{{\"name\": [\"{SECRET}\"]}}")).unwrap();
    let err = load_manifest(json_dir.path().to_str().unwrap()).unwrap_err();
    assert!(!err.contains(SECRET), "{err}");
}

#[test]
fn install_list_inspect_remove_roundtrip() {
    let mgr = AppManager::new();
    let dir = app_dir(TOML);
    let a = noop();
    let r = handle_install(&mgr, &json!({"path": dir.path().to_str().unwrap()}), None, &a);
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.result.unwrap(), json!("demo-app"));

    let list = handle_list(&mgr).result.unwrap();
    assert_eq!(list, json!([{"name": "demo-app", "state": "installed", "version": "1.2.3"}]));

    let ins = handle_inspect(&mgr, &json!({"name": "demo-app"})).result.unwrap();
    assert_eq!(ins["manifest"]["agents"][0]["id"], "worker");
    assert!(!handle_inspect(&mgr, &json!({"name": "nope"})).ok);
    assert!(!handle_inspect(&mgr, &json!({})).ok);

    let dup = handle_install(&mgr, &json!({"path": dir.path().to_str().unwrap()}), None, &a);
    assert!(!dup.ok);

    assert!(handle_remove(&mgr, &json!({"name": "demo-app"}), None, &a).ok);
    assert_eq!(handle_list(&mgr).result.unwrap(), json!([]));
}

#[test]
fn chain_kinds_match_kernel_constants() {
    #[cfg(feature = "exochain")]
    {
        use clawft_kernel::chain as c;
        assert_eq!(APP_INSTALL, c::EVENT_KIND_APP_INSTALL);
        assert_eq!(APP_START, c::EVENT_KIND_APP_START);
        assert_eq!(APP_STOP, c::EVENT_KIND_APP_STOP);
        assert_eq!(APP_REMOVE, c::EVENT_KIND_APP_REMOVE);
    }
}

#[cfg(feature = "exochain")]
mod gated {
    use super::*;
    use crate::rpc_gate::test_support::{FixedGate, Recorder};

    #[test]
    fn denied_install_leaves_catalog_empty_and_unchained() {
        let mgr = AppManager::new();
        let dir = app_dir(TOML);
        let gate = FixedGate::deny("policy");
        let rec = Recorder::default();
        let r = handle_install(&mgr, &json!({"path": dir.path().to_str().unwrap()}), Some(&gate), &rec.sink());
        assert!(!r.ok);
        assert!(r.error.unwrap().contains("policy"));
        assert!(mgr.is_empty());
        assert_eq!(gate.actions(), [APP_INSTALL]);
        assert!(rec.kinds().is_empty());
    }

    #[test]
    fn permitted_install_and_remove_are_chained() {
        let mgr = AppManager::new();
        let dir = app_dir(TOML);
        let gate = FixedGate::permit();
        let rec = Recorder::default();
        assert!(handle_install(&mgr, &json!({"path": dir.path().to_str().unwrap()}), Some(&gate), &rec.sink()).ok);
        assert!(handle_remove(&mgr, &json!({"name": "demo-app"}), Some(&gate), &rec.sink()).ok);
        assert_eq!(gate.actions(), [APP_INSTALL, APP_REMOVE]);
        assert_eq!(rec.kinds(), [APP_INSTALL, APP_REMOVE]);
    }

    #[test]
    fn denied_remove_keeps_app() {
        let mgr = AppManager::new();
        let dir = app_dir(TOML);
        let a = noop();
        assert!(handle_install(&mgr, &json!({"path": dir.path().to_str().unwrap()}), None, &a).ok);
        let r = handle_remove(&mgr, &json!({"name": "demo-app"}), Some(&FixedGate::deny("x")), &a);
        assert!(!r.ok);
        assert_eq!(mgr.len(), 1);
    }
}
