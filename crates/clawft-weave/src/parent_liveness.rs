//! Owned stdin liveness pipe (ADR-103 D10).
//!
//! A supervisor that must not leave a child behind when it dies (a nested user
//! instance for its inner user daemon, and for the project kernels it
//! supervises) spawns the child with a piped stdin and keeps the write end. The
//! kernel closes it on any death of the owner, SIGKILL included, so the child
//! sees EOF within bounded time and runs its normal SIGTERM shutdown, with a
//! hard exit if that wedges. A top-level user daemon does not do this for its
//! project children: they outlive a restart on purpose and are re-adopted.

/// Set by a supervisor that handed the child a stdin liveness pipe.
pub const ENV: &str = "WEFTOS_PARENT_LIVENESS";

/// Exit code when graceful shutdown wedged after the owner died.
const WEDGED_EXIT: i32 = 78;

/// Grace between the SIGTERM cascade and the hard exit.
const GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// True when this process was started with an owned liveness pipe on stdin.
pub fn requested() -> bool {
    std::env::var_os(ENV).is_some_and(|v| v == "stdin")
}

/// Watch stdin: EOF or a read error means the owner is gone.
pub fn spawn_stdin_watcher() {
    std::thread::spawn(|| {
        use std::io::Read;
        let mut byte = [0u8; 1];
        loop {
            match std::io::stdin().read(&mut byte) {
                Ok(0) | Err(_) => {
                    // Use the normal shutdown cascade, then bound any wedged exit.
                    unsafe {
                        nix::libc::kill(nix::libc::getpid(), nix::libc::SIGTERM);
                    }
                    std::thread::sleep(GRACE);
                    std::process::exit(WEDGED_EXIT);
                }
                Ok(_) => {}
            }
        }
    });
}
