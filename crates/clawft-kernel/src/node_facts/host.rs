//! The seam between probes and the machine: [`ProbeHost`].
//!
//! Probes never touch the OS directly; they ask a host to run a tool, read
//! a file or list a directory. [`SystemHost`] does it for real (with a
//! per-command timeout and no shell); tests supply a fake host with canned
//! outputs, so a Linux ARM board or a Coral TPU can be simulated on a Mac.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Largest tool output a probe reads, in bytes.
pub const MAX_OUTPUT: usize = 1 << 20;

/// What probes may ask of the machine.
pub trait ProbeHost {
    /// `std::env::consts::OS` of the probed machine (`macos`, `linux`).
    fn os(&self) -> String;
    /// `std::env::consts::ARCH` of the probed machine (`aarch64`, `arm`).
    fn arch(&self) -> String;
    /// Resolve a tool name to an executable path, if installed.
    fn which(&self, tool: &str) -> Option<PathBuf>;
    /// Run a tool (resolved with [`Self::which`]) and return stdout when it
    /// exits 0 within the timeout.
    fn run(&self, tool: &str, args: &[&str]) -> Option<String>;
    /// Like [`Self::run`] but returns stdout followed by stderr (for tools
    /// that print their version on stderr, such as `llama-server`).
    fn run_all(&self, tool: &str, args: &[&str]) -> Option<String>;
    /// Read a small text file.
    fn read_file(&self, path: &str) -> Option<String>;
    /// List directory entry names.
    fn list_dir(&self, path: &str) -> Option<Vec<String>>;
    /// True if the path exists.
    fn exists(&self, path: &str) -> bool;
}

/// The real machine.
#[derive(Debug, Clone)]
pub struct SystemHost {
    timeout: Duration,
    extra_dirs: Vec<PathBuf>,
}

impl Default for SystemHost {
    fn default() -> Self {
        Self::new(Duration::from_secs(8))
    }
}

type Pipe = Option<Box<dyn Read + Send>>;

fn drain(r: Pipe) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(r) = r {
            let _ = r.take(MAX_OUTPUT as u64).read_to_end(&mut buf);
        }
        buf
    })
}

impl SystemHost {
    /// A host whose commands are killed after `timeout`.
    ///
    /// Besides `PATH`, tools are looked up in the usual package-manager
    /// directories, because a daemon often starts with a minimal `PATH`.
    pub fn new(timeout: Duration) -> Self {
        let mut extra_dirs: Vec<PathBuf> = [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        if let Some(home) = std::env::var_os("HOME") {
            extra_dirs.push(PathBuf::from(home).join(".local/bin"));
        }
        Self {
            timeout,
            extra_dirs,
        }
    }

    fn exec(&self, tool: &str, args: &[&str], merge_stderr: bool) -> Option<String> {
        let exe = self.which(tool)?;
        let mut child = Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(if merge_stderr {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .spawn()
            .ok()?;
        // Drain the pipes on threads so a chatty tool cannot fill a pipe and
        // block before we notice it has exited.
        let out = drain(
            child
                .stdout
                .take()
                .map(|r| Box::new(r) as Box<dyn Read + Send>),
        );
        let err = drain(
            child
                .stderr
                .take()
                .map(|r| Box::new(r) as Box<dyn Read + Send>),
        );
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        // On failure or timeout the readers are detached, not joined: a
        // grandchild may still hold a pipe open.
        match status {
            Some(s) if s.success() => {
                let mut bytes = out.join().ok()?;
                bytes.extend(err.join().ok()?);
                Some(String::from_utf8_lossy(&bytes).into_owned())
            }
            _ => None,
        }
    }
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

impl ProbeHost for SystemHost {
    fn os(&self) -> String {
        std::env::consts::OS.to_string()
    }

    fn arch(&self) -> String {
        std::env::consts::ARCH.to_string()
    }

    fn which(&self, tool: &str) -> Option<PathBuf> {
        if tool.is_empty() || tool.contains('/') {
            return None;
        }
        let path_dirs = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .unwrap_or_default();
        path_dirs
            .into_iter()
            .chain(self.extra_dirs.iter().cloned())
            .map(|d| d.join(tool))
            .find(|p| is_executable(p))
    }

    fn run(&self, tool: &str, args: &[&str]) -> Option<String> {
        self.exec(tool, args, false)
    }

    fn run_all(&self, tool: &str, args: &[&str]) -> Option<String> {
        self.exec(tool, args, true)
    }

    fn read_file(&self, path: &str) -> Option<String> {
        let f = std::fs::File::open(path).ok()?;
        let mut buf = Vec::new();
        f.take(MAX_OUTPUT as u64).read_to_end(&mut buf).ok()?;
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    fn list_dir(&self, path: &str) -> Option<Vec<String>> {
        let mut names: Vec<String> = std::fs::read_dir(path)
            .ok()?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        Some(names)
    }

    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_rejects_paths_and_finds_sh() {
        let h = SystemHost::default();
        assert!(h.which("../sh").is_none());
        assert!(h.which("").is_none());
        assert!(h.which("sh").is_some());
    }

    #[test]
    fn run_returns_stdout_only_on_success() {
        let h = SystemHost::default();
        assert_eq!(
            h.run("sh", &["-c", "echo hi; echo err >&2"]).as_deref(),
            Some("hi\n")
        );
        assert_eq!(
            h.run_all("sh", &["-c", "echo hi; echo err >&2"]).as_deref(),
            Some("hi\nerr\n")
        );
        assert!(h.run("sh", &["-c", "exit 3"]).is_none());
    }

    #[test]
    fn run_kills_a_tool_that_overruns_the_timeout() {
        let h = SystemHost::new(Duration::from_millis(200));
        let t = Instant::now();
        assert!(h.run("sh", &["-c", "sleep 5"]).is_none());
        assert!(t.elapsed() < Duration::from_secs(3));
    }
}
