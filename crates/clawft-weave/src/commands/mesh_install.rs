//! `weaver mesh install-service` / `uninstall-service` (ADR-103 Phase 3,
//! package H).
//!
//! Both verbs only PRINT a shell script for an administrator to read and run.
//! Nothing is executed, no file is written, and there is no `--apply` form.
//! The script creates the service account and group, adds the invoking user
//! to that group (the runtime directory is group-owned 0750, so group
//! membership is what lets a user reach the socket), installs a root-owned
//! copy of the binary, writes `mesh.toml`, and installs the unit. Enabling is
//! the last, separate line (stop any collapsed user daemon first: it holds
//! 9489).

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use clap::{Args, ValueEnum};

use crate::service_units::{canonical_exe, sh_quote};
use crate::service_units_system::*;

/// Default mesh listener (S's decision: loopback unless the owner opens it).
pub const DEFAULT_LISTEN: &str = "127.0.0.1:9489";

/// Target service manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Manager {
    Launchd,
    Systemd,
}

impl Manager {
    /// launchd on macOS, systemd elsewhere.
    pub fn host_default() -> Self {
        if cfg!(target_os = "macos") { Manager::Launchd } else { Manager::Systemd }
    }
}

/// Shared by both verbs.
#[derive(Args, Debug, Clone)]
pub struct InstallArgs {
    /// Service manager (default: this host's).
    #[arg(long, value_enum)]
    pub kind: Option<Manager>,
    /// Refused: these verbs only print a script; nothing is applied.
    #[arg(long, hide = true)]
    pub apply: bool,
    /// Copy an existing node key (the collapsed daemon's `~/.weftos/run/node.key`)
    /// into the service state so the machine keeps its node id (install only).
    #[arg(long, value_name = "PATH")]
    pub adopt_node_key: Option<PathBuf>,
    /// Mesh listener written to mesh.toml (non-loopback exposes it to the network).
    #[arg(long, default_value = DEFAULT_LISTEN)]
    pub listen: String,
    /// Extra uid(s) allowed to run admin verbs (root always may).
    #[arg(long = "admin-uid")]
    pub admin_uids: Vec<u32>,
    /// uninstall only: also delete /var/lib/weftos/mesh/node.key.
    #[arg(long)]
    pub purge_key: bool,
}

/// What the printer needs, all injectable.
#[derive(Debug, Clone)]
pub struct Plan {
    pub manager: Manager,
    /// The `weaver` to copy into [`SERVICE_EXE`].
    pub exe: PathBuf,
    pub adopt_node_key: Option<PathBuf>,
    pub listen: String,
    pub admin_uids: Vec<u32>,
    pub purge_key: bool,
}

fn no_control(what: &str, p: &Path) -> Result<()> {
    if p.to_string_lossy().chars().any(char::is_control) {
        bail!("the {what} path contains a control character; refusing to put it in a script");
    }
    Ok(())
}

impl Plan {
    /// Validate and build from arguments.
    pub fn from_args(a: &InstallArgs, exe: &Path) -> Result<Self> {
        if a.apply {
            bail!("--apply is refused: `weaver mesh install-service` only prints a script for an administrator to review and run");
        }
        no_control("weaver binary", exe)?;
        if let Some(k) = &a.adopt_node_key {
            no_control("--adopt-node-key", k)?;
        }
        if a.listen.chars().any(|c| c.is_control() || c == '"' || c == '\\') || a.listen.trim().is_empty() {
            bail!("--listen must be host:port without quotes or control characters");
        }
        Ok(Plan {
            manager: a.kind.unwrap_or_else(Manager::host_default),
            exe: canonical_exe(exe),
            adopt_node_key: a.adopt_node_key.clone(),
            listen: a.listen.clone(),
            admin_uids: a.admin_uids.clone(),
            purge_key: a.purge_key,
        })
    }

    fn account(&self) -> &'static str {
        match self.manager {
            Manager::Launchd => MACOS_ACCOUNT,
            Manager::Systemd => LINUX_ACCOUNT,
        }
    }

    fn group(&self) -> &'static str {
        match self.manager {
            Manager::Launchd => macos_group(),
            Manager::Systemd => linux_group(),
        }
    }

    fn root_group(&self) -> &'static str {
        match self.manager {
            Manager::Launchd => "wheel",
            Manager::Systemd => "root",
        }
    }
}

/// `mesh.toml` the script writes (only when none exists).
pub fn mesh_toml(p: &Plan) -> String {
    let mut t = format!(
        "# WeftOS machine mesh service (written by `weaver mesh install-service`).\n\
state_dir = \"{STATE_DIR}\"\n\
socket = \"{SOCKET}\"\n\
listen = \"{}\"\n\
health_listen = \"127.0.0.1:9490\"\n",
        p.listen
    );
    if !p.admin_uids.is_empty() {
        let ids: Vec<String> = p.admin_uids.iter().map(u32::to_string).collect();
        t.push_str(&format!("admin_uids = [{}]\n", ids.join(", ")));
    }
    t
}

