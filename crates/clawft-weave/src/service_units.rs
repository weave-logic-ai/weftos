//! Service-unit generators for the per-user daemon (ADR-103 Phase 1, package H).
//!
//! Pure text builders: nothing here touches the filesystem, `launchctl` or
//! `systemctl`. `weaver service unit` prints the result and the commands to
//! install it; the operator runs them.
//!
//! The unit runs `<exe> kernel start --foreground --profile user`, where
//! `exe` is the canonical path of the `weaver` that generated the unit. No
//! install prefix is hard-coded (the flaw in `scripts/com.clawft.wake.plist`).
//! After `weaver update` replaces that binary in place the unit keeps
//! pointing at the same path, so a restart picks up the new build.

use std::path::{Path, PathBuf};

/// launchd label for the per-user daemon.
pub const LAUNCHD_LABEL: &str = "ai.weftos.user";

/// systemd user unit name (without the `.service` suffix).
pub const SYSTEMD_UNIT: &str = "weftos";

/// Arguments after the executable.
const ARGS: [&str; 5] = ["kernel", "start", "--foreground", "--profile", "user"];

/// `<home>/.weftos/run/kernel.log`, the launchd stdout/stderr target.
pub fn log_path(home: &Path) -> PathBuf {
    home.join(".weftos").join("run").join("kernel.log")
}

/// Canonical path of the running binary, as a unit would reference it.
///
/// A replaced-in-place binary reads back from `/proc/<pid>/exe` with a
/// ` (deleted)` suffix; that suffix is dropped before canonicalising.
pub fn canonical_exe(exe: &Path) -> PathBuf {
    let stripped = strip_deleted(exe);
    stripped.canonicalize().unwrap_or(stripped)
}

