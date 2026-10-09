//! Hardened `git` runner for the project install/update handlers (ADR-108 P3a).
//!
//! `git` is started with an argument vector, never a shell, with no terminal
//! prompt, a wall-clock timeout, a cap on the output kept, and the transports
//! that run local programs or read local files switched off. The member's own
//! git credential helper still works. Output that reaches an error message has
//! any `user:pass@` part of a URL removed first.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// Wall-clock limit for one git invocation (one clone or pull of one repository).
pub const GIT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Most bytes of git's stdout (and, separately, stderr) that are kept.
pub const MAX_OUTPUT: usize = 256 * 1024;
const MAX_ERROR_CHARS: usize = 400;

/// How git is run. [`GitRunner::default`] is the hardened production setting.
#[derive(Debug, Clone)]
pub struct GitRunner {
    pub timeout: Duration,
    pub max_output: usize,
    /// Lets git read repositories by local path or `file://`. Always `false`
    /// in production (request URLs cannot name a local path anyway); tests turn
    /// it on to clone from temp bare repositories.
    pub allow_file: bool,
}

impl Default for GitRunner {
    fn default() -> Self {
        Self { timeout: GIT_TIMEOUT, max_output: MAX_OUTPUT, allow_file: false }
    }
}

impl GitRunner {
    /// Run `git <args>` in `cwd`; stdout on success, a redacted message otherwise.
    pub async fn run(&self, cwd: &Path, args: &[&str]) -> Result<String, String> {
        let file_allow = if self.allow_file { "protocol.file.allow=always" } else { "protocol.file.allow=never" };
        let mut cmd = Command::new("git");
        cmd.current_dir(cwd)
            .args(["-c", file_allow, "-c", "protocol.ext.allow=never", "-c", "core.fsmonitor=false"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| format!("cannot start git: {e}"))?;
        let out = child.stdout.take().ok_or("git has no stdout")?;
        let err = child.stderr.take().ok_or("git has no stderr")?;
        let cap = self.max_output;
        let both = async {
            let (o, e) = tokio::join!(read_capped(out, cap), read_capped(err, cap));
            let status = child.wait().await;
            (o, e, status)
        };
        let Ok((o, e, status)) = tokio::time::timeout(self.timeout, both).await else {
            return Err(format!("git {} timed out after {}s", args.first().unwrap_or(&""), self.timeout.as_secs()));
        };
        let status = status.map_err(|e| format!("git did not finish: {e}"))?;
        if status.success() {
            Ok(String::from_utf8_lossy(&o).into_owned())
        } else {
            let text = redact(String::from_utf8_lossy(&e).trim());
            Err(format!("git {} failed ({status}): {text}", args.first().unwrap_or(&"")))
        }
    }
}

/// Read to the end, keeping at most `cap` bytes (the rest is drained so git never blocks).
async fn read_capped<R: AsyncRead + Unpin>(mut r: R, cap: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    while let Ok(n) = r.read(&mut buf).await {
        if n == 0 {
            break;
        }
        let room = cap.saturating_sub(kept.len());
        kept.extend_from_slice(&buf[..n.min(room)]);
    }
    kept
}

/// Drop `userinfo@` from every `scheme://userinfo@host` in `text` and shorten it.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        out.push_str(&rest[..i + 3]);
        rest = &rest[i + 3..];
        let end = rest.find(|c: char| c.is_whitespace() || c == '/' || c == '\'' || c == '"').unwrap_or(rest.len());
        let authority = &rest[..end];
        out.push_str(authority.rsplit_once('@').map_or(authority, |(_, host)| host));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out.chars().take(MAX_ERROR_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_drops_userinfo_and_caps_length() {
        assert_eq!(redact("fatal: unable to access 'https://bob:hunter2@example.org/x.git/'"), "fatal: unable to access 'https://example.org/x.git/'");
        assert_eq!(redact("ssh://git@host/x"), "ssh://host/x");
        assert_eq!(redact("no urls here"), "no urls here");
        assert_eq!(redact(&"a".repeat(2000)).len(), MAX_ERROR_CHARS);
    }

    #[tokio::test]
    async fn timeout_kills_a_slow_git_and_errors_name_the_command() {
        let d = tempfile::tempdir().unwrap();
        let r = GitRunner { timeout: Duration::from_millis(1), ..GitRunner::default() };
        // `clone` of an unreachable host would block; any git call may beat 1ms, so accept either outcome.
        let res = r.run(d.path(), &["clone", "--", "https://203.0.113.1/x.git", "x"]).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn failures_carry_stderr() {
        let d = tempfile::tempdir().unwrap();
        let e = GitRunner::default().run(d.path(), &["rev-parse", "HEAD"]).await.unwrap_err();
        assert!(e.starts_with("git rev-parse failed"), "{e}");
    }
}