fn heredoc(out: &mut String, path: &str, mode: &str, owner: &str, body: &str) {
    out.push_str(&format!("cat > {path}.new <<'WEFTOS_EOF'\n{body}WEFTOS_EOF\n"));
    out.push_str(&format!("chown {owner} {path}.new && chmod {mode} {path}.new && mv {path}.new {path}\n"));
}

/// The install script.
pub fn install_script(p: &Plan) -> String {
    let (acct, group, rg) = (p.account(), p.group(), p.root_group());
    let mut s = String::new();
    s.push_str(&format!(
        "#!/bin/sh\n\
# WeftOS machine mesh service install ({}). PRINTED by `weaver mesh install-service`;\n\
# nothing has been run. Read it, then run it as an administrator (sudo sh install.sh).\n\
#\n\
# What it does:\n\
#  - creates the {acct} account and the {group} group (no login, never root)\n\
#  - adds YOU (the invoking user) to the {group} group: {RUN_DIR} is owned by that group with\n\
#    mode 0750, so only group members can reach the mesh socket. Log out and in (or start a new\n\
#    login session) for the membership to apply.\n\
#  - installs a root-owned copy of weaver at {SERVICE_EXE} (the service never runs from a\n\
#    user-writable path)\n\
#  - writes {MESH_TOML} (only if absent) and installs the unit; it does NOT start the service\n\
set -eu\n\
[ \"$(id -u)\" -eq 0 ] || {{ echo 'run this script as root (sudo sh install.sh)' >&2; exit 1; }}\n\
TARGET_USER=\"${{SUDO_USER:-}}\"\n\
[ -n \"$TARGET_USER\" ] && [ \"$TARGET_USER\" != root ] || {{ echo 'run it with sudo from your own account so the group member can be your user' >&2; exit 1; }}\n\n",
        match p.manager {
            Manager::Launchd => "launchd",
            Manager::Systemd => "systemd",
        }
    ));

    s.push_str("# --- account and group ---\n");
    match p.manager {
        Manager::Launchd => s.push_str(&macos_account_snippet()),
        Manager::Systemd => {
            heredoc(&mut s, SYSUSERS_PATH, "0644", &format!("root:{rg}"), &sysusers_conf());
            s.push_str(&format!("systemd-sysusers {SYSUSERS_PATH}\n"));
        }
    }
    s.push_str(&format!("# add {group} membership for the invoking user\n"));
    match p.manager {
        Manager::Launchd => s.push_str(&format!("dseditgroup -o edit -a \"$TARGET_USER\" -t user {group}\n")),
        Manager::Systemd => s.push_str(&format!("usermod -aG {group} \"$TARGET_USER\"\n")),
    }

    s.push_str("\n# --- directories and modes ---\n");
    s.push_str(&format!(
        "install -d -m 0755 -o root -g {rg} /etc/weftos /var/lib/weftos /usr/local/libexec/weftos\n\
install -d -m 0700 -o {acct} -g {group} {STATE_DIR}\n\
install -d -m 0750 -o {acct} -g {group} {RUN_DIR}\n"
    ));
    if p.manager == Manager::Launchd {
        s.push_str(&format!("install -d -m 0750 -o {acct} -g {group} {LOG_DIR}\n"));
    }

    s.push_str("\n# --- binary (root-owned copy) ---\n");
    s.push_str(&format!(
        "install -m 0755 -o root -g {rg} {} {SERVICE_EXE}\n",
        sh_quote(&p.exe.to_string_lossy())
    ));

    if let Some(key) = &p.adopt_node_key {
        s.push_str("\n# --- adopt the existing node key (the key will exist in two places; node id unchanged) ---\n");
        s.push_str(&format!(
            "[ ! -e {STATE_DIR}/node.key ] || {{ echo '{STATE_DIR}/node.key already exists; not overwriting' >&2; exit 1; }}\n\
install -m 0600 -o {acct} -g {group} {} {STATE_DIR}/node.key\n",
            sh_quote(&key.to_string_lossy())
        ));
    }

    s.push_str("\n# --- configuration (kept if already present) ---\n");
    s.push_str(&format!("if [ ! -e {MESH_TOML} ]; then\n"));
    heredoc(&mut s, MESH_TOML, "0644", &format!("root:{rg}"), &mesh_toml(p));
    s.push_str("fi\n");

    s.push_str("\n# --- unit ---\n");
    let (exe, state, sock) = (Path::new(SERVICE_EXE), Path::new(STATE_DIR), Path::new(SOCKET));
    match p.manager {
        Manager::Launchd => {
            heredoc(&mut s, RUNDIR_PLIST_PATH, "0644", "root:wheel", &launchd_rundir_plist());
            heredoc(&mut s, MESH_PLIST_PATH, "0644", "root:wheel", &launchd_system_plist(exe, state, sock));
            s.push_str(&format!("# macOS clears /var/run at boot; {MESH_RUNDIR_LABEL} recreates {RUN_DIR} (group {group}, 0750).\n"));
            s.push_str(&format!("launchctl bootstrap system {RUNDIR_PLIST_PATH} || true\n"));
        }
        Manager::Systemd => {
            heredoc(&mut s, MESH_UNIT_PATH, "0644", "root:root", &systemd_system_unit(exe, state, sock));
            s.push_str("systemctl daemon-reload\n");
        }
    }

    s.push_str("\n# --- done; NOT started ---\n");
    s.push_str(&format!(
        "echo 'installed. {group} membership for '\"$TARGET_USER\"' takes effect at next login.'\n\
echo 'stop any collapsed user daemon first (it holds 9489): weaver kernel stop'\n\
echo 'then enable the service with:'\n"
    ));
    let enable = match p.manager {
        Manager::Launchd => format!("launchctl bootstrap system {MESH_PLIST_PATH}"),
        Manager::Systemd => format!("systemctl enable --now {MESH_SYSTEMD_UNIT}"),
    };
    s.push_str(&format!("echo '  sudo {enable}'\n"));
    s.push_str("echo 'afterwards: weaver mesh status, compare the machine key fingerprint out of band, weaver mesh trust'\n");
    s.push_str(&format!("# ENABLE (exact command): {enable}\n"));
    s
}

