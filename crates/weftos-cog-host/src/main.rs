//! weft-cog-host (COG-009): run cogs on an appliance under WeftOS, with no agent cap.
//!
//!   weft-cog-host serve [--port 9480] [--root <dir>]      # supervise + lifecycle HTTP API
//!   weft-cog-host add --id <id> --binary <path> [--source weavelogic|cognitum|local]
//!                     [--version v] [--arg --interval --arg 1] [--enable] [--signed]
//!                     [--guide <guide dir | bundled guide.json>]   (served at GET /cogs/<id>/guide)
//!   weft-cog-host list [--root <dir>]
//!   weft-cog-host start|stop <id> [--root <dir>]           # flip enabled on disk (a running daemon picks it up)
//!   weft-cog-host licence status|import <records.json> [--root <dir>] [--licence-dir <dir>]
//!
//! Root defaults to $WEFTOS_COG_ROOT or ~/.weftos/cogs. The agent's /var/lib/cognitum/apps is
//! untouched; cogs run here still POST to the agent store on :80, so they join its memory.

mod http;

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use weftos_cog_host::licence::{default_licence_dir, HostLicence, Records};
use weftos_cog_host::supervise::Supervisor;
use weftos_cog_host::{default_root, load_records, save_record, CogRecord, Source};

fn args() -> Vec<String> {
    std::env::args().collect()
}
fn opt<'a>(a: &'a [String], f: &str) -> Option<&'a str> {
    a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).map(String::as_str)
}
fn opts_all(a: &[String], f: &str) -> Vec<String> {
    a.iter().enumerate().filter(|(_, x)| x.as_str() == f).filter_map(|(i, _)| a.get(i + 1).cloned()).collect()
}
fn has(a: &[String], f: &str) -> bool {
    a.iter().any(|x| x == f)
}
fn root_of(a: &[String]) -> PathBuf {
    opt(a, "--root").map(PathBuf::from).unwrap_or_else(default_root)
}
fn licence_dir_of(a: &[String], root: &std::path::Path) -> PathBuf {
    opt(a, "--licence-dir").map(PathBuf::from).unwrap_or_else(|| default_licence_dir(root))
}

fn usage() -> ! {
    eprintln!(
        "weft-cog-host (COG-009)\n\n\
         serve [--port 9480] [--root <dir>]        supervise cogs + serve the lifecycle API\n\
         add --id <id> --binary <path> [--source weavelogic|cognitum|local] [--version v]\n\
         \t[--arg <a> ...] [--enable] [--signed] [--guide <dir|guide.json>]   install a cog into the root\n\
         list [--root <dir>]                       show installed cogs\n\
         start|stop <id> [--root <dir>]            set enabled on disk (a running daemon applies it)\n\
         licence status [--licence-dir <dir>]      show the ADR-106 licence state the start check uses\n\
         licence import <records.json>             verify and apply signed binding/grants/approvals/revocations\n\n\
         root defaults to $WEFTOS_COG_ROOT or ~/.weftos/cogs; the licence dir to\n\
         $WEFT_COG_HOST_LICENCE_DIR or <root>/.licence (serve also takes --licence-dir)"
    );
    std::process::exit(2);
}

