//! Build-stamp mismatch guard (installer DX).
//!
//! A stale installed `weft` talking to a freshly-built daemon (or the
//! reverse) silently diverges — e.g. the CLI is missing a subcommand the
//! daemon now speaks, or vice-versa. This guard catches that: after
//! connecting to the daemon, callers run [`warn_on_build_mismatch`], which
//! reads the daemon's build hash from `kernel.status` and compares it to
//! the hash baked into this binary (`BUILD_GIT_HASH`, set by `build.rs`).
//! It prefers `kernel.handshake` (build sha plus the accepted protocol
//! range) and falls back to `kernel.status` for older daemons.
//! On divergence it prints ONE warning line to stderr, once per process,
//! ending in the exact remedy, and is otherwise silent. It never fails a command — a transport error,
//! an `unknown` stamp on either side, or a matching hash all no-op.
//!
//! Wire contract: the daemon reports `kernel.status` → `build.sha` and this
//! side reads exactly that key path. Both are the git short hash from the
//! same `--short=8` + `-dirty` convention (see the two `build.rs` files),
//! so a same-tree build of `weft` and `weaver` produces identical hashes.

use std::sync::atomic::{AtomicBool, Ordering};

use clawft_rpc::handshake::is_dirty;
use clawft_rpc::{DaemonClient, Handshake, PROTO_VERSION, ProtoRange, Request, remedy};

/// The build hash baked into this `weft` binary (set by `build.rs`).
const WEFT_SHA: &str = env!("BUILD_GIT_HASH");

/// Warn at most once per process invocation.
static WARNED: AtomicBool = AtomicBool::new(false);

/// True when a hash is absent or the git-unavailable sentinel, so a
/// comparison would be meaningless.
fn indeterminate(sha: &str) -> bool {
    sha.is_empty() || sha == "unknown"
}

/// What the daemon reported about itself.
struct DaemonView {
    sha: String,
    /// Protocol range; `None` from a daemon that predates the handshake.
    proto: Option<ProtoRange>,
}

/// The one warning line for this client/daemon pair, if any.
///
/// A protocol the daemon would refuse wins over plain sha skew, since
/// every call will fail rather than merely diverge. Both name the exact
/// next command via [`remedy`].
fn skew_warning(weft_sha: &str, daemon: &DaemonView) -> Option<String> {
    if let Some(range) = daemon.proto
        && !range.accepts(PROTO_VERSION)
    {
        return Some(format!(
            "warning: weft speaks protocol {PROTO_VERSION} but the daemon accepts {}..={} — {}",
            range.min,
            range.current,
            remedy(&daemon.sha, false)
        ));
    }
    if indeterminate(weft_sha) || indeterminate(&daemon.sha) || daemon.sha == weft_sha {
        return None;
    }
    Some(format!(
        "warning: weft built at {weft_sha} but daemon at {} — {}",
        daemon.sha,
        remedy(&daemon.sha, is_dirty(weft_sha))
    ))
}

/// Ask the daemon who it is: `kernel.handshake`, falling back to the
/// build stamp in `kernel.status` for daemons that predate it.
async fn fetch_daemon_view(client: &mut DaemonClient) -> Option<DaemonView> {
    if let Ok(resp) = client.call(Request::new("kernel.handshake")).await
        && let Ok(v) = resp.into_result()
        && let Ok(h) = serde_json::from_value::<Handshake>(v)
    {
        return Some(DaemonView {
            sha: h.sha,
            proto: Some(h.proto),
        });
    }
    let value = client
        .call(Request::new("kernel.status"))
        .await
        .ok()?
        .into_result()
        .ok()?;
    let sha = value
        .get("build")
        .and_then(|b| b.get("sha"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_owned();
    Some(DaemonView { sha, proto: None })
}

/// Compare this binary's build hash and protocol against the connected
/// daemon's and print a single warning to stderr on mismatch.
///
/// Best-effort and non-fatal: any RPC/parse failure, an indeterminate
/// stamp on either side, or a matching pair returns silently. The warning
/// fires at most once per process invocation.
pub async fn warn_on_build_mismatch(client: &mut DaemonClient) {
    if WARNED.load(Ordering::Relaxed) {
        return;
    }
    let Some(daemon) = fetch_daemon_view(client).await else {
        return;
    };
    let Some(line) = skew_warning(WEFT_SHA, &daemon) else {
        return;
    };
    // Set-and-check so concurrent callers still emit exactly one line.
    if WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    eprintln!("{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indeterminate_covers_empty_and_unknown() {
        assert!(indeterminate(""));
        assert!(indeterminate("unknown"));
        assert!(!indeterminate("14145f16"));
        assert!(!indeterminate("14145f16-dirty"));
    }

    fn view(sha: &str, proto: Option<ProtoRange>) -> DaemonView {
        DaemonView {
            sha: sha.into(),
            proto,
        }
    }

    #[test]
    fn matching_or_indeterminate_is_silent() {
        assert!(skew_warning("aaaa", &view("aaaa", None)).is_none());
        assert!(skew_warning("aaaa", &view("unknown", None)).is_none());
        assert!(skew_warning("unknown", &view("bbbb", None)).is_none());
        let ok = Some(ProtoRange::supported());
        assert!(skew_warning("aaaa", &view("aaaa", ok)).is_none());
    }

    #[test]
    fn sha_skew_warns_with_remedy() {
        let w = skew_warning("aaaa", &view("bbbb", None)).unwrap();
        assert!(w.contains("aaaa") && w.contains("bbbb"), "{w}");
        assert!(w.contains("weaver update"), "{w}");
        let w = skew_warning("aaaa-dirty", &view("bbbb", None)).unwrap();
        assert!(w.contains("scripts/build.sh install"), "{w}");
    }

    #[test]
    fn proto_refusal_wins_over_sha_skew() {
        let newer = ProtoRange {
            current: PROTO_VERSION + 2,
            min: PROTO_VERSION + 1,
        };
        let w = skew_warning("aaaa", &view("aaaa", Some(newer))).unwrap();
        assert!(w.contains("protocol"), "{w}");
        assert!(w.contains("weaver kernel restart"), "{w}");
    }

    #[test]
    fn weft_sha_is_baked_nonempty() {
        // build.rs always sets BUILD_GIT_HASH (git hash or the `unknown`
        // fallback), so the compile-time env is never missing.
        assert!(!WEFT_SHA.is_empty());
    }
}
