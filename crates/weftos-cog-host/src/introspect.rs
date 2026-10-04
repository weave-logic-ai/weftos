//! Read-only facts the console needs to guide an install and to check it afterwards (ADR-107):
//!
//!   GET /hw/buses            (token) what this node can host: arch, UART / I2C / USB-serial
//!                            devices, whether the kernel console owns the UART, enabled cogs
//!   GET /cogs/<id>/last      the cog's last output line, health fields only (no positions)
//!   GET /cogs/<id>/guide     the hook-up guide installed next to the cog (`guide.json`)
//!
//! The host only reports what it can read: a device node that exists, the kernel command line, the
//! cog records on disk. It never claims a bus is "enabled" beyond that; the console turns the facts
//! into pass / fail / "verify by hand" using the cog's guide. `fs` is the filesystem root (`/` in
//! production) so tests run against a temp tree.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Largest installed guide served.
const GUIDE_CAP: u64 = 4 * 1024 * 1024;
/// How much of the log tail is searched for the last output line.
const TAIL_BYTES: u64 = 16 * 1024;
/// Output fields passed through by `/cogs/<id>/last`. Anything else, notably target positions,
/// stays on the node: the cog keeps its own export on loopback for that reason.
const LAST_FIELDS: &[&str] = &["schema", "cog", "version", "timestamp_ms", "health", "quality", "reasons", "frames", "frame_rate_hz", "parse_errors", "firmware", "tracking_mode"];

fn exists(fs: &Path, dev: &str) -> bool {
    fs.join(dev.trim_start_matches('/')).exists()
}

/// Names in `<fs>/dev` starting with one of `prefixes`, as `/dev/<name>`, sorted.
fn dev_matching(fs: &Path, prefixes: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(fs.join("dev"))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| prefixes.iter().any(|p| n.starts_with(p)))
        .map(|n| format!("/dev/{n}"))
        .collect();
    v.sort();
    v
}

/// The `console=` token of the kernel command line that points at a serial port, if any.
pub fn serial_console(cmdline: &str) -> Option<String> {
    cmdline
        .split_whitespace()
        .find(|t| t.strip_prefix("console=").is_some_and(|v| ["serial", "ttyAMA", "ttyS"].iter().any(|p| v.starts_with(p))))
        .map(str::to_string)
}

pub fn bus_facts(fs: &Path, node: &str, root: &Path) -> Value {
    let cmdline = std::fs::read_to_string(fs.join("proc/cmdline")).ok();
    let console = cmdline.as_deref().and_then(serial_console);
    let uart: Vec<&str> = ["/dev/serial0", "/dev/ttyAMA0", "/dev/ttyS0"].into_iter().filter(|d| exists(fs, d)).collect();
    // ids only: a cog's command line can carry key or token paths, which a bus check never needs
    let cogs: Vec<Value> = crate::load_records(root).iter().filter(|r| r.enabled).map(|r| json!({"id": r.id})).collect();
    json!({
        "ok": true,
        "node": node,
        "arch": std::env::consts::ARCH,
        "uart": {"devices": uart, "console_on_uart": cmdline.as_ref().map(|_| console.is_some()), "console": console},
        "i2c": {"devices": dev_matching(fs, &["i2c-"])},
        "usb_serial": dev_matching(fs, &["ttyUSB", "ttyACM"]),
        "enabled_cogs": cogs,
    })
}

fn cog_dir(root: &Path, id: &str) -> Option<PathBuf> {
    (crate::valid_cog_id(id) && crate::load_records(root).iter().any(|r| r.id == id)).then(|| root.join(id))
}

/// The newest JSON-object line in the tail of `host.log`, reduced to [`LAST_FIELDS`] plus the
/// `source.verified` / `source.simulated` flags.
pub fn last_output(root: &Path, id: &str) -> Result<Value, (&'static str, String)> {
    let dir = cog_dir(root, id).ok_or(("404 Not Found", "no such cog".to_string()))?;
    let log = dir.join("host.log");
    let mut f = std::fs::File::open(&log).map_err(|_| ("404 Not Found", "no output yet".to_string()))?;
    use std::io::{Read, Seek, SeekFrom};
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES))).map_err(|e| ("500 Internal Server Error", e.to_string()))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(|e| ("500 Internal Server Error", e.to_string()))?;
    let text = String::from_utf8_lossy(&buf);
    let obj = text
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .find(|v| v.is_object() && v.get("health").is_some())
        .ok_or(("404 Not Found", "no output line yet".to_string()))?;
    let mut out = serde_json::Map::new();
    for k in LAST_FIELDS {
        if let Some(v) = obj.get(*k) {
            out.insert((*k).into(), v.clone());
        }
    }
    let src = |k: &str| obj.pointer(&format!("/source/{k}")).cloned();
    out.insert("source".into(), json!({"verified": src("verified"), "simulated": src("simulated"), "kind": src("kind")}));
    Ok(Value::Object(out))
}

