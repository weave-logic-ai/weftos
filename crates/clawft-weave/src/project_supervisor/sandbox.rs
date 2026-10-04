//! OS sandbox applied to a project kernel before it executes. A requested
//! driver never falls back to `logical` when unavailable or invalid.

use std::path::Path;

use clawft_types::project::ProjectSandbox;

/// Internal helper argv. The supervisor spawns its own binary, applies the
/// profile before starting threads, then execs the target kernel in place.
pub const HELPER_ARG: &str = "--weftos-sandbox-exec";

/// Run the internal Seatbelt helper, if `args` starts with [`HELPER_ARG`].
/// The helper and target have one PID, preserving spawn proof and adoption.
pub fn run_helper(args: &[String]) -> Result<bool, String> {
    if args.first().map(String::as_str) != Some(HELPER_ARG) {
        return Ok(false);
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::CommandExt as _;
        let target = args
            .get(1)
            .ok_or("sandbox helper needs a target executable")?;
        seatbelt::enter_from_env()?;
        let error = std::process::Command::new(target).args(&args[2..]).exec();
        Err(format!("sandboxed exec of {target} failed: {error}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Seatbelt helper is available only on macOS".into())
    }
}

/// Configure a command for the manifest's sandbox. Called after the run dir
/// has been populated and before `spawn`, while the parent still owns it.
pub(super) fn configure(
    command: &mut std::process::Command,
    driver: ProjectSandbox,
    root: &Path,
    run_dir: &Path,
    parent_socket: &Path,
    exe: &Path,
) -> Result<(), String> {
    match driver {
        ProjectSandbox::Logical => Ok(()),
        ProjectSandbox::LinuxContainer => {
            Err("container driver requires the container launcher".into())
        }
        ProjectSandbox::Seatbelt => {
            #[cfg(target_os = "macos")]
            {
                seatbelt::configure(command, root, run_dir, parent_socket, exe)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (command, root, run_dir, parent_socket, exe);
                Err("seatbelt is available only on macOS".into())
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod seatbelt {
    use std::ffi::{CString, c_char};
    use std::path::Path;

    #[link(name = "System")]
    unsafe extern "C" {
        fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> i32;
    }

    fn literal(path: &Path) -> Result<String, String> {
        let p = path
            .canonicalize()
            .or_else(|_| {
                let parent = path
                    .parent()
                    .ok_or_else(|| std::io::Error::other("path has no parent"))?;
                let name = path
                    .file_name()
                    .ok_or_else(|| std::io::Error::other("path has no name"))?;
                Ok::<_, std::io::Error>(parent.canonicalize()?.join(name))
            })
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let s = p
            .to_str()
            .ok_or_else(|| format!("{} is not UTF-8", p.display()))?;
        // Seatbelt's profile language uses quoted string literals. Escaping
        // these two characters prevents a path from injecting profile rules.
        Ok(format!(
            "\"{}\"",
            s.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    }

    /// Default deny. A data read of the literal root directory is needed by dyld on
    /// current macOS; it does not grant a subpath read of files under `/`.
    /// The child may read system libraries and its executable,
    /// and read/write only the project and its run dir. Its only network paths
    /// are the parent UDS and its own kernel UDS. Tool network access needs a
    /// separately governed profile extension; this driver does not grant it.
    fn profile(
        root: &Path,
        run: &Path,
        parent_socket: &Path,
        exe: &Path,
    ) -> Result<CString, String> {
        let root = literal(root)?;
        let run_literal = literal(run)?;
        let parent_socket = literal(parent_socket)?;
        let exe = literal(exe)?;
        let socket = literal(&run.join("kernel.sock"))?;
        let text = format!(
            "(version 1)\n(deny default)\n\
             (allow process-exec process-fork)\n(allow sysctl-read)\n(allow mach-lookup)\n\
             (allow network-outbound (literal {parent_socket}))\n\
             (allow network-bind network-inbound (literal {socket}))\n\
             (allow file-read-data (literal \"/\"))\n\
             (allow file-read* file-test-existence file-map-executable \
              (subpath \"/System\") (subpath \"/System/Volumes/Preboot/Cryptexes\") \
              (subpath \"/usr\") (subpath \"/bin\") (subpath \"/sbin\") \
              (subpath \"/Library\") (subpath \"/opt/homebrew\") \
              (subpath \"/etc\") (subpath \"/private/etc\") (subpath \"/private/var/db\") \
              (subpath \"/dev\") (literal {exe}) (literal {parent_socket}))\n\
             (allow file-read* file-write* (subpath {root}) (subpath {run_literal}))\n\
             (deny file-write* (literal {pin}) (literal {policy}))\n",
            pin = literal(&run.join("user.pub"))?,
            policy = literal(&run.join("parent-policy.json"))?,
        );
        CString::new(text).map_err(|_| "sandbox profile contains a NUL byte".into())
    }

    pub(super) fn configure(
        command: &mut std::process::Command,
        root: &Path,
        run_dir: &Path,
        parent_socket: &Path,
        exe: &Path,
    ) -> Result<(), String> {
        let profile = profile(root, run_dir, parent_socket, exe)?;
        command.env(
            "WEFTOS_SANDBOX_PROFILE",
            profile.to_str().map_err(|e| e.to_string())?,
        );
        Ok(())
    }

    pub(super) fn enter_from_env() -> Result<(), String> {
        let profile = std::env::var("WEFTOS_SANDBOX_PROFILE")
            .map_err(|_| "sandbox helper has no profile".to_owned())?;
        // SAFETY: this helper is entered before any Tokio runtime or user
        // thread starts. The profile has already been parsed in the parent.
        unsafe { std::env::remove_var("WEFTOS_SANDBOX_PROFILE") };
        let profile =
            CString::new(profile).map_err(|_| "sandbox profile contains NUL".to_owned())?;
        let mut error: *mut c_char = std::ptr::null_mut();
        let rc = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
        if rc == 0 {
            return Ok(());
        }
        let detail = if error.is_null() {
            "no diagnostic".to_owned()
        } else {
            unsafe { std::ffi::CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        };
        Err(format!("sandbox_init failed: {detail}"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn profile_has_no_global_network_or_home_access() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("project");
            let run = root.join("run");
            std::fs::create_dir_all(&run).unwrap();
            let text = profile(
                &root,
                &run,
                &run.join("parent.sock"),
                Path::new("/usr/bin/true"),
            )
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
            assert!(!text.contains("(allow network*)"));
            assert!(!text.contains("(allow process*)"));
            assert!(!text.contains("(subpath \"/Users\")"));
            assert!(text.contains("(allow network-outbound (literal"));
            assert!(text.contains("(allow file-read-data (literal \"/\"))"));
            assert!(!text.contains("(allow file-read* (subpath \"/\")"));
            assert!(text.contains("(deny file-write* (literal"));
        }

        #[test]
        fn quoted_path_is_escaped_in_profile() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("quote\"(allow default)");
            let run = root.join("run");
            std::fs::create_dir_all(&run).unwrap();
            let text = profile(
                &root,
                &run,
                &run.join("parent.sock"),
                Path::new("/usr/bin/true"),
            )
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
            assert!(text.contains("quote\\\"(allow default)"));
            assert!(!text.contains("quote\"(allow default)"));
        }
    }
}
