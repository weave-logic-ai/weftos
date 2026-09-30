"""Pure planning helpers for the Pi 5 test lane (scripts/pi/pi_lane.py).

Everything here is side-effect free so it can be unit-tested
(scripts/pi/test_pi_plan.py): input validation at the CLI boundary, the
builder container command, cargo artifact parsing, the isolated remote
environment, and libtest result parsing.
"""
import json
import os
import re
import shlex

HOST_RE = re.compile(r"^(?:[A-Za-z0-9._-]+@)?[A-Za-z0-9._:-]+$")
CRATE_RE = re.compile(r"^[a-z0-9][a-z0-9_-]*$")
FILTER_RE = re.compile(r"^[A-Za-z0-9_:.-]+$")
PART_RE = re.compile(r"^[A-Za-z0-9._-]+$")
GLIBC_RE = re.compile(r"(\d+)\.(\d+)\s*$")
RESULT_RE = re.compile(
    r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored")
# Tracked files the tests may read at runtime (CARGO_MANIFEST_DIR-relative
# fixtures, ../../config, ../../assets). Synced to the Pi next to the binaries.
# scripts/pi/csi_feed.py: the placement stage runs it on the Pi.
SYNC_ROOTS = ("Cargo.toml", "Cargo.lock", "crates", "config", "assets", "scripts/pi/csi_feed.py")
RUSTUP_VOLUME = "weftos-pi-rustup"
PATH_ENV = "/usr/local/bin:/usr/bin:/bin"


def valid_host(host):
    """[user@]host with no option-looking or shell-significant characters."""
    return bool(host) and not host.startswith("-") and bool(HOST_RE.match(host))


def valid_crate(name):
    return bool(CRATE_RE.match(name or ""))


def valid_filter(f):
    return bool(FILTER_RE.match(f or ""))


def valid_scratch(rd):
    """Scratch dir relative to the Pi login dir: no `..`, no absolute path,
    no empty or dot-only parts, so `rm -rf` on it stays inside $HOME."""
    if not rd or rd.startswith("/"):
        return False
    return all(PART_RE.match(p) and p.strip(".") for p in rd.split("/"))


def glibc_version(text):
    """(major, minor) from `ldd --version` first line, or None."""
    m = GLIBC_RE.search((text or "").strip().splitlines()[0] if text else "")
    return (int(m.group(1)), int(m.group(2))) if m else None


def glibc_compatible(builder, target):
    """Binaries link against the builder's glibc, so it must not be newer."""
    return builder is not None and target is not None and builder <= target


def toolchain_channel(toml_text):
    m = re.search(r'^\s*channel\s*=\s*"([^"]+)"', toml_text, re.M)
    return m.group(1) if m else None


def builder_command(image, root, src_mount, target_dir, registry_dir, cargo_args):
    """`docker run` for an arm64 builder. The workspace is mounted at the same
    absolute path it will have on the Pi, so compile-time CARGO_MANIFEST_DIR
    paths (env!(...)) resolve there."""
    return ["docker", "run", "--rm", "--platform", "linux/arm64",
            "-v", "%s:%s:ro" % (root, src_mount),
            "-v", "%s:/target" % target_dir,
            "-v", "%s:/usr/local/cargo/registry" % registry_dir,
            # Named volume: seeded from the image, then keeps the toolchain
            # rustup installs for rust-toolchain.toml across runs.
            "-v", "%s:/usr/local/rustup" % RUSTUP_VOLUME,
            "-w", src_mount, "-e", "CARGO_TARGET_DIR=/target", "-e", "CARGO_TERM_COLOR=never",
            image] + list(cargo_args)


def test_cargo_args(crates):
    args = ["cargo", "test", "--locked", "--no-run",
            "--message-format=json-render-diagnostics"]
    for c in crates:
        args += ["-p", c]
    return args


def launcher_cargo_args():
    """cog_adapter_run, the conformance --launcher (see cmd_cogs_launcher)."""
    return ["cargo", "build", "--locked", "-p", "clawft-kernel", "--no-default-features",
            "--features", "workload-runtime", "--example", "cog_adapter_run"]


def package_name(pkg_id):
    """Package name from a cargo package id: `path+file:///x/foo#0.1.0` or
    `path+file:///x/dir#foo@0.1.0` (the latter when name != dir)."""
    base, _, frag = pkg_id.partition("#")
    if "@" in frag:
        return frag.split("@", 1)[0]
    return base.rstrip("/").rsplit("/", 1)[-1]


def parse_test_artifacts(lines, target_dir):
    """Test executables from cargo JSON messages.

    Returns dicts {crate, kind, name, container_path, local_path, manifest_dir}
    where kind is lib / bin / test and container paths under /target are
    mapped onto `target_dir` on this host."""
    out, seen = [], set()
    for line in lines:
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("reason") != "compiler-artifact" or not msg.get("executable"):
            continue
        if not (msg.get("profile") or {}).get("test"):
            continue
        exe = msg["executable"]
        if exe in seen or not exe.startswith("/target/"):
            continue
        seen.add(exe)
        tgt = msg.get("target") or {}
        kind = (tgt.get("kind") or ["?"])[0]
        crate = package_name(msg.get("package_id") or "")
        manifest = msg.get("manifest_path") or ""
        out.append({
            "crate": crate or tgt.get("name", "?"),
            "kind": "lib" if kind in ("lib", "rlib", "proc-macro") else kind,
            "name": tgt.get("name", "?"),
            "container_path": exe,
            "local_path": target_dir.rstrip("/") + exe[len("/target"):],
            "manifest_dir": manifest.rsplit("/", 1)[0] if "/" in manifest else "",
        })
    return out


def isolated_env(scratch_abs, extra=None):
    """Environment for every process the lane starts on the Pi: a fresh HOME,
    runtime dir and XDG dirs under the scratch dir, so nothing can reach the
    Pi's ~/.clawft or talk to its weaver.service."""
    env = {
        "PATH": PATH_ENV,
        "HOME": scratch_abs + "/home",
        "WEFTOS_RUNTIME_DIR": scratch_abs + "/runtime",
        "TMPDIR": scratch_abs + "/tmp",
        "XDG_CONFIG_HOME": scratch_abs + "/home/.config",
        "XDG_DATA_HOME": scratch_abs + "/home/.local/share",
        "XDG_CACHE_HOME": scratch_abs + "/home/.cache",
        "XDG_STATE_HOME": scratch_abs + "/home/.local/state",
        "RUST_BACKTRACE": "1",
        # No cargo on the Pi: insta cannot run `cargo metadata`, so give it
        # the synced workspace root, and never let it write .snap.new files.
        "INSTA_WORKSPACE_ROOT": scratch_abs + "/src",
        "INSTA_UPDATE": "no",
    }
    env.update(extra or {})
    return env


def remote_test_command(scratch_abs, binary, manifest_dir, test_args, extra_env=None,
                        timeout=None):
    """Shell line run over ssh: cd to the crate dir (as cargo test would) and
    exec the test binary under `env -i` with only the isolated environment.
    With `timeout` the Pi's coreutils `timeout` bounds it, so a hung test is
    killed on the Pi itself, not only the local ssh client."""
    env = isolated_env(scratch_abs, extra_env)
    env["CARGO_MANIFEST_DIR"] = manifest_dir
    assigns = ["%s=%s" % (k, v) for k, v in sorted(env.items())]
    cap = "timeout -k 10 %d " % int(timeout) if timeout else ""
    return "cd %s && exec %senv -i %s %s" % (
        shlex.quote(manifest_dir), cap, shlex.join(assigns),
        shlex.join([binary] + list(test_args)))


# Operator files on the Pi the lane must never change (relative to $HOME).
# sessions/ and kernel.log are left out: the Pi's own weaver writes them.
OPERATOR_FILES = (".clawft/chain.rvf", ".clawft/chain.key", ".clawft/chain.tree.json",
                  ".clawft/config.json", ".clawft/node.key")
STATE_END = "weftos-pi-state-end"


def pi_state_command():
    """One ssh line: `<file> <size> <mtime>` (or `<file> absent`) per operator
    file, the weaver.service state with its MainPID and ActiveEnterTimestamp
    (so a restart during the run is a change, not `active -> active`), then an
    end marker so a truncated or failed probe is detectable."""
    show = "systemctl show -p %s --value weaver.service 2>/dev/null"
    parts = ["for f in %s; do stat -c '%%n %%s %%Y' \"$f\" 2>/dev/null || echo \"$f absent\"; done"
             % " ".join(OPERATOR_FILES),
             "echo \"weaver $(systemctl is-active weaver.service 2>/dev/null || true)"
             " pid=$(%s) since=$(%s)\"" % (show % "MainPID", show % "ActiveEnterTimestampMonotonic"),
             "echo %s" % STATE_END]
    return "cd && " + "; ".join(parts)


def parse_pi_state(rc, out):
    """{file: 'size mtime'|'absent', 'weaver': state} or None when the probe
    failed or is incomplete (never guessed)."""
    lines = (out or "").strip().splitlines()
    if rc != 0 or not lines or lines[-1].strip() != STATE_END:
        return None
    state = {}
    for line in lines[:-1]:
        name, _, rest = line.strip().partition(" ")
        state[name] = rest.strip() or "?"
    want = set(OPERATOR_FILES) | {"weaver"}
    return state if want <= set(state) else None


def operator_guard(pi_before, pi_after, mac_before, mac_after):
    """Guard verdict. An unknown Pi state (failed probe) is NOT unchanged."""
    known = pi_before is not None and pi_after is not None
    changed = sorted(k for k in OPERATOR_FILES if known and pi_before[k] != pi_after[k])
    g = {"pi_probe_ok": known,
         "pi_operator_files_unchanged": known and not changed,
         "pi_changed": changed,
         "pi_weaver_service": "%s -> %s" % ((pi_before or {}).get("weaver", "?"),
                                            (pi_after or {}).get("weaver", "?")),
         "pi_weaver_unchanged": known and pi_before["weaver"] == pi_after["weaver"],
         "mac_chain_unchanged": mac_before == mac_after}
    g["ok"] = (g["pi_operator_files_unchanged"] and g["pi_weaver_unchanged"]
               and g["mac_chain_unchanged"])
    return g


def filter_matched(results):
    """With --filter, the test stage must select at least one test in total
    (passed, failed or ignored); otherwise a typo'd filter would report green
    without testing anything. Failures are judged by stage_ok, not here."""
    return sum(r["passed"] + r["failed"] + r["ignored"]
               for r in results if r["stage"] == "test") > 0


def remove_scratch_command(scratch_rel):
    """Remove the (validated, relative) Pi scratch dir, falling back to sudo -n
    for the root-owned files a conformance --sudo stage leaves, and fail if it
    is still there. Used before staging (a killed run's leftovers) and after."""
    if not valid_scratch(scratch_rel):
        raise ValueError("invalid scratch dir %r" % scratch_rel)
    rd = scratch_rel
    return "cd && { rm -rf %s 2>/dev/null || sudo -n rm -rf %s; } && test ! -e %s" % (rd, rd, rd)


def present_files(root, listing):
    """Split a NUL-separated `git ls-files` listing into (bytes of the paths
    that exist in the worktree, [missing paths]). A tracked file deleted but
    not committed is left out instead of failing rsync with rc 23."""
    keep, missing = [], []
    for p in (listing or b"").split(b"\0"):
        if p:
            (keep if os.path.lexists(os.path.join(root.encode(), p)) else missing).append(p)
    return b"".join(p + b"\0" for p in keep), [m.decode(errors="replace") for m in missing]


def sync_files_command(root):
    """git command listing the files synced to the Pi: tracked files only
    under SYNC_ROOTS, so untracked local files (notes, credentials) never leave
    the Mac."""
    return ["git", "-C", root, "ls-files", "-z", "--cached", "--"] + list(SYNC_ROOTS)


def parse_results(text):
    """Sum libtest `test result:` lines -> {passed, failed, ignored, suites}."""
    tot = {"passed": 0, "failed": 0, "ignored": 0, "suites": 0}
    for m in RESULT_RE.finditer(text or ""):
        tot["suites"] += 1
        tot["passed"] += int(m.group(2))
        tot["failed"] += int(m.group(3))
        tot["ignored"] += int(m.group(4))
    return tot


def stage_ok(rc, totals, require_ran=False):
    """A stage passes when the binary exited 0, libtest reported, nothing
    failed and (for a filtered live run) at least one test actually ran."""
    if rc != 0 or totals["suites"] == 0 or totals["failed"]:
        return False
    return totals["passed"] > 0 if require_ran else True


# ── two-node placement stage (card mesh-placement-12) ────────────────────
# The Pi runs an isolated `weaver` daemon (its own HOME and runtime dir
# under the scratch dir) serving its workload-host on a non-default port;
# the system weaver keeps its mesh port and ~/.clawft. The system mesh port is
# 9489 (ADR-103 D1); a Pi weaver deployed before that change still listens on
# 9470 until it is redeployed, so the lane guards both.
SYSTEM_MESH_PORT = 9489
LEGACY_SYSTEM_MESH_PORT = 9470
SYSTEM_MESH_PORTS = (SYSTEM_MESH_PORT, LEGACY_SYSTEM_MESH_PORT)
# Override with WEFTOS_PI_PLACEMENT_PORT; it must not be a system mesh port.
PLACEMENT_PORT = int(os.environ.get("WEFTOS_PI_PLACEMENT_PORT", "9471"))
assert PLACEMENT_PORT not in SYSTEM_MESH_PORTS, "placement port collides with the system mesh port"
PLACEMENT_FEED_PORT = 15006
PUBKEY_RE = re.compile(r"^[0-9a-f]{64}$")
STATUS_MARK = "==STATUS=="
# Evidence must not carry addresses or local paths (public repo).
LEAK_RE = re.compile(r"(?:\b(?!0\.0\.0\.0\b)(?!127\.0\.0\.1\b)\d{1,3}(?:\.\d{1,3}){3}\b|/Users/|/home/)")


def placement_cargo_args():
    """workload_node, the Mac-side policy-file tool (`daemon-files`, host target)."""
    return ["cargo", "build", "--locked", "-p", "clawft-kernel", "--no-default-features",
            "--features", "workload-runtime,mesh", "--example", "workload_node"]


def weaver_cargo_args():
    """The real `weaver` daemon for the Pi (default features incl. placement)."""
    return ["cargo", "build", "--locked", "-p", "clawft-weave", "--bin", "weaver"]


def ssh_hostname(host):
    """The address part of a validated [user@]host (the Pi's mesh address)."""
    if not valid_host(host):
        raise ValueError("invalid host")
    return host.rsplit("@", 1)[-1]


def _isolated(scratch_abs, argv):
    env = isolated_env(scratch_abs)
    return "env -i %s %s" % (shlex.join("%s=%s" % kv for kv in sorted(env.items())),
                             shlex.join(argv))


def weaver_start_command(scratch_abs, binary):
    """Start `weaver kernel start --foreground` detached under `env -i` (its
    runtime dir, chain and sockets under the scratch dir) and print its PID.
    Its policy files (workload-host.json etc.) are already in the runtime dir."""
    run = scratch_abs + "/runtime"
    # `cd || exit;` rather than `cd &&`: `&` must background only the daemon,
    # or the whole list (holding ssh's stdout) is backgrounded and ssh hangs.
    return "cd %s || exit 1; nohup %s > %s 2>&1 < /dev/null & echo $!" % (
        shlex.quote(run), _isolated(scratch_abs, [binary, "kernel", "start", "--foreground"]),
        shlex.quote(run + "/placement.log"))


def feed_start_command(scratch_abs, port=PLACEMENT_FEED_PORT, secs=900):
    """Start the synthetic sensor feed (python3, from the synced sources)."""
    run = scratch_abs + "/runtime"
    feed = scratch_abs + "/src/scripts/pi/csi_feed.py"
    return "cd %s || exit 1; nohup %s > %s 2>&1 < /dev/null & echo $!" % (
        shlex.quote(run), _isolated(scratch_abs, ["python3", feed, "--port", str(int(port)),
                                                   "--secs", str(int(secs))]),
        shlex.quote(run + "/feed.log"))


def weaver_ready_command(scratch_abs, binary, port=PLACEMENT_PORT, secs=120):
    """Wait until the daemon serves workload-host on `port` with its own
    signed facts (a pinned local target), then print its workload status."""
    status = _isolated(scratch_abs, [binary, "workload", "status", "--json"])
    return ("for i in $(seq 1 %d); do OUT=$(%s 2>/dev/null); "
            "echo \"$OUT\" | grep -q '\"served_on\": \"0.0.0.0:%d\"' "
            "&& echo \"$OUT\" | grep -q '\"tier\": \"pinned\"' && break; sleep 1; done; "
            "echo %s; echo \"$OUT\"" % (int(secs), status, int(port), STATUS_MARK))


def parse_ready(out):
    """(node id, served_on) from weaver_ready_command output, or (None, None)."""
    _, _, text = (out or "").partition(STATUS_MARK)
    try:
        st = json.loads(text)
    except ValueError:
        return None, None
    host = st.get("workload_host") or {}
    served = st.get("served_on")
    if not isinstance(host, dict) or not served:
        return None, None
    return host.get("node_id"), served


def weaver_stop_command(scratch_abs, binary, pid, feed_pid=None):
    """Export the daemon's chain (JSON source/kind rows), SIGTERM daemon and
    feed, wait, then print the daemon log and the chain."""
    run = scratch_abs + "/runtime"
    chain = run + "/pi-chain.json"
    pids = " ".join(str(int(p)) for p in (pid, feed_pid) if p)
    export = _isolated(scratch_abs, [binary, "chain", "export", "--format", "json"])
    return ("%s > %s 2>/dev/null; kill -TERM %s 2>/dev/null; for i in $(seq 1 20); do "
            "kill -0 %d 2>/dev/null || break; sleep 1; done; kill -KILL %s 2>/dev/null; "
            "cat %s; echo ==CHAIN==; cat %s 2>/dev/null" % (
                export, shlex.quote(chain), pids, int(pid), pids,
                shlex.quote(run + "/placement.log"), shlex.quote(chain)))


def placement_kill_all_command(scratch_rel):
    """Cleanup backstop: stop an isolated weaver or feed left running from the
    scratch dir (never the system weaver, which runs from /usr/local/bin)."""
    if not valid_scratch(scratch_rel):
        raise ValueError("invalid scratch dir %r" % scratch_rel)
    # `[w]...` so the pattern never matches this ssh shell's own command line.
    head = "[%s]%s" % (scratch_rel[0], scratch_rel[1:])
    return "pkill -TERM -f %s; pkill -TERM -f %s; true" % (
        shlex.quote(head + "/bin/weaver kernel start"),
        shlex.quote(head + "/src/scripts/pi/csi_feed.py"))


def split_node_output(text):
    """(log text, chain events list or None) from weaver_stop_command output."""
    log, _, chain = (text or "").partition("==CHAIN==")
    try:
        events = json.loads(chain) if chain.strip() else None
    except ValueError:
        events = None
    return log, events if isinstance(events, list) else None