/// The guide shipped inside the installed package, as the `/guide` JSON: either a ready-made
/// `<root>/<id>/guide.json`, or an ADR-104 `<root>/<id>/guide/` folder (`guide.toml`, one `.md`
/// per page, any images) bundled on the fly. Needs no running cog.
pub fn installed_guide(root: &Path, id: &str) -> Result<String, (&'static str, String)> {
    let dir = cog_dir(root, id).ok_or(("404 Not Found", "no such cog".to_string()))?;
    let p = dir.join("guide.json");
    if std::fs::symlink_metadata(&p).is_ok() {
        // a ready-made guide.json: regular file only, size-capped
        return read_regular(&p, GUIDE_CAP)
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or(("404 Not Found", "no usable guide installed with this cog".to_string()));
    }
    bundle_dir(&dir.join("guide")).ok_or(("404 Not Found", "no guide installed with this cog".to_string()))
}

/// Store a hook-up guide with an installed cog (`<root>/<id>/guide.json`) so the host can serve it
/// at `GET /cogs/<id>/guide`. `src` is a guide folder (ADR-104) or a bundled `/guide` JSON file.
pub fn install_guide(root: &Path, id: &str, src: &Path) -> Result<(), String> {
    if !crate::valid_cog_id(id) {
        return Err(format!("bad cog id '{id}'"));
    }
    if !root.join(id).is_dir() {
        return Err(format!("cog '{id}' is not installed in {}", root.display()));
    }
    let json = if src.is_dir() {
        bundle_dir(src).ok_or("not a guide folder (needs guide.toml) or too large")?
    } else {
        let t = std::fs::read_to_string(src).map_err(|e| format!("read {}: {e}", src.display()))?;
        let v: Value = serde_json::from_str(&t).map_err(|e| format!("guide JSON: {e}"))?;
        if !(v["toml"].is_string() && v["pages"].is_object()) {
            return Err("guide JSON needs \"toml\" and \"pages\"".into());
        }
        t
    };
    if json.len() as u64 > GUIDE_CAP {
        return Err("guide too large".into());
    }
    std::fs::write(root.join(id).join("guide.json"), json).map_err(|e| e.to_string())
}

/// Read one regular file of at most `cap` bytes, never through a symlink.
#[cfg(unix)]
fn open_nofollow(path: &Path) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path).ok()
}

#[cfg(not(unix))]
fn open_nofollow(path: &Path) -> Option<std::fs::File> {
    std::fs::File::open(path).ok()
}

fn read_regular(path: &Path, cap: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let m = std::fs::symlink_metadata(path).ok()?;
    if !m.file_type().is_file() || m.len() > cap {
        return None;
    }
    // Open without following a symlink swapped in after the check, then re-check the opened file.
    let f = open_nofollow(path)?;
    if !f.metadata().ok()?.file_type().is_file() {
        return None;
    }
    let mut v = Vec::new();
    f.take(cap).read_to_end(&mut v).ok()?;
    Some(v)
}

