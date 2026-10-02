//! Count of open daemon streams (`ipc.subscribe_stream` forwarders), one of
//! the child kernel's idle-stop inputs (`mesh.heartbeat` `busy.streams`).

use std::sync::atomic::{AtomicU32, Ordering};

static OPEN: AtomicU32 = AtomicU32::new(0);

/// Open streams right now.
pub fn count() -> u32 {
    OPEN.load(Ordering::Relaxed)
}

/// Held for the life of one stream.
#[derive(Debug)]
pub struct StreamGuard(());

/// Register one open stream until the guard drops.
pub fn guard() -> StreamGuard {
    OPEN.fetch_add(1, Ordering::Relaxed);
    StreamGuard(())
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_follows_the_guards() {
        let base = count();
        let a = guard();
        let b = guard();
        assert_eq!(count(), base + 2);
        drop(a);
        assert_eq!(count(), base + 1);
        drop(b);
        assert_eq!(count(), base);
    }
}
