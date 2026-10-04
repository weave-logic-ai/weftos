#!/usr/bin/env python3
"""Real LinuxContainer adapter acceptance on an offline disposable DinD engine.

Without --run: validate inputs and print the plan; no engine contact or writes.
With --run: requires an explicitly named disposable Docker context + engine ID.
The parent and dockerd run in the same privileged fixture container. No host
socket is mounted. Children are created ONLY by the production supervisor.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import time
import tomllib
import urllib.request

WEAVER = "/usr/local/bin/weaver"
LABEL = "weftos.p4-driver"
HEX = re.compile(r"[0-9a-f]{64}\Z")
PIN = re.compile(r"[^\s]+@sha256:[0-9a-f]{64}\Z")


def need(value, message):
    if not value:
        raise RuntimeError(message)


def sha(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def run(argv, timeout=30, **kw):
    return subprocess.run(argv, timeout=timeout, check=True, **kw)


def poll(name, action, timeout=120):
    end = time.monotonic() + timeout
    last = "not ready"
    while time.monotonic() < end:
        try:
            value = action()
            if value:
                return value
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            last = str(error)
        time.sleep(0.25)
    raise RuntimeError(f"{name}: timed out ({last})")


BOUNDARY_PROBE = r'''
import errno, hashlib, json, os, pathlib, socket
pid = os.environ['WEFTOS_PROJECT_ID']
assert set(os.listdir('/sys/class/net')) == {'lo'}
for p in ['/case', '/var/run/docker.sock', '/run/docker.sock', '/var/run/weftos/mesh.sock']:
    assert not os.path.exists(p), 'exposed parent path: ' + p
for p in ['/p4-write-denied', '/weftos/trust/p4-write-denied', '/weftos/parent/p4-write-denied']:
    try:
        fd = os.open(p, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except OSError as e:
        assert e.errno == errno.EROFS, (p, str(e))
    else:
        os.close(fd)
        raise AssertionError('writable protected mount: ' + p)
for p in ['/weftos/trust/user.pub', '/weftos/trust/parent-policy.json']:
    assert os.path.isfile(p)
    try:
        fd = os.open(p, os.O_WRONLY)  # never truncate actual trust material
    except OSError as e:
        assert e.errno == errno.EROFS, (p, str(e))
    else:
        os.close(fd)
        raise AssertionError('writable trust file: ' + p)
for root in ['/weftos/project', '/weftos/run/' + pid]:
    p = pathlib.Path(root) / 'p4-write-probe'
    p.write_text('fixture')
    assert p.read_text() == 'fixture'
    p.unlink()
assert set(os.listdir('/weftos/parent')) == {'child.sock'}
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
    s.settimeout(5)
    s.connect('/weftos/parent/child.sock')  # actual native Linux directory mount
print(json.dumps({'boundary': 'pass', 'weaver_sha256': hashlib.sha256(
    pathlib.Path('/usr/local/bin/weaver').read_bytes()).hexdigest()}))
'''


def guest(args, common_source):
    need(sys.platform == "linux" and os.getpid() == 1 and
         os.environ.get("P4_DRIVER_GUEST") == "1", "guest must be the dedicated fixture PID 1")
    need(os.path.ismount("/case"), "missing fresh fixture volume")
    need(not Path("/var/run/docker.sock").exists(), "unexpected preexisting engine socket")
    need(sha(WEAVER) == args.weaver_sha256, "parent weaver hash mismatch")
    # Reuse reviewed wire helpers without running the logical harness's main.
    common = {"__name__": "p4_common"}
    exec(compile(common_source, "<p4-common>", "exec"), common)
    rpc, result = common["rpc"], common["result"]
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey

    base = Path("/case")
    (base / "started").open("x").close()
    for name in ("h", "r", "p", "scratch", "engine", "client", "registry"):
        (base / name).mkdir(mode=0o700)
    home, root, runtime = base / "h", base / "p", base / "r"
    endpoint = "unix:///case/engine/docker.sock"
    image = "127.0.0.1:5000/p4/child@sha256:" + args.child_digest
    env = {"PATH": "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
           "HOME": str(home), "TMPDIR": "/case/scratch", "DOCKER_HOST": endpoint,
           "DOCKER_CONFIG": "/case/client", "WEFTOS_RUNTIME_DIR": str(runtime),
           "RUST_LOG": "warn"}
    processes, handles, evidence = [], [], {"checks": [], "containers": []}
    project_id = None
    parent = None
    pending = None
    cleanup_errors = []

    def spawn(label, argv, process_env=env):
        handle = (base / (label + ".log")).open("wb")
        handles.append(handle)
        proc = subprocess.Popen(argv, env=process_env, cwd=base, stdin=subprocess.DEVNULL,
                                stdout=handle, stderr=subprocess.STDOUT, start_new_session=True)
        processes.append(proc)
        return proc

    def docker(*argv, timeout=30, **kw):
        return run(["docker", "--host", endpoint, *argv], env=env, timeout=timeout,
                   capture_output=True, text=True, **kw).stdout.strip()

    def check(name, **facts):
        evidence["checks"].append({"name": name, **facts})
        print(json.dumps({"check": name, "status": "pass", **facts}), flush=True)

    def owned(cid):
        need(HEX.fullmatch(cid), "invalid immutable child ID")
        record = json.loads(docker("inspect", cid))[0]
        need(record["Id"] == cid and record["Config"]["Image"] == image, "child ID/image changed")
        labels = record["Config"].get("Labels", {})
        need(labels.get("weftos.project") == project_id and
             labels.get("weftos.supervisor") == parent_identity["user_key_id"], "foreign child labels")
        need(record["Config"]["Entrypoint"] == [WEAVER], "wrong executable entrypoint")
        need(record["Config"]["User"] == "0:0", "child UID/GID mismatch")
        need(record["Config"]["Cmd"] == ["kernel", "start", "--foreground", "--profile", "project",
                                         "--project", project_id], "not a real supervised project kernel")
        hc = record["HostConfig"]
        need(hc["ReadonlyRootfs"] and not hc["Privileged"] and hc["NetworkMode"] == "none",
             "child isolation weakened")
        need(not hc.get("CapAdd") and "ALL" in [x.upper() for x in hc.get("CapDrop", [])], "capabilities exposed")
        need(any(x.split(":")[0] == "no-new-privileges" for x in hc.get("SecurityOpt", [])), "NNP missing")
        need(hc["PidsLimit"] == 64, "child PID limit changed")
        need(set(record["NetworkSettings"].get("Networks", {})) <= {"none"}, "child network attached")
        expected = {(str(root), "/weftos/project", True),
                    (str(child_run / "guest"), f"/weftos/run/{project_id}", True),
                    (str(child_run), "/weftos/trust", False),
                    (str(runtime / "child-ipc"), "/weftos/parent", False)}
        mounts = record["Mounts"]
        need(len(mounts) == 4 and all(m["Type"] == "bind" for m in mounts) and
             {(m["Source"], m["Destination"], m["RW"]) for m in mounts} == expected, "child mount contract changed")
        return record

    def snapshot(cid, stage):
        record = owned(cid)
        evidence["containers"].append({"stage": stage, "inspect": record})
        return record

    def child_ids():
        return docker("ps", "-a", "--no-trunc", "-q", "--filter", f"label=weftos.project={project_id}").splitlines()

    def state():
        return json.loads((child_run / "state.json").read_text())

    def start_parent():
        proc = spawn(f"parent-{len(processes)}", [WEAVER, "kernel", "start", "--foreground",
                     "--profile", "user", "--config", "/case/config.json"])
        def ready():
            need(proc.poll() is None, f"parent exited {proc.returncode}")
            if not owner.exists():
                return False
            h = result(owner, "kernel.handshake")
            return h if h.get("pid") == proc.pid else False
        h = poll("parent startup", ready)
        need(h.get("profile") == "user" and h.get("mesh", {}).get("mode") == "off", "bad parent profile/mesh")
        need(h["runtime_dir"] == str(runtime), "parent runtime escaped")
        for ns in ("pid", "mnt"):
            need(os.readlink(f"/proc/{proc.pid}/ns/{ns}") == os.readlink(f"/proc/{dockerd.pid}/ns/{ns}"),
                 f"parent and dockerd do not share {ns} namespace")
        return proc, h

    def certify():
        view = result(owner, "project.cert.show", {"id": project_id})
        need(view.get("certified") is True, "project not certified")
        cert = view["cert"]
        pub = bytes.fromhex(cert["user_pubkey"])
        need(hashlib.sha256(pub).hexdigest()[:32] == parent_identity["user_key_id"], "foreign cert issuer")
        body = {k: v for k, v in cert.items() if k != "sig"}
        Ed25519PublicKey.from_public_bytes(pub).verify(bytes.fromhex(cert["sig"]),
            b"weftos-project-cert-v1\n" + json.dumps(body, sort_keys=True, separators=(",", ":")).encode())
        need(cert["project_id"] == project_id, "wrong cert project")
        need(cert["project_key_id"] == hashlib.sha256(bytes.fromhex(cert["project_pubkey"])).hexdigest()[:32],
             "wrong certified key ID")
        return cert

    def proof(cid, cert):
        record = owned(cid)
        need(record["State"]["Running"], "child not running")
        host_pid = record["State"]["Pid"]
        need(host_pid > 1 and os.readlink(f"/proc/{host_pid}/exe") == WEAVER, "host PID not visible as weaver")
        need(sha(f"/proc/{host_pid}/exe") == args.weaver_sha256, "child executable hash mismatch")
        for ns in ("pid", "mnt", "net"):
            need(os.readlink(f"/proc/{host_pid}/ns/{ns}") != os.readlink(f"/proc/self/ns/{ns}"),
                 f"child shares parent {ns} namespace")
        nonce = secrets.token_hex(32)
        h = result(child_socket, "kernel.handshake", {"challenge": nonce}, auth=None)
        need(h["project_id"] == project_id and h["node_id"] == cert["project_key_id"], "child identity mismatch")
        need(h["pid"] > 0 and h["runtime_dir"] == f"/weftos/run/{project_id}", "guest path/PID mismatch")
        need(h.get("mesh", {}).get("mode") == "off", "child joined outer mesh")
        Ed25519PublicKey.from_public_bytes(bytes.fromhex(cert["project_pubkey"])).verify(
            bytes.fromhex(h["project_proof"]), f"weftos-project-handshake-v1\n{project_id}\n{nonce}".encode())
        return h

    def session(cid, h, cert, bad=False):
        # An invented spawn nonce cannot open a session. A live session refuses
        # with second_session; the v2 signature must pass before that decision.
        nonce, client = secrets.token_hex(16), secrets.token_hex(16)
        guest_sock = f"/weftos/run/{project_id}/kernel.sock"
        binding = f"weftos-mesh-local-bind-v2\n{project_id}\n{nonce}\n{client}\n{guest_sock}\n{h['pid']}\ndocker\n{cid}\n{child_socket}"
        key = Ed25519PrivateKey.from_private_bytes((root / ".weftos/project.key").read_bytes())
        sig = bytearray(key.sign(binding.encode()))
        if bad:
            sig[0] ^= 1
        reply = rpc(owner, "mesh.register", {"role": "project", "project_id": project_id,
            "project_pubkey": cert["project_pubkey"], "pid": h["pid"], "socket": guest_sock,
            "container": {"engine": "docker", "container_id": cid, "host_socket": str(child_socket)},
            "version": "fixture-probe", "client_nonce": client, "bind_sig": sig.hex(),
            "spawn_nonce": secrets.token_hex(32), "nonce_reply": {"nonce": nonce, "sig": "00" * 64}})
        need(reply.get("ok") is False, "probe opened a session")
        return reply.get("error_kind") == ("pop_failed" if bad else "second_session")

    try:
        # Entire image distribution is local to this network-none fixture.
        registry_cfg = base / "registry.yml"
        registry_cfg.write_text('version: 0.1\nstorage:\n  filesystem:\n    rootdirectory: /case/registry\nhttp:\n  addr: 127.0.0.1:5000\n')
        registry = spawn("registry", ["registry", "serve", str(registry_cfg)])
        def registry_ready():
            need(registry.poll() is None, "fixture registry exited")
            with urllib.request.urlopen("http://127.0.0.1:5000/v2/", timeout=2) as response:
                return response.status == 200
        poll("loopback registry", registry_ready, 30)
        dockerd = spawn("dockerd", ["dockerd", "--host=" + endpoint, "--data-root=/case/engine/data",
            "--exec-root=/case/engine/exec", "--pidfile=/case/engine/pid", "--storage-driver=vfs",
            "--bridge=none", "--iptables=false", "--ip6tables=false", "--ip-forward=false",
            "--ip-masq=false", "--insecure-registry=127.0.0.1:5000"])
        def engine_ready():
            need(dockerd.poll() is None, "dedicated dockerd exited")
            return json.loads(docker("info", "--format", "{{json .}}"))
        info = poll("dedicated engine", engine_ready)
        need(info["DockerRootDir"] == "/case/engine/data", "wrong engine data root")
        evidence["inner_engine"] = {k: info.get(k) for k in ("ID", "ServerVersion", "DockerRootDir", "Driver")}
        run(["skopeo", "copy", "--preserve-digests", "--dest-tls-verify=false",
             "oci:/opt/p4/child:acceptance", "docker://127.0.0.1:5000/p4/child:acceptance"],
            env=env, timeout=180, capture_output=True)
        docker("pull", image, timeout=180)
        im = json.loads(docker("image", "inspect", image))[0]
        need(image in im.get("RepoDigests", []), "embedded image digest was not preserved")
        need(im["Os"] == "linux" and not im["Config"].get("Volumes"), "invalid child image")
        need(set(os.listdir("/sys/class/net")) == {"lo"}, "fixture has external networking")
        check("offline-digest-provisioning", child_image=image, engine_id=info["ID"])

        (home / ".weftos").mkdir(mode=0o700)
        operator = home / ".weftos/project-container.json"
        operator.write_text(json.dumps({"engine": "docker", "image": image}))
        operator.chmod(0o600)
        (base / "config.json").write_text(json.dumps({"kernel": {
            "chain": {"enabled": True, "checkpoint_path": str(runtime / "chain.json")},
            "mesh": {"enabled": False, "service": "off"}, "ipc_tcp": {"enabled": False},
            "governance": {"outside_project": "allow_all"},
            "llm": {"service_url": "http://127.0.0.1:1", "model": "fixture-no-model"}}}))
        (root / "weave.toml").write_text('[kernel.mesh]\nenabled=false\nservice="off"\n[kernel.ipc_tcp]\nenabled=false\n')
        owner = runtime / "kernel.sock"
        parent, parent_identity = start_parent()
        project_id = result(owner, "project.register", {"root": str(root), "name": "driver-fixture"})["project"]["id"]
        need(re.fullmatch(r"[0-9A-HJKMNP-TV-Z]{26}", project_id), "invalid project ID")
        child_run = runtime / project_id
        child_socket = child_run / "guest/kernel.sock"
        manifest = home / ".weftos/projects" / (project_id + ".toml")
        text = manifest.read_text()
        parsed = tomllib.loads(text)
        need(set(parsed.get("serve", {})) <= {"via", "sandbox"}, "unexpected fresh serve options")
        section = '[serve]\nvia="child-kernel"\nsandbox="linux-container"\nidle_stop_secs=1800\nrestart_max=3\nrestart_window_secs=600\n\n'
        if "serve" in parsed:
            text, count = re.subn(r"(?ms)^\[serve\][ \t]*\n.*?(?=^\[|\Z)", lambda _: section, text)
            need(count == 1, "serve table not found")
        else:
            text += "\n" + section
        need(tomllib.loads(text)["serve"]["sandbox"] == "linux-container", "wrong driver selected")
        replacement = manifest.with_suffix(".new")
        replacement.write_text(text)
        replacement.chmod(0o600)
        replacement.replace(manifest)
        started = result(owner, "project.start", {"id": project_id}, timeout=150)
        saved = state()["container"]
        a = saved["id"]
        need(saved["engine"] == "docker" and saved["host_socket"] == str(child_socket), "wrong persisted transport")
        cert = certify()
        h = proof(a, cert)
        poll("initial container-bound session", lambda: session(a, h, cert))
        need(session(a, h, cert, bad=True), "invalid v2 binding accepted")
        need(child_ids() == [a], "duplicate initial containers")
        snapshot(a, "initial")
        check("real-driver-signed-registration", container_id=a, guest_pid=h["pid"], host_pid=owned(a)["State"]["Pid"])
        probe = json.loads(docker("exec", "-i", a, "python3", "-I", "-c", BOUNDARY_PROBE))
        need(probe["boundary"] == "pass" and probe["weaver_sha256"] == args.weaver_sha256, "child boundary probe failed")
        check("child-filesystem-uds-network-isolation")

        owned(a)  # immediately verify before deliberate fault injection
        docker("kill", "--signal=KILL", a)
        def restarted():
            status = result(owner, "project.status", {"id": project_id})
            c = status.get("container") or {}
            return c.get("id") if status.get("state") == "running" and c.get("id") != a else False
        b = poll("automatic adapter restart", restarted, 150)
        need(a not in docker("ps", "-a", "--no-trunc", "-q").splitlines(), "old container was not removed")
        need(certify() == cert, "crash restart changed certificate")
        h = proof(b, cert)
        poll("replacement session", lambda: session(b, h, cert))
        snapshot(b, "restart")
        check("automatic-restart-new-container-same-key", previous=a, replacement=b)

        result(owner, "kernel.shutdown")
        need(parent.wait(timeout=40) == 0, "parent shutdown failed")
        need(owned(b)["State"]["Running"], "child did not survive parent exit")
        parent, again = start_parent()
        need(again["user_key_id"] == parent_identity["user_key_id"], "parent key changed")
        def adopted():
            status = result(owner, "project.status", {"id": project_id})
            return status.get("state") == "running" and (status.get("container") or {}).get("id") == b
        poll("container adoption", adopted)
        ensured = result(owner, "project.ensure_running", {"id": project_id}, timeout=150)
        need(ensured["started"] is False and state()["container"]["id"] == b, "adoption respawned child")
        h = proof(b, cert)
        poll("adopted signed session", lambda: session(b, h, cert))
        poll("adoption chain receipt", lambda: any(e["kind"] == "project.kernel.adopted" for e in
             result(owner, "chain.tail", {"count": 0})))
        need(child_ids() == [b] and certify() == cert, "adoption duplicate or changed identity")
        snapshot(b, "adopted")
        check("parent-restart-adopts-same-container-and-session", container_id=b)

        result(owner, "project.revoke", {"id": project_id, "reason": "disposable driver acceptance"}, timeout=150)
        poll("revoked container stopped", lambda: not owned(b)["State"]["Running"])
        need((child_run / "revoked").is_file(), "revocation marker absent")
        for method in ("project.start", "project.restart", "project.ensure_running"):
            reply = rpc(owner, method, {"id": project_id}, timeout=150)
            need(reply.get("ok") is False and reply.get("error_kind") == "project_revoked", f"revocation bypass: {method}")
        need(not session(b, h, cert), "revoked session still live")
        snapshot(b, "revoked-exited")
        check("revoke-stops-container-and-refuses-all-starts")
        result(owner, "kernel.shutdown")
        need(parent.wait(timeout=40) == 0, "final parent shutdown failed")
        need(not owner.exists() and not (runtime / "child-ipc/child.sock").exists(), "parent sockets remain")
        pending = {"status": "pass", "scope": "linux-container-driver", "container_ids": [a, b],
                   "weaver_sha256": args.weaver_sha256, "child_image": image,
                   "source_receipt": args.source_receipt, "project_id": project_id,
                   "d10_accepted": False, **evidence}
    except BaseException:
        for path in sorted(base.glob("*.log")):
            print(f"--- {path.name}: last 35 lines ---", file=sys.stderr)
            print("\n".join(path.read_text(errors="replace").splitlines()[-35:]), file=sys.stderr)
        raise
    finally:
        # Stop parent before removing child records so supervision cannot race.
        if parent is not None and parent.poll() is None:
            parent.terminate()
            try:
                parent.wait(timeout=30)
            except subprocess.TimeoutExpired:
                parent.kill()
                parent.wait(timeout=10)
        if project_id is not None:
            try:
                for cid in child_ids():
                    owned(cid)
                    docker("rm", "--force", cid)
                need(not child_ids(), "project containers survived cleanup")
            except Exception as error:
                cleanup_errors.append(str(error))
        for proc in reversed(processes):
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=10)
        for handle in handles:
            handle.close()
        need(not cleanup_errors, f"inner cleanup failed: {cleanup_errors}")
    need(pending is not None, "missing lifecycle result")
    print("P4_DRIVER_RECEIPT=" + json.dumps({**pending, "inner_cleanup": "pass"}), flush=True)


def host(args):
    need(PIN.fullmatch(args.fixture_image or ""), "fixture image must be pinned by digest")
    need(HEX.fullmatch(args.child_digest) and HEX.fullmatch(args.weaver_sha256), "hashes must be 64 lowercase hex")
    need(bool(args.source_receipt.strip()), "source receipt required")
    if not args.run:
        print(json.dumps({"mode": "plan-only", "scope": "linux-container-driver", "engine_operations": False,
                          "fixture_image": args.fixture_image, "child_digest": args.child_digest,
                          "privileged_fixture": True, "host_socket_mount": False}, indent=2))
        return
    need(args.docker_context and args.outer_engine_id and args.report_dir,
         "run requires explicit disposable --docker-context, --outer-engine-id and new --report-dir")
    reports = args.report_dir.resolve()
    reports.mkdir(parents=True, exist_ok=False)
    token = secrets.token_hex(12)
    name = "p4-driver-" + token
    cid, attempted = None, False
    errors = []
    env = {k: v for k, v in os.environ.items() if k not in ("DOCKER_HOST", "DOCKER_CONTEXT")}
    prefix = ["docker", "--context", args.docker_context]

    def docker(*argv):
        return run([*prefix, *argv], env=env, capture_output=True, text=True).stdout.strip()

    def outer():
        rec = json.loads(docker("inspect", cid))[0]
        need(rec["Id"] == cid and rec["Config"]["Labels"].get(LABEL) == token, "outer ownership mismatch")
        return rec

    info = json.loads(docker("info", "--format", "{{json .}}"))
    need(info["ID"] == args.outer_engine_id and info["OSType"] == "linux", "wrong disposable outer engine")
    image = json.loads(docker("image", "inspect", args.fixture_image))[0]
    need(image["Os"] == "linux" and not image["Config"].get("Volumes"), "fixture image has implicit volumes")
    need(not image["Config"].get("OnBuild"), "fixture image has build triggers")
    try:
        attempted = True
        docker("volume", "create", "--label", f"{LABEL}={token}", name)
        payload = {"code": Path(__file__).read_text(),
                   "common": Path(__file__).with_name("p4-container-lifecycle.py").read_text()}
        bootstrap = "import json,sys; p=json.load(sys.stdin); COMMON=p['common']; exec(compile(p['code'],'<p4-driver>','exec'))"
        cid = docker("create", "--pull=never", "--name", name, "--label", f"{LABEL}={token}",
            "--privileged", "--network=none", "--no-healthcheck", "--user=0:0",
            "--mount", f"type=volume,src={name},dst=/case", "--env=P4_DRIVER_GUEST=1",
            "--workdir=/case", "--entrypoint=python3", args.fixture_image, "-I", "-B", "-c", bootstrap,
            "--guest", "--child-digest", args.child_digest, "--weaver-sha256", args.weaver_sha256,
            "--source-receipt", args.source_receipt)
        need(HEX.fullmatch(cid), "invalid outer engine ID")
        record = outer()
        hc = record["HostConfig"]
        need(hc["Privileged"] and hc["NetworkMode"] == "none" and hc.get("PidMode", "") != "host",
             "wrong fixture namespaces")
        mounts = record["Mounts"]
        need(len(mounts) == 1 and mounts[0]["Type"] == "volume" and mounts[0].get("Name") == name
             and mounts[0]["Destination"] == "/case", "fixture has unexpected/host mounts")
        (reports / "outer-inspect.json").write_text(json.dumps(record, indent=2))
        with (reports / "driver.log").open("wb") as log:
            run([*prefix, "start", "--attach", "--interactive", cid], env=env, timeout=1200,
                input=json.dumps(payload).encode(), stdout=log, stderr=subprocess.STDOUT)
        rec = outer()
        need(not rec["State"]["Running"] and rec["State"]["ExitCode"] == 0, "driver fixture failed")
        receipts = [json.loads(line.split("=", 1)[1]) for line in (reports / "driver.log").read_text().splitlines()
                    if line.startswith("P4_DRIVER_RECEIPT=")]
        need(len(receipts) == 1 and receipts[0]["status"] == "pass" and receipts[0]["inner_cleanup"] == "pass",
             "missing driver/inner cleanup evidence")
        receipt = receipts[0]
    finally:
        if attempted:
            try:
                if not (cid and HEX.fullmatch(cid)):
                    ids = docker("ps", "-a", "--no-trunc", "-q", "--filter", f"label={LABEL}={token}").splitlines()
                    need(len(ids) <= 1, "ambiguous fixture resources")
                    cid = ids[0] if ids else None
                if cid:
                    outer()
                    docker("rm", "--force", cid)
                    need(cid not in docker("ps", "-a", "--no-trunc", "-q").splitlines(), "fixture remains")
                v = json.loads(docker("volume", "inspect", name))[0]
                need(v.get("Labels", {}).get(LABEL) == token, "volume ownership mismatch")
                docker("volume", "rm", name)
                need(name not in docker("volume", "ls", "-q").splitlines(), "fixture volume remains")
            except Exception as error:
                errors.append(str(error))
        (reports / "cleanup.json").write_text(json.dumps({"status": "fail" if errors else "pass", "errors": errors}, indent=2))
        need(not errors, f"outer cleanup failed: {errors}")
    # No acceptance receipt until lifecycle AND both cleanup layers succeeded.
    (reports / "receipt.json").write_text(json.dumps({**receipt, "fixture_image": args.fixture_image,
        "outer_engine_id": args.outer_engine_id, "outer_container_id": cid,
        "outer_cleanup": "pass", "container_driver_accepted": True}, indent=2))
    print(f"PASS real LinuxContainer adapter; evidence: {reports}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture-image")
    parser.add_argument("--child-digest", required=True, help="64-hex OCI manifest digest, without sha256:")
    parser.add_argument("--weaver-sha256", required=True)
    parser.add_argument("--source-receipt", required=True, help="evaluated commit plus dirty-delta/artifact provenance")
    parser.add_argument("--docker-context")
    parser.add_argument("--outer-engine-id")
    parser.add_argument("--report-dir", type=Path)
    parser.add_argument("--run", action="store_true")
    parser.add_argument("--guest", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.guest:
        guest(args, globals()["COMMON"])
    else:
        host(args)


if __name__ == "__main__":
    main()