/// The inverse. Keeps `node.key` and the state directory unless `purge_key`.
pub fn uninstall_script(p: &Plan) -> String {
    let (acct, group) = (p.account(), p.group());
    let mut s = String::from(
        "#!/bin/sh\n\
# WeftOS machine mesh service uninstall. PRINTED by `weaver mesh uninstall-service`; nothing has been run.\n\
# Run as root after reading. It stops and removes the unit, the root-owned binary and the runtime directory.\n\
# It KEEPS the state directory, the journal, the log directory, mesh.toml and the account/group\n\
# (account removal is listed below, commented out).\n\
set -eu\n\
[ \"$(id -u)\" -eq 0 ] || { echo 'run this script as root' >&2; exit 1; }\n\n",
    );
    match p.manager {
        Manager::Launchd => s.push_str(&format!(
            "launchctl bootout system/{MESH_LAUNCHD_LABEL} || true\n\
launchctl bootout system/{MESH_RUNDIR_LABEL} || true\n\
rm -f {MESH_PLIST_PATH} {RUNDIR_PLIST_PATH}\n"
        )),
        Manager::Systemd => s.push_str(&format!(
            "systemctl disable --now {MESH_SYSTEMD_UNIT} || true\n\
rm -f {MESH_UNIT_PATH} {SYSUSERS_PATH}\n\
systemctl daemon-reload\n"
        )),
    }
    s.push_str(&format!("rm -f {SERVICE_EXE}\nrmdir /usr/local/libexec/weftos 2>/dev/null || true\nrm -rf {RUN_DIR}\n"));
    if p.purge_key {
        s.push_str(&format!(
            "\n# --purge-key was given: the box identity is destroyed; peers that pinned it will not recognise this machine.\n\
rm -f {STATE_DIR}/node.key\n"
        ));
    } else {
        s.push_str(&format!(
            "\n# {STATE_DIR}/node.key is KEPT (pass --purge-key to remove it). The state directory and journal are kept too.\n"
        ));
    }
    s.push_str(&format!(
        "\n# Account removal (commented out). A later account that reuses the uid would inherit its binds:\n\
# run `weaver mesh bind revoke <uid>` for every bound uid first, and see docs/guides.\n"
    ));
    match p.manager {
        Manager::Launchd => s.push_str(&format!(
            "# dseditgroup -o edit -d USER -t user {group}   (for each member)\n# dscl . -delete /Users/{acct}\n# dscl . -delete /Groups/{group}\n"
        )),
        Manager::Systemd => s.push_str(&format!(
            "# gpasswd -d USER {group}   (for each member)\n# userdel {acct}\n"
        )),
    }
    s
}

/// Entry points from `mesh_cmd`.
pub fn run_install(a: &InstallArgs, w: &mut dyn Write) -> Result<()> {
    let p = Plan::from_args(a, &std::env::current_exe()?)?;
    if !p.listen.starts_with("127.") && !p.listen.starts_with("[::1]") && !p.listen.starts_with("localhost") {
        eprintln!("note: listen {} exposes the mesh port beyond this machine", p.listen);
    }
    w.write_all(install_script(&p).as_bytes())?;
    Ok(())
}

