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
    let cogs: Vec<Value> = crate::load_records(root).iter().filter(|r| r.enabled).map(|r| json!({"id": r.id, "args": r.args})).collect();
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
    if p.is_file() {
        if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) > GUIDE_CAP {
            return Err(("413 Payload Too Large", "guide too large".into()));
        }
        return std::fs::read_to_string(&p).map_err(|_| ("404 Not Found", "no guide installed with this cog".to_string()));
    }
    bundle_dir(&dir.join("guide")).ok_or(("404 Not Found", "no guide installed with this cog".to_string()))
}

fn bundle_dir(g: &Path) -> Option<String> {
    use base64::Engine as _;
    let toml = std::fs::read_to_string(g.join("guide.toml")).ok()?;
    let (mut pages, mut images, mut total) = (serde_json::Map::new(), serde_json::Map::new(), toml.len() as u64);
    for e in std::fs::read_dir(g).ok()?.flatten() {
        let path = e.path();
        let name = e.file_name().into_string().ok()?;
        let len = e.metadata().ok()?.len();
        total += len;
        if total > GUIDE_CAP {
            return None;
        }
        match path.extension().and_then(|x| x.to_str()) {
            Some("md") => {
                pages.insert(path.file_stem()?.to_str()?.to_string(), Value::String(std::fs::read_to_string(&path).ok()?));
            }
            Some("png" | "jpg" | "jpeg" | "gif" | "webp") => {
                images.insert(name, Value::String(base64::engine::general_purpose::STANDARD.encode(std::fs::read(&path).ok()?)));
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