fn main() {
    let a = args();
    let res = match a.get(1).map(String::as_str) {
        Some("serve") => cmd_serve(&a[2..]),
        Some("add") => cmd_add(&a[2..]),
        Some("list") => cmd_list(&a[2..]),
        Some(act @ ("start" | "stop")) => cmd_enable(&a[2..], act == "start"),
        Some("licence") => cmd_licence(&a[2..]),
        _ => usage(),
    };
    if let Err(e) = res {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn cmd_serve(a: &[String]) -> Result<(), String> {
    let root = root_of(a);
    std::fs::create_dir_all(&root).map_err(|e| format!("create root: {e}"))?;
    let port: u16 = opt(a, "--port").and_then(|p| p.parse().ok()).unwrap_or(9480);
    let licence = Arc::new(HostLicence::open(licence_dir_of(a, &root)));
    let mut supervisor = Supervisor::new(root.clone());
    supervisor.set_licence_gate(licence.clone());
    let sup = Arc::new(Mutex::new(supervisor));

    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("bind :{port}: {e}"))?;
    eprintln!("[cog-host] root={} api=http://0.0.0.0:{port} — {} cog(s) known", root.display(), sup.lock().unwrap().ids().len());
    eprintln!("[cog-host] licence dir {} ({})", licence.dir().display(), licence.status()["state"].as_str().unwrap_or("?"));

    // Per-host bearer token for the /hw/* mutating routes ($WEFT_COG_HOST_TOKEN, else <root>/host.token).
    let policy = Arc::new(weftos_cog_host::auth::Policy::load(&root).map_err(|e| format!("host token: {e}"))?);
    eprintln!("[cog-host] /hw token: {} (or $WEFT_COG_HOST_TOKEN); allowed browser origins: loopback + $WEFT_COG_HOST_ORIGINS", weftos_cog_host::auth::token_path(&root).display());
    // Persist the dex numbering (append-only) once so read-only GETs never have to write.
    let _ = weftos_cog_host::dex::with_dex(&root, |_| ());

    // HTTP thread
    let (http_sup, http_lic) = (Arc::clone(&sup), Arc::clone(&licence));
    std::thread::spawn(move || http::serve(listener, http_sup, policy, http_lic));

    // Stop children cleanly on Ctrl-C by relying on process exit; supervision loop in the main thread.
    loop {
        {
            licence.refresh(); // pick up `licence import` from the CLI
            let mut s = sup.lock().unwrap();
            s.reload(); // pick up installs/enable-flips from disk or the CLI
            s.tick();
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn cmd_add(a: &[String]) -> Result<(), String> {
    let root = root_of(a);
    let id = opt(a, "--id").ok_or("add needs --id")?;
    let bin = opt(a, "--binary").ok_or("add needs --binary <path>")?;
    let source = match opt(a, "--source").unwrap_or("local") {
        "weavelogic" => Source::WeaveLogic,
        "cognitum" => Source::Cognitum,
        _ => Source::Local,
    };
    let version = opt(a, "--version").unwrap_or("0.0.0").to_string();
    let args = opts_all(a, "--arg");

    let binary = format!("cog-{id}-arm");
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {dir:?}: {e}"))?;
    let bytes = std::fs::read(bin).map_err(|e| format!("read {bin}: {e}"))?;
    std::fs::write(dir.join(&binary), &bytes).map_err(|e| format!("write binary: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join(&binary), std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let rec = CogRecord {
        id: id.to_string(),
        version,
        source,
        enabled: has(a, "--enable"),
        binary,
        args,
        signed: has(a, "--signed"),
    };
    save_record(&root, &rec).map_err(|e| e.to_string())?;
    if let Some(g) = opt(a, "--guide") {
        weftos_cog_host::introspect::install_guide(&root, id, std::path::Path::new(g)).map_err(|e| format!("--guide: {e}"))?;
    }
    eprintln!("added {id} ({} bytes){}", bytes.len(), if rec.enabled { ", enabled" } else { "" });
    Ok(())
}

fn cmd_list(a: &[String]) -> Result<(), String> {
    let root = root_of(a);
    let recs = load_records(&root);
    if recs.is_empty() {
        eprintln!("no cogs in {}", root.display());
        return Ok(());
    }
    eprintln!("{} cog(s) in {}:", recs.len(), root.display());
    for r in recs {
        eprintln!("  {:<16} v{:<8} {:<10} enabled={} signed={} args={:?}", r.id, r.version, format!("{:?}", r.source).to_lowercase(), r.enabled, r.signed, r.args);
    }
    Ok(())
}

fn cmd_enable(a: &[String], enable: bool) -> Result<(), String> {
    let root = root_of(a);
    let id = a.first().filter(|s| !s.starts_with("--")).ok_or("need a <id>")?;
    let mut recs = load_records(&root);
    let rec = recs.iter_mut().find(|r| &r.id == id).ok_or_else(|| format!("no such cog '{id}'"))?;
    rec.enabled = enable;
    save_record(&root, rec).map_err(|e| e.to_string())?;
    eprintln!("{id} enabled={enable} (a running daemon applies it within ~1s)");
    Ok(())
}

fn cmd_licence(a: &[String]) -> Result<(), String> {
    let root = root_of(a);
    let lic = HostLicence::open(licence_dir_of(a, &root));
    match a.first().map(String::as_str) {
        Some("status") => {
            println!("{}", serde_json::to_string_pretty(&lic.status()).map_err(|e| e.to_string())?);
            Ok(())
        }
        Some("import") => {
            let file = a.get(1).filter(|s| !s.starts_with("--")).ok_or("licence import needs <records.json>")?;
            let bytes = std::fs::read(file).map_err(|e| format!("read {file}: {e}"))?;
            if bytes.len() > weftos_cog_host::licence::MAX_IMPORT_BYTES {
                return Err(format!("{file} is larger than {} bytes", weftos_cog_host::licence::MAX_IMPORT_BYTES));
            }
            let recs: Records = serde_json::from_slice(&bytes).map_err(|e| format!("{file}: {e}"))?;
            let lines = lic.import(&recs)?;
            println!("{}", serde_json::to_string_pretty(&lines).map_err(|e| e.to_string())?);
            Ok(())
        }
        _ => Err("licence needs status or import <records.json>".into()),
    }
}