pub fn run_uninstall(a: &InstallArgs, w: &mut dyn Write) -> Result<()> {
    if a.adopt_node_key.is_some() {
        bail!("--adopt-node-key applies to install-service only");
    }
    let p = Plan::from_args(a, &std::env::current_exe()?)?;
    w.write_all(uninstall_script(&p).as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(m: Manager) -> Plan {
        Plan {
            manager: m,
            exe: PathBuf::from("/opt/we ftos/it's/weaver"),
            adopt_node_key: None,
            listen: DEFAULT_LISTEN.into(),
            admin_uids: vec![],
            purge_key: false,
        }
    }

    fn check_golden(name: &str, got: &str) {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&p, got).unwrap();
        }
        assert_eq!(got, std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display())), "{name}");
    }

    #[test]
    fn scripts_match_golden() {
        let mut adopt = plan(Manager::Launchd);
        adopt.adopt_node_key = Some(PathBuf::from("/Users/a b/.weftos/run/node.key"));
        adopt.admin_uids = vec![501];
        check_golden("mesh-install-launchd.sh", &install_script(&adopt));
        check_golden("mesh-install-systemd.sh", &install_script(&plan(Manager::Systemd)));
        check_golden("mesh-uninstall-launchd.sh", &uninstall_script(&plan(Manager::Launchd)));
        check_golden("mesh-uninstall-systemd.sh", &uninstall_script(&plan(Manager::Systemd)));
    }

    #[test]
    fn paths_with_spaces_and_quotes_are_shell_quoted() {
        let s = install_script(&plan(Manager::Systemd));
        assert!(s.contains("install -m 0755 -o root -g root '/opt/we ftos/it'\\''s/weaver' /usr/local/libexec/weftos/weaver"));
    }

    #[test]
    fn group_membership_and_modes_are_stated() {
        let l = install_script(&plan(Manager::Launchd));
        assert!(l.contains("dseditgroup -o edit -a \"$TARGET_USER\" -t user _weftos"));
        assert!(l.contains("install -d -m 0750 -o _weftos -g _weftos /var/run/weftos"));
        assert!(l.contains("mode 0750, so only group members"));
        let d = install_script(&plan(Manager::Systemd));
        assert!(d.contains("usermod -aG weftos \"$TARGET_USER\""));
        assert!(d.contains("ENABLE (exact command): systemctl enable --now weftos-mesh"));
    }

    #[test]
    fn default_listen_is_loopback_and_script_parses() {
        assert!(mesh_toml(&plan(Manager::Systemd)).contains("listen = \"127.0.0.1:9489\""));
        for m in [Manager::Launchd, Manager::Systemd] {
            for text in [install_script(&plan(m)), uninstall_script(&plan(m))] {
                if let Ok(mut c) = std::process::Command::new("sh").arg("-n").stdin(std::process::Stdio::piped()).spawn() {
                    use std::io::Write as _;
                    c.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
                    assert!(c.wait().unwrap().success(), "sh -n failed");
                }
            }
        }
    }

    #[test]
    fn generated_mesh_toml_loads() {
        let mut p = plan(Manager::Systemd);
        p.admin_uids = vec![501, 502];
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("mesh.toml");
        std::fs::write(&f, mesh_toml(&p)).unwrap();
        let cfg = clawft_mesh_service::MeshServiceConfig::load(Some(&f), &clawft_mesh_service::Overrides::default()).unwrap();
        assert_eq!(cfg.listen, "127.0.0.1:9489");
        assert_eq!(cfg.admin_uids, vec![501, 502]);
    }

    #[test]
    fn apply_is_refused_and_uninstall_keeps_the_key() {
        let a = InstallArgs { kind: None, apply: true, adopt_node_key: None, listen: DEFAULT_LISTEN.into(), admin_uids: vec![], purge_key: false };
        assert!(Plan::from_args(&a, Path::new("/x/weaver")).unwrap_err().to_string().contains("--apply is refused"));
        for m in [Manager::Launchd, Manager::Systemd] {
            let keep = uninstall_script(&plan(m));
            assert!(!keep.contains("rm -f /var/lib/weftos/mesh/node.key"));
            let mut purge = plan(m);
            purge.purge_key = true;
            assert!(uninstall_script(&purge).contains("rm -f /var/lib/weftos/mesh/node.key"));
        }
    }

    #[test]
    fn control_characters_are_refused() {
        let a = InstallArgs { kind: None, apply: false, adopt_node_key: Some("/k\nrm -rf /".into()), listen: DEFAULT_LISTEN.into(), admin_uids: vec![], purge_key: false };
        assert!(Plan::from_args(&a, Path::new("/x/weaver")).is_err());
    }
}
