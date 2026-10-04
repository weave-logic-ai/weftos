#!/usr/bin/env python3
"""Real weaver lifecycle inside an isolated Linux container; never a --version gate.

Default: print the plan without contacting an engine. --run explicitly executes.
Scope: real user daemon + logical project supervisor in ONE Linux container.
This does NOT accept the imported linux-container launcher or a child sandbox.
See docs/research/daemon-topology/completion-runs/container-lifecycle-plan.md.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import signal
import socket
import subprocess
import sys
import threading
import time
import tomllib


LABEL = "weftos.p4-lifecycle"
SCOPE = "real-user-and-logical-child-inside-linux-container"
WEAVER = "/usr/local/bin/weaver"
HEX64 = re.compile(r"[0-9a-f]{64}\Z")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def command(argv, timeout=30, **kwargs):
    return subprocess.run(argv, check=True, timeout=timeout, **kwargs)


def rpc(path, method, params=None, auth="admin", timeout=8):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(timeout)
        client.connect(str(path))
        request = {"id": secrets.token_hex(8), "proto": 1, "method": method,
                   "params": params or {}}
        if auth is not None:
            request["auth"] = auth
        client.sendall((json.dumps(request) + "\n").encode())
        with client.makefile("rb") as stream:
            line = stream.readline(4 * 1024 * 1024)
        require(line.endswith(b"\n"), "missing/oversized RPC reply")
        reply = json.loads(line)
        return reply


def result(path, method, params=None, **kwargs):
    reply = rpc(path, method, params, **kwargs)
    require(reply.get("ok") is True, f"{method}: {reply}")
    return reply["result"]


def wait_for(what, predicate, seconds=90):
    end = time.monotonic() + seconds
    last = None
    while time.monotonic() < end:
        try:
            value = predicate()
            if value:
                return value
        except (OSError, ValueError) as error:
            last = str(error)
        time.sleep(0.2)
    raise RuntimeError(f"timeout: {what}; last transport error: {last}")


def process_identity(pid):
    """Executable + start time guards against PID reuse. Linux guest only."""
    try:
        proc = Path(f"/proc/{pid}")
        fields = (proc / "stat").read_text().rsplit(") ", 1)[1].split()
        if fields[0] == "Z":
            return None
        return (os.readlink(proc / "exe"), fields[19])
    except (OSError, IndexError):
        return None


def worker(binary_sha):
    # No worker execution on a host: both the sentinel env and volume mount
    # are installed by the outer runner, and all writes are below /case.
    require(sys.platform == "linux" and os.environ.get("P4_LIFECYCLE_GUEST") == "1",
            "worker requires the isolated Linux fixture container")
    require(os.path.ismount("/case"), "/case must be the disposable engine volume")
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    require(Path(WEAVER).is_file(), f"{WEAVER} must be the weaver executable")
    require(hashlib.sha256(Path(WEAVER).read_bytes()).hexdigest() == binary_sha,
            f"{WEAVER} differs from the expected weaver artifact SHA-256")
    base = Path("/case")
    require(not (base / "started").exists(), "refusing to reuse fixture state")
    (base / "started").write_text("single use\n")
    home, run, project = base / "h", base / "r", base / "p"
    for path in (home, run, project, base / "scratch"):
        path.mkdir(mode=0o700)
    config = base / "config.json"
    config.write_text(json.dumps({"kernel": {
        "chain": {"enabled": True, "checkpoint_path": str(run / "chain.json")},
        "mesh": {"enabled": False, "service": "off"},
        "ipc_tcp": {"enabled": False},
        "governance": {"outside_project": "allow_all"},
        "llm": {"service_url": "http://127.0.0.1:1", "model": "fixture-no-model"}
    }}))
    # Child config discovery must also stay offline, independently of parent.
    (project / "weave.toml").write_text(
        '[kernel.mesh]\nenabled = false\nservice = "off"\n'
        '[kernel.ipc_tcp]\nenabled = false\n'
        '[kernel.llm]\nservice_url = "http://127.0.0.1:1"\nmodel = "fixture-no-model"\n')
    env = {"HOME": str(home), "WEFTOS_RUNTIME_DIR": str(run),
           "PATH": "/usr/local/bin:/usr/bin:/bin", "TMPDIR": str(base / "scratch"),
           "RUST_LOG": "warn"}
    owner = run / "kernel.sock"
    parents, children, logs = [], {}, []
    checks = []
    reaper_stop = threading.Event()

    def reap_orphans():
        # This Python worker is PID 1. After a parent restart its surviving
        # child is ours, not the new daemon's waitpid child. Reap only tracked
        # project PIDs, never steal a Popen parent's exit status.
        while not reaper_stop.wait(0.1):
            for pid in list(children):
                try:
                    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
                    if fields[19] == children[pid][1]:
                        os.waitpid(pid, os.WNOHANG)
                except (OSError, IndexError):
                    pass

    reaper = threading.Thread(target=reap_orphans, daemon=True)
    reaper.start()

    def checked(name):
        checks.append(name)
        print(json.dumps({"check": name, "status": "pass"}), flush=True)

    def boot_parent():
        log = (base / f"parent-{len(parents)}.log").open("wb")
        logs.append(log)
        parent = subprocess.Popen(
            [WEAVER, "kernel", "start", "--foreground", "--profile", "user",
             "--config", str(config)], cwd=base, env=env,
            stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
            start_new_session=True)
        parents.append(parent)

        def ready():
            require(parent.poll() is None, f"parent exited: {parent.returncode}")
            if not owner.exists():
                return False
            h = result(owner, "kernel.handshake")
            return h if h.get("pid") == parent.pid else False

        h = wait_for("user daemon ready", ready)
        require(h.get("profile") == "user" and "user" in h.get("roles", []),
                f"image does not run a real user daemon: {h}")
        require(h.get("runtime_dir") == str(run), "parent runtime escaped fixture")
        require(h.get("mesh", {}).get("mode") == "off", "parent mesh is enabled")
        return parent, h

    def events():
        return result(owner, "chain.tail", {"count": 0})

    def certificate(project_id):
        view = result(owner, "project.cert.show", {"id": project_id})
        require(view.get("certified") is True, "child was not certified")
        cert = view["cert"]
        body = {k: v for k, v in cert.items() if k != "sig"}
        public = bytes.fromhex(cert["user_pubkey"])
        Ed25519PublicKey.from_public_bytes(public).verify(
            bytes.fromhex(cert["sig"]), b"weftos-project-cert-v1\n" +
            json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode())
        require(hashlib.sha256(public).hexdigest()[:32] == parent_handshake["user_key_id"],
                "certificate is not issued by this fixture parent")
        require(cert["project_id"] == project_id, "wrong certified project")
        require(cert["project_key_id"] == hashlib.sha256(
            bytes.fromhex(cert["project_pubkey"])).hexdigest()[:32], "wrong project key ID")
        return cert

    def prove(pid, cert):
        nonce = secrets.token_hex(32)
        h = result(child_socket, "kernel.handshake", {"challenge": nonce}, auth=None)
        require(h.get("pid") == pid and h.get("project_id") == project_id,
                "child handshake identity mismatch")
        require(h.get("node_id") == cert["project_key_id"], "child node is not certified key")
        require(h.get("runtime_dir") == str(child_run), "child runtime escaped fixture")
        require(h.get("mesh", {}).get("mode") == "off", "child mesh is enabled")
        identity = process_identity(pid)
        require(identity and identity[0] == WEAVER, "child is not the pinned real weaver")
        children[pid] = identity
        public = Ed25519PublicKey.from_public_bytes(bytes.fromhex(cert["project_pubkey"]))
        proof = bytes.fromhex(h["project_proof"])
        message = f"weftos-project-handshake-v1\n{project_id}\n{nonce}".encode()
        public.verify(proof, message)
        # A verifier that accidentally ignores the message must not pass.
        from cryptography.exceptions import InvalidSignature
        try:
            public.verify(proof, message + b"changed")
        except InvalidSignature:
            pass
        else:
            raise RuntimeError("proof verified under a changed challenge")
        return h

    def live_session(pid, cert, bad_signature=False):
        # Probe the real registry without opening/replacing a session. A fresh
        # invented spawn nonce can NEVER authorize this request. A live session
        # returns second_session before consuming a nonce; an expired adoption
        # tombstone returns a spawn refusal instead. Sign the binding so the
        # probe actually reaches that decision, not an earlier shape check.
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        key = Ed25519PrivateKey.from_private_bytes((project / ".weftos/project.key").read_bytes())
        nonce, client_nonce = secrets.token_hex(16), secrets.token_hex(16)
        binding = f"weftos-mesh-local-bind-v1\n{project_id}\n{nonce}\n{client_nonce}\n{child_socket}\n{pid}"
        sig = bytearray(key.sign(binding.encode()))
        if bad_signature:
            sig[0] ^= 1
        reply = rpc(owner, "mesh.register", {
            "role": "project", "project_id": project_id,
            "project_pubkey": cert["project_pubkey"], "pid": pid,
            "socket": str(child_socket), "version": "fixture-probe",
            "client_nonce": client_nonce, "bind_sig": sig.hex(),
            "spawn_nonce": secrets.token_hex(32),
            "nonce_reply": {"nonce": nonce, "sig": "00" * 64}})
        require(reply.get("ok") is False, "probe unexpectedly registered a second session")
        return reply.get("error_kind") == ("pop_failed" if bad_signature else "second_session")

    try:
        parent, parent_handshake = boot_parent()
        project_id = result(owner, "project.register",
                            {"root": str(project), "name": "container-lifecycle"})["project"]["id"]
        require(re.fullmatch(r"[0-9A-HJKMNP-TV-Z]{26}", project_id), "invalid project ID")
        child_run = run / project_id
        child_socket = child_run / "kernel.sock"
        # Deliberately logical: no engine client/API exists inside this container.
        # Set serve options without a TOML writer dependency or production edits.
        manifest = home / ".weftos/projects" / f"{project_id}.toml"
        text = manifest.read_text()
        parsed = tomllib.loads(text)
        require(parsed["id"] == project_id, "manifest identity mismatch")
        require(set(parsed.get("serve", {})) <= {"via", "sandbox"}, "unexpected fresh serve options")
        serve = ('[serve]\nvia = "child-kernel"\nsandbox = "logical"\n'
                 'idle_stop_secs = 1800\nrestart_max = 3\nrestart_window_secs = 600\n\n')
        if "serve" in parsed:
            text, count = re.subn(r"(?ms)^\[serve\][ \t]*\n.*?(?=^\[|\Z)", lambda _: serve, text)
            require(count == 1, "cannot locate fresh serve table")
        else:
            text += "\n" + serve
        require(tomllib.loads(text)["serve"]["sandbox"] == "logical", "manifest rewrite failed")
        replacement = manifest.with_suffix(".fixture-write")
        replacement.write_text(text)
        replacement.chmod(0o600)
        replacement.replace(manifest)
        started = result(owner, "project.start", {"id": project_id}, timeout=120)
        pid1 = started["pid"]
        cert = certificate(project_id)
        prove(pid1, cert)
        require(live_session(pid1, cert), "initial signed registration is not live")
        require(live_session(pid1, cert, bad_signature=True), "invalid binding signature accepted")
        require(not (child_run / "spawn.json").exists(), "spawn credential was not consumed")
        require(any(e["kind"] == "project.register" for e in events()), "missing registration receipt")
        checked("real-child-signed-registration-and-fresh-proof")

        # Crash the verified child; automatic supervisor restart must change PID
        # while preserving its certificate and project identity.
        require(process_identity(pid1) == children[pid1], "child PID changed before crash")
        os.kill(pid1, signal.SIGKILL)

        def restarted():
            s = result(owner, "project.status", {"id": project_id})
            return s if s.get("state") == "running" and s.get("pid") != pid1 else False

        pid2 = wait_for("automatic child restart", restarted, 120)["pid"]
        require(certificate(project_id) == cert, "restart replaced certificate")
        prove(pid2, cert)
        checked("automatic-crash-restart-preserves-identity")

        # kernel.shutdown stops this daemon; CLI kernel stop would cascade by
        # default. Assert survival, then require a real adoption chain receipt.
        result(owner, "kernel.shutdown")
        require(parent.wait(timeout=30) == 0, "parent did not shut down cleanly")
        require(process_identity(pid2) == children[pid2], "child did not survive parent exit")
        parent, new_handshake = boot_parent()
        require(new_handshake["user_key_id"] == parent_handshake["user_key_id"], "parent key changed")
        wait_for("adoption receipt", lambda: any(
            e["kind"] == "project.kernel.adopted" and f"pid={pid2}" in e.get("detail", "")
            for e in events()))
        adopted = result(owner, "project.ensure_running", {"id": project_id}, timeout=120)
        require(adopted["pid"] == pid2 and adopted["started"] is False, "adoption respawned child")
        wait_for("adopted child signed re-registration", lambda: live_session(pid2, cert))
        require(certificate(project_id) == cert, "adoption replaced certificate")
        prove(pid2, cert)
        checked("parent-restart-adopts-and-re-registers-same-child")

        # Distinct persistence + real filesystem/network isolation of the OUTER
        # container. This is not a boundary between the same-uid logical pair.
        require((run / "chain.key").read_bytes() != (project / ".weftos/project.key").read_bytes(),
                "parent and child share signing seed")
        require((project / ".weftos/chain").is_dir(), "child chain directory missing")
        require(not Path("/var/run/docker.sock").exists() and not Path("/run/podman/podman.sock").exists(),
                "engine socket exposed")
        require(not Path("/var/run/weftos/mesh.sock").exists(), "outer mesh socket exposed")
        require(set(os.listdir("/sys/class/net")) == {"lo"}, "external network interface exposed")
        try:
            Path("/p4-write-probe").write_text("must fail")
        except OSError as error:
            import errno
            require(error.errno == errno.EROFS, f"unexpected root write error: {error}")
        else:
            raise RuntimeError("container root filesystem is writable")
        checked("distinct-chain-key-and-container-envelope-isolation")

        result(owner, "project.revoke", {"id": project_id, "reason": "disposable lifecycle fixture"})
        wait_for("revoked child exits", lambda: process_identity(pid2) != children[pid2])
        require((child_run / "revoked").is_file(), "revoked marker absent")
        for method in ("project.start", "project.restart", "project.ensure_running"):
            reply = rpc(owner, method, {"id": project_id}, timeout=120)
            require(reply.get("ok") is False and reply.get("error_kind") == "project_revoked",
                    f"revocation bypass: {method}: {reply}")
        wait_for("child socket cleanup", lambda: not child_socket.exists())
        checked("revocation-stops-child-and-blocks-all-start-paths")
        result(owner, "kernel.shutdown")
        require(parent.wait(timeout=30) == 0, "final parent exit failed")
        wait_for("parent socket cleanup", lambda: not owner.exists() and
                 not (run / "child-ipc/child.sock").exists())
        checked("daemon-and-child-socket-cleanup")
        receipt = {"status": "pass", "scope": SCOPE, "weaver_sha256": binary_sha,
                   "project_id": project_id, "child_pids": [pid1, pid2], "checks": checks,
                   "container_driver_accepted": False, "d10_accepted": False}
        print("P4_RECEIPT=" + json.dumps(receipt), flush=True)
    except BaseException:
        # Disposable fixture logs only; no keys, spawn files or tokens exported.
        for path in sorted(base.glob("parent-*.log")) + sorted(run.glob("*/kernel.log")):
            print(f"--- {path}: last 40 lines ---", file=sys.stderr)
            print("\n".join(path.read_text(errors="replace").splitlines()[-40:]), file=sys.stderr)
        raise
    finally:
        # Only signal tracked processes with matching identities. Outer engine
        # cleanup also destroys the private PID namespace on failure/timeout.
        for pid, identity in children.items():
            if process_identity(pid) == identity:
                os.kill(pid, signal.SIGKILL)
        for parent in parents:
            if parent.poll() is None:
                parent.kill()
            parent.wait(timeout=10)
        reaper_stop.set()
        reaper.join(timeout=2)
        for log in logs:
            log.close()


def host(args):
    require(re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", args.image), "image must be repository@sha256:digest")
    require(HEX64.fullmatch(args.weaver_sha256), "expected weaver SHA-256 must be 64 lowercase hex")
    if not args.run:
        print(json.dumps({"mode": "plan-only", "scope": SCOPE, "image": args.image,
                          "weaver_sha256": args.weaver_sha256,
                          "engine_operations": False, "container_driver_accepted": False}, indent=2))
        return
    require(args.report_dir is not None, "--run requires a new --report-dir")
    reports = args.report_dir.resolve()
    reports.mkdir(parents=True, exist_ok=False)
    run_id = secrets.token_hex(12)
    volume = "p4-lifecycle-" + run_id
    cid = None
    volume_created = False
    create_attempted = False
    cleanup_errors = []

    def engine(*argv, timeout=30):
        return command([args.engine, *argv], timeout=timeout, capture_output=True, text=True).stdout.strip()

    def owned_container():
        record = json.loads(engine("inspect", cid))[0]
        require(record["Id"] == cid and record["Config"]["Labels"].get(LABEL) == run_id,
                "container ownership changed; refusing destructive operation")
        return record

    try:
        # Inspect only an already-present immutable image; never pull/build.
        image = json.loads(engine("image", "inspect", args.image))[0]
        require(image.get("Os") == "linux", "image is not Linux")
        require(not image.get("Config", {}).get("Volumes"), "image declares implicit volumes")
        require(not image.get("Config", {}).get("OnBuild"), "image has unexpected build triggers")
        require(not image.get("Config", {}).get("Env") or all(
            not item.startswith(("DOCKER_HOST=", "CONTAINER_HOST=", "LD_PRELOAD=", "PYTHONPATH="))
            for item in image["Config"]["Env"]), "image carries engine/runtime injection env")
        # Treat even a timed-out create as possibly having created the volume.
        volume_created = True
        engine("volume", "create", "--label", f"{LABEL}={run_id}", volume)
        argv = ["create", "--pull=never", "--interactive", "--name", volume,
                "--label", f"{LABEL}={run_id}",
                "--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--no-healthcheck",
                "--network=none", "--pids-limit=256", "--memory=4g", "--cpus=2",
                "--user=0:0", "--tmpfs=/tmp:rw,nosuid,nodev,size=64m",
                "--mount", f"type=volume,src={volume},dst=/case",
                "--env=P4_LIFECYCLE_GUEST=1", "--env=PYTHONDONTWRITEBYTECODE=1",
                "--env=HOME=/case", "--workdir=/case", "--entrypoint=python3", args.image,
                "-I", "-B", "-c", "import sys; exec(compile(sys.stdin.read(), '<p4-harness>', 'exec'))",
                "--worker", "--weaver-sha256", args.weaver_sha256]
        create_attempted = True
        cid = engine(*argv)
        require(HEX64.fullmatch(cid), "engine did not return immutable container ID")
        record = owned_container()
        hc = record["HostConfig"]
        require(hc["ReadonlyRootfs"] and not hc["Privileged"] and hc["NetworkMode"] == "none",
                "engine weakened isolation")
        require(any(x.upper() == "ALL" for x in hc.get("CapDrop", [])) and not hc.get("CapAdd"),
                "engine did not drop capabilities")
        require(any(x.split(":")[0] == "no-new-privileges" for x in hc.get("SecurityOpt", [])),
                "no-new-privileges missing")
        # Engines may report the explicit tmpfs in Mounts or only HostConfig.
        mounts = record["Mounts"]
        require(any(m.get("Name") == volume and m["Destination"] == "/case" for m in mounts),
                "fixture volume missing")
        require(all((m.get("Type") == "volume" and m.get("Name") == volume and
                     m["Destination"] == "/case") or
                    (m.get("Type") == "tmpfs" and m["Destination"] == "/tmp") for m in mounts),
                "unexpected mount (host state must never be exposed)")
        (reports / "container-inspect.json").write_text(json.dumps(record, indent=2))
        with (reports / "lifecycle.log").open("wb") as output:
            command([args.engine, "start", "--attach", "--interactive", cid], timeout=900,
                    input=Path(__file__).read_bytes(), stdout=output, stderr=subprocess.STDOUT)
        stopped = owned_container()
        require(not stopped["State"]["Running"] and stopped["State"]["ExitCode"] == 0,
                "fixture container did not exit successfully")
        lines = (reports / "lifecycle.log").read_text().splitlines()
        receipts = [json.loads(line[len("P4_RECEIPT="):]) for line in lines if line.startswith("P4_RECEIPT=")]
        require(len(receipts) == 1 and receipts[0]["status"] == "pass", "missing lifecycle acceptance receipt")
        (reports / "receipt.json").write_text(json.dumps(
            {**receipts[0], "image": args.image, "container_id": cid}, indent=2))
    finally:
        if create_attempted and not (cid and HEX64.fullmatch(cid)):
            try:
                # A timed-out create can still have succeeded. Resolve only
                # this invocation's label, then use the immutable ID for rm.
                candidates = engine("ps", "--all", "--no-trunc", "--quiet",
                                    "--filter", f"label={LABEL}={run_id}").splitlines()
                require(len(candidates) <= 1, "ambiguous fixture container ownership")
                cid = candidates[0] if candidates else None
            except Exception as error:
                cleanup_errors.append(str(error))
        if cid and HEX64.fullmatch(cid):
            try:
                owned_container()
                engine("rm", "--force", cid)
                require(cid not in engine("ps", "--all", "--no-trunc", "--quiet").splitlines(),
                        "fixture container survived cleanup")
            except Exception as error:
                cleanup_errors.append(str(error))
        if volume_created:
            try:
                v = json.loads(engine("volume", "inspect", volume))[0]
                require(v.get("Labels", {}).get(LABEL) == run_id, "volume ownership mismatch")
                engine("volume", "rm", volume)
                require(volume not in engine("volume", "ls", "--quiet").splitlines(),
                        "fixture volume survived cleanup")
            except Exception as error:
                cleanup_errors.append(str(error))
        (reports / "cleanup.json").write_text(json.dumps(
            {"status": "fail" if cleanup_errors else "pass", "errors": cleanup_errors}, indent=2))
        require(not cleanup_errors, f"cleanup incomplete: {cleanup_errors}")
    print(f"PASS {SCOPE}; receipt and cleanup evidence: {reports}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image")
    parser.add_argument("--weaver-sha256", required=True)
    parser.add_argument("--engine", choices=("docker", "podman"), default="docker")
    parser.add_argument("--report-dir", type=Path)
    parser.add_argument("--run", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        worker(args.weaver_sha256)
    else:
        require(args.image is not None, "--image is required")
        host(args)


if __name__ == "__main__":
    main()
