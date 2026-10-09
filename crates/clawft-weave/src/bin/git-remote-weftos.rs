//! `git-remote-weftos`: git's remote helper for `weftos://<node>/<ULID>/<dir>`
//! URLs (ADR-108 P3b). Git runs it for clone, fetch and pull; it speaks the
//! remote-helper protocol on stdin/stdout and talks to the local WeftOS user
//! daemon over its RPC socket, which forwards each `project.fetch` call to the
//! project's primary node on the signed mesh channel.
//!
//! Capabilities: `fetch` only (read-only; push goes through the project's
//! normal git remotes). `list` answers the primary's branches, tags and HEAD;
//! `fetch` asks the primary for a bundle of the wanted refs minus every ref tip
//! this repository already has, then `git bundle unbundle`s it, so a later pull
//! moves only the new commits.
//!
//! Debug lines go to stderr when `GIT_TRANSPORT_HELPER_DEBUG` is set. Nothing
//! secret is ever on the command line or in the output.
//!
//! The helper needs the daemon's Unix socket and the placement modules, so on
//! other platforms it only explains that and exits.

#[cfg(unix)]
#[path = "git_remote_weftos/unix.rs"]
mod imp;

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    imp::main()
}

#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    eprintln!("git-remote-weftos: weftos:// remotes are supported on Unix only");
    std::process::ExitCode::from(1)
}