/// Bundle a guide folder into the `/guide` JSON. Only regular files directly inside it count:
/// symlinks, sub-folders and anything over the size cap are skipped, so a package cannot make the
/// host read an unrelated file.
fn bundle_dir(g: &Path) -> Option<String> {
    use base64::Engine as _;
    // A symlinked guide/ directory could point anywhere; only a real directory counts.
    if !std::fs::symlink_metadata(g).ok()?.file_type().is_dir() {
        return None;
    }
    let toml = String::from_utf8(read_regular(&g.join("guide.toml"), GUIDE_CAP)?).ok()?;
    let (mut pages, mut images, mut total) = (serde_json::Map::new(), serde_json::Map::new(), toml.len() as u64);
    for e in std::fs::read_dir(g).ok()?.flatten() {
        let path = e.path();
        let Ok(name) = e.file_name().into_string() else { continue };
        let Some(bytes) = read_regular(&path, GUIDE_CAP.saturating_sub(total)) else { continue };
        total += bytes.len() as u64;
        match path.extension().and_then(|x| x.to_str()) {
            Some("md") => {
                if let (Some(stem), Ok(text)) = (path.file_stem().and_then(|s| s.to_str()), String::from_utf8(bytes)) {
                    pages.insert(stem.to_string(), Value::String(text));
                }
            }
            Some("png" | "jpg" | "jpeg" | "gif" | "webp") => {
                images.insert(name, Value::String(base64::engine::general_purpose::STANDARD.encode(bytes)));
            }
            _ => {}
        }
    }
    Some(json!({"toml": toml, "pages": pages, "images": images}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(fs: &Path, rel: &str) {
        let p = fs.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"").unwrap();
    }
    fn add_cog(root: &Path, id: &str, args: &[&str], enabled: bool) {
        let rec = crate::CogRecord {
            id: id.into(),
            version: "0.1.0".into(),
            source: crate::Source::Local,
            enabled,
            binary: format!("cog-{id}-arm"),
            args: args.iter().map(|s| s.to_string()).collect(),
            signed: false,
        };
        std::fs::create_dir_all(root.join(id)).unwrap();
        crate::save_record(root, &rec).unwrap();
    }

    #[test]
    fn kernel_console_on_a_serial_port_is_found() {
        assert_eq!(serial_console("root=/dev/mmcblk0p2 console=serial0,115200 console=tty1").as_deref(), Some("console=serial0,115200"));
        assert_eq!(serial_console("console=ttyAMA0,115200").as_deref(), Some("console=ttyAMA0,115200"));
        assert_eq!(serial_console("root=/dev/sda1 console=tty1 quiet"), None);
    }

    #[test]
    fn bus_facts_report_only_what_exists() {
        let fs = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        touch(fs.path(), "dev/serial0");
        touch(fs.path(), "dev/i2c-1");
        touch(fs.path(), "dev/ttyUSB0");
        touch(fs.path(), "proc/cmdline");
        std::fs::write(fs.path().join("proc/cmdline"), "console=serial0,115200 quiet").unwrap();
        add_cog(root.path(), "a", &["--device", "/dev/serial0"], true);
        add_cog(root.path(), "b", &[], false);
        let f = bus_facts(fs.path(), "cog0", root.path());
        assert_eq!(f["uart"]["devices"], json!(["/dev/serial0"]));
        assert_eq!(f["uart"]["console_on_uart"], true);
        assert_eq!(f["i2c"]["devices"], json!(["/dev/i2c-1"]));
        assert_eq!(f["usb_serial"], json!(["/dev/ttyUSB0"]));
        assert_eq!(f["enabled_cogs"].as_array().unwrap().len(), 1, "disabled cogs are not listed");
        assert_eq!(f["enabled_cogs"][0], json!({"id": "a"}), "no command line leaves the node");
        assert!(!f.to_string().contains("--device"));
        // an unreadable cmdline is "unknown", never "console off"
        let bare = tempfile::tempdir().unwrap();
        let f = bus_facts(bare.path(), "n", root.path());
        assert!(f["uart"]["console_on_uart"].is_null());
        assert_eq!(f["uart"]["devices"], json!([]));
    }

    #[test]
    fn last_output_keeps_health_and_drops_positions() {
        let root = tempfile::tempdir().unwrap();
        add_cog(root.path(), "ld2450-radar", &[], true);
        let line = r#"{"schema":"weavelogic.cog-output.v0","cog":"ld2450-radar","health":"no_source","quality":null,"reasons":["no_bytes"],"source":{"kind":"ld2450-uart","verified":false,"simulated":false},"targets":[{"x_m":1.0,"y_m":2.0}],"frames":0}"#;
        std::fs::write(root.path().join("ld2450-radar/host.log"), format!("starting\n{line}\nnoise\n")).unwrap();
        let v = last_output(root.path(), "ld2450-radar").unwrap();
        assert_eq!(v["health"], "no_source");
        assert_eq!(v["source"]["verified"], false);
        assert!(v.get("targets").is_none(), "positions must not leave the node");
        assert_eq!(last_output(root.path(), "../etc").unwrap_err().0, "404 Not Found");
        std::fs::write(root.path().join("ld2450-radar/host.log"), "only text\n").unwrap();
        assert_eq!(last_output(root.path(), "ld2450-radar").unwrap_err().0, "404 Not Found");
    }

    #[test]
    fn installed_guide_is_served_only_for_installed_cogs() {
        let root = tempfile::tempdir().unwrap();
        add_cog(root.path(), "x", &[], true);
        assert_eq!(installed_guide(root.path(), "x").unwrap_err().0, "404 Not Found");
        std::fs::write(root.path().join("x/guide.json"), r#"{"toml":"","pages":{}}"#).unwrap();
        assert!(installed_guide(root.path(), "x").unwrap().contains("pages"));
        assert_eq!(installed_guide(root.path(), "nope").unwrap_err().0, "404 Not Found");
    }

    #[test]
    fn install_guide_stores_a_folder_or_a_json_file_and_it_is_then_served() {
        let root = tempfile::tempdir().unwrap();
        add_cog(root.path(), "z", &[], true);
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("guide.toml"), "pages = [\"start\"]\n").unwrap();
        std::fs::write(src.path().join("start.md"), "# S\n").unwrap();
        install_guide(root.path(), "z", src.path()).unwrap();
        assert!(installed_guide(root.path(), "z").unwrap().contains("start"));
        let f = src.path().join("g.json");
        std::fs::write(&f, r##"{"toml":"","pages":{"a":"# A"}}"##).unwrap();
        install_guide(root.path(), "z", &f).unwrap();
        assert!(installed_guide(root.path(), "z").unwrap().contains("# A"));
        std::fs::write(&f, r#"{"nope":1}"#).unwrap();
        assert!(install_guide(root.path(), "z", &f).is_err());
        assert!(install_guide(root.path(), "z", Path::new("/nonexistent")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn bundling_never_follows_a_symlink_or_reads_past_the_cap() {
        let root = tempfile::tempdir().unwrap();
        add_cog(root.path(), "s", &[], false);
        let g = root.path().join("s/guide");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("guide.toml"), "pages = []\n").unwrap();
        let secret = root.path().join("secret.md");
        std::fs::write(&secret, "TOP SECRET").unwrap();
        std::os::unix::fs::symlink(&secret, g.join("leak.md")).unwrap();
        std::os::unix::fs::symlink(root.path().join("host.token"), g.join("tok.png")).unwrap();
        std::fs::create_dir_all(g.join("sub.md")).unwrap();
        let out = installed_guide(root.path(), "s").unwrap();
        assert!(!out.contains("TOP SECRET") && !out.contains("leak"), "{out}");
        assert!(!out.contains("tok.png"));
        // a guide.toml that is itself a symlink is refused outright
        std::fs::remove_file(g.join("guide.toml")).unwrap();
        std::os::unix::fs::symlink(&secret, g.join("guide.toml")).unwrap();
        assert!(installed_guide(root.path(), "s").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_guide_directory_is_refused() {
        let root = tempfile::tempdir().unwrap();
        add_cog(root.path(), "s", &[], false);
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("guide.toml"), "pages = []\n").unwrap();
        std::fs::write(elsewhere.join("leak.md"), "OUTSIDE").unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.path().join("s/guide")).unwrap();
        assert!(installed_guide(root.path(), "s").is_err());
    }

    #[test]
    fn install_guide_refuses_a_bad_or_uninstalled_id() {
        let root = tempfile::tempdir().unwrap();
        let f = root.path().join("g.json");
        std::fs::write(&f, r#"{"toml":"","pages":{}}"#).unwrap();
        assert!(install_guide(root.path(), "../etc", &f).is_err());
        assert!(install_guide(root.path(), "not-installed", &f).is_err());
    }

    #[test]
    fn a_guide_folder_in_the_package_is_bundled_on_the_fly() {
        let root = tempfile::tempdir().unwrap();
        add_cog(root.path(), "y", &[], false);
        let g = root.path().join("y/guide");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("guide.toml"), "pages = [\"start\"]\n").unwrap();
        std::fs::write(g.join("start.md"), "# Start\n\n> go\n").unwrap();
        std::fs::write(g.join("pin.png"), [1u8, 2, 3]).unwrap();
        let v: Value = serde_json::from_str(&installed_guide(root.path(), "y").unwrap()).unwrap();
        assert_eq!(v["pages"]["start"], "# Start\n\n> go\n");
        assert!(v["images"]["pin.png"].is_string());
        assert!(v["toml"].as_str().unwrap().contains("start"));
    }
}