/// Drop the Linux ` (deleted)` marker from an exe path.
pub fn strip_deleted(exe: &Path) -> PathBuf {
    let s = exe.to_string_lossy();
    match s.strip_suffix(" (deleted)") {
        Some(p) => PathBuf::from(p),
        None => exe.to_path_buf(),
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Quote one word for a systemd `ExecStart=` line: double quotes, with
/// `\`, `"`, `%` (specifier) and `$` (variable) escaped.
fn systemd_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `WorkingDirectory=` takes a path verbatim; only `%` needs escaping.
fn systemd_path(s: &str) -> String {
    s.replace('%', "%%")
}

const PLIST_TEMPLATE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>@LABEL@</string>
    <key>ProgramArguments</key>
    <array>
        <string>@EXE@</string>
@ARGS@    </array>
    <key>WorkingDirectory</key>
    <string>@HOME@</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>StandardOutPath</key>
    <string>@LOG@</string>
    <key>StandardErrorPath</key>
    <string>@LOG@</string>
</dict>
</plist>
"#;

/// The launchd property list for `~/Library/LaunchAgents/ai.weftos.user.plist`.
pub fn launchd_plist(exe: &Path, home: &Path) -> String {
    let exe = xml_escape(&exe.to_string_lossy());
    let log = xml_escape(&log_path(home).to_string_lossy());
    let home = xml_escape(&home.to_string_lossy());
    let args: String = ARGS
        .iter()
        .map(|a| format!("        <string>{a}</string>\n"))
        .collect();
    PLIST_TEMPLATE
        .replace("@LABEL@", LAUNCHD_LABEL)
        .replace("@EXE@", &exe)
        .replace("@ARGS@", &args)
        .replace("@HOME@", &home)
        .replace("@LOG@", &log)
}

/// The systemd user unit for `~/.config/systemd/user/weftos.service`.
pub fn systemd_user_unit(exe: &Path, home: &Path) -> String {
    let mut exec = systemd_quote(&exe.to_string_lossy());
    for a in ARGS {
        exec.push(' ');
        exec.push_str(a);
    }
    let home = systemd_path(&home.to_string_lossy());
    format!(
        "[Unit]\n\
Description=WeftOS per-user daemon\n\
Documentation=https://github.com/weave-logic-ai/weftos\n\
\n\
[Service]\n\
Type=simple\n\
WorkingDirectory={home}\n\
ExecStart={exec}\n\
Restart=on-failure\n\
RestartSec=3\n\
\n\
[Install]\n\
WantedBy=default.target\n"
    )
}

/// Which service manager a unit is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Launchd,
    Systemd,
}

impl UnitKind {
    /// Unit text for this manager.
    pub fn render(self, exe: &Path, home: &Path) -> String {
        match self {
            UnitKind::Launchd => launchd_plist(exe, home),
            UnitKind::Systemd => systemd_user_unit(exe, home),
        }
    }

    /// Conventional install location of the unit file.
    pub fn default_path(self, home: &Path) -> PathBuf {
        match self {
            UnitKind::Launchd => home
                .join("Library/LaunchAgents")
                .join(format!("{LAUNCHD_LABEL}.plist")),
            UnitKind::Systemd => home
                .join(".config/systemd/user")
                .join(format!("{SYSTEMD_UNIT}.service")),
        }
    }

    /// Install commands as text for a unit written to `file`. Never run.
    pub fn install_commands(self, file: &Path) -> Vec<String> {
        let f = file.display();
        match self {
            UnitKind::Launchd => vec![format!("launchctl bootstrap gui/$UID '{f}'")],
            UnitKind::Systemd => vec![
                "systemctl --user daemon-reload".to_owned(),
                format!("systemctl --user enable --now {SYSTEMD_UNIT}"),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn golden(name: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    #[test]
    fn launchd_matches_golden() {
        let got = launchd_plist(Path::new("/opt/weftos/bin/weaver"), Path::new("/Users/alice"));
        assert_eq!(got, golden("weftos-user.plist"));
    }

    #[test]
    fn systemd_matches_golden() {
        let got = systemd_user_unit(Path::new("/opt/weftos/bin/weaver"), Path::new("/home/alice"));
        assert_eq!(got, golden("weftos.service"));
    }

    #[test]
    fn no_hardcoded_install_prefix() {
        let exe = Path::new("/somewhere/else/weaver");
        for t in [
            launchd_plist(exe, Path::new("/h")),
            systemd_user_unit(exe, Path::new("/h")),
        ] {
            assert!(!t.contains("/usr/local/bin"));
            assert!(t.contains("/somewhere/else/weaver"));
        }
    }

    #[test]
    fn launchd_escapes_xml_in_paths() {
        let t = launchd_plist(Path::new("/a b/&<weaver>"), Path::new("/Users/jos\u{e9} \"x\""));
        assert!(t.contains("<string>/a b/&amp;&lt;weaver&gt;</string>"));
        assert!(t.contains("jos\u{e9} &quot;x&quot;/.weftos/run/kernel.log"));
        assert!(!t.contains("&<"));
    }

    #[test]
    fn systemd_quotes_exec_start() {
        let t = systemd_user_unit(Path::new("/a b/we\"av%er$"), Path::new("/h/100%"));
        assert!(t.contains("ExecStart=\"/a b/we\\\"av%%er$$\" kernel start --foreground --profile user\n"));
        assert!(t.contains("WorkingDirectory=/h/100%%\n"));
    }

    #[test]
    fn deleted_suffix_is_stripped() {
        assert_eq!(
            strip_deleted(Path::new("/opt/bin/weaver (deleted)")),
            PathBuf::from("/opt/bin/weaver")
        );
        assert_eq!(strip_deleted(Path::new("/opt/bin/weaver")), PathBuf::from("/opt/bin/weaver"));
    }

    #[test]
    fn install_commands_are_text_only() {
        let f = Path::new("/tmp/x.plist");
        assert_eq!(
            UnitKind::Launchd.install_commands(f),
            vec!["launchctl bootstrap gui/$UID '/tmp/x.plist'".to_owned()]
        );
        assert!(UnitKind::Systemd.install_commands(f)[1].ends_with("enable --now weftos"));
    }
}
