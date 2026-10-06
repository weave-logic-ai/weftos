#!/usr/bin/env python3
"""Exercise the ACTUAL project guest and runner; never compiles or touches HOME.
Requires Python cryptography. All state stays under --state-root (must not exist).
Artifacts are explicit: --runner native executable --guest wasm32-wasip1 .wasm.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import threading
import time
from datetime import datetime, timezone

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

MAX_FRAME = 1024 * 1024
# The runner compiles the whole guest on every start; a debug runner needs far
# longer than a release one. Overridden by --startup-timeout.
STARTUP = 15
PROJECT = "01ARZ3NDEKTSV4RRFFQ69G5FAV"


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def public(key):
    return key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex()


def key_id(key_hex):
    return hashlib.sha256(bytes.fromhex(key_hex)).hexdigest()[:32]


def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def wire_call(path, request):
    with socket.socket(socket.AF_UNIX) as sock:
        sock.settimeout(4)
        sock.connect(str(path))
        sock.sendall(canonical(request) + b"\n")
        data = bytearray()
        while not data.endswith(b"\n"):
            chunk = sock.recv(65536)
            if not chunk:
                raise RuntimeError("runner closed connection")
            data.extend(chunk)
            if len(data) > MAX_FRAME:
                raise RuntimeError("oversize reply")
        return json.loads(data)


def wait_for(predicate, seconds=None):
    deadline = time.monotonic() + (seconds or STARTUP)
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.05)
    raise AssertionError("lifecycle deadline expired")


class Parent:
    """Independent protocol oracle: checks signatures rather than accepting echo."""
    def __init__(self, path, root):
        self.path, self.root = path, root
        self.key = Ed25519PrivateKey.generate()
        self.nonces = set()
        self.sessions = set()
        self.project_pk = None
        self.registrations = 0
        self.adopted_pid = None
        self.adoption_refusals = 0
        self.heartbeats = 0
        self.unregisters = 0
        self.anchors = []
        self.fail_anchors = False
        self.fail_all = False
        self.bad_ack = False
        self.errors = []
        self.done = threading.Event()
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.bind(str(path))
        os.chmod(path, 0o600)
        self.sock.listen()
        self.sock.settimeout(.1)
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def policy(self, version=1):
        limits = dict(risk_threshold=None, max_processes=None, spawn_budget=None, human_approval_required=False)
        hash_limits = dict(limits)
        hash_limits["risk_threshold_bits"] = hash_limits.pop("risk_threshold")
        rule_hash = hashlib.sha256(b"weftos-parent-rules-v1\n" + canonical(dict(limits=hash_limits, rules=[]))).hexdigest()
        value = dict(schema=1, user_key_id=key_id(public(self.key)), version=version,
                     issued_at=now(), rules=[], limits=limits, rule_hash=rule_hash)
        signed = dict(value, limits=hash_limits)
        value["sig"] = self.key.sign(b"weftos-parent-policy-v1\n" + canonical(signed)).hex()
        return value

    def certificate(self, pk):
        value = dict(v=1, type="project-cert", project_id=PROJECT, project_pubkey=pk,
                     project_key_id=key_id(pk), user_key_id=key_id(public(self.key)), user_pubkey=public(self.key),
                     serial=1, issued_at=now(), expires_at=None)
        value["sig"] = self.key.sign(b"weftos-project-cert-v1\n" + canonical(value)).hex()
        return value

    def dispatch(self, req):
        assert req["project"] == PROJECT
        if self.fail_all:
            return dict(ok=False, error_kind="parent_unavailable")
        p, method = req["params"], req["method"]
        if method == "mesh.challenge":
            nonce = secrets.token_hex(16)
            self.nonces.add(nonce)
            result = dict(nonce=nonce, user_key_id=key_id(public(self.key)))
        elif method == "mesh.register":
            nonce = p["nonce_reply"]["nonce"]
            assert nonce in self.nonces
            self.nonces.remove(nonce)
            assert p["project_id"] == PROJECT and p["pid"] > 0
            assert p["socket"] == str(self.path.parent / "run" / "kernel.sock")
            assert p["root_sha256"] == hashlib.sha256(os.fsencode(self.root)).hexdigest()
            assert p["spawn_nonce"] in ("fixture-spawn", None)
            if p["spawn_nonce"] is None and self.adopted_pid != p["pid"]:
                self.adoption_refusals += 1
                return dict(ok=False, error_kind="spawn_not_expected")
            pk = Ed25519PublicKey.from_public_bytes(bytes.fromhex(p["project_pubkey"]))
            pop = f"weftos-mesh-local-pop-v2\nregister\n{key_id(public(self.key))}\n{nonce}\n{PROJECT}".encode()
            pk.verify(bytes.fromhex(p["nonce_reply"]["sig"]), pop)
            bind = f"weftos-mesh-local-bind-v1\n{PROJECT}\n{nonce}\n{p['client_nonce']}\n{p['socket']}\n{p['pid']}".encode()
            pk.verify(bytes.fromhex(p["bind_sig"]), bind)
            if self.project_pk is not None:
                assert self.project_pk == p["project_pubkey"], "guest regenerated its key"
            self.project_pk = p["project_pubkey"]
            session = secrets.token_hex(16)
            self.sessions.add(session)
            ack = f"weftos-mesh-local-ack-v1\n{PROJECT}\n{session}\n{nonce}\n{p['client_nonce']}".encode()
            sig = self.key.sign(ack).hex() if not self.bad_ack else "00" * 64
            result = dict(ok=True, session=session, cert=self.certificate(self.project_pk), accepted=[PROJECT],
                          proto=dict(current=1, min=1), heartbeat_secs=1,
                          parent_head=dict(user_seq=1, user_event_hash="ab" * 32), parent_sig=sig)
            self.registrations += 1
        elif method == "mesh.heartbeat":
            if p["session"] not in self.sessions:
                return dict(ok=False, error_kind="unknown_session")
            a = p["activity"]
            extra = f"{a['last_activity_unix']}:{a['busy']['agents']}:{a['busy']['workloads']}:{a['busy']['streams']}"
            msg = f"weftos-mesh-local-session-v1\nheartbeat\n{p['session']}\n{p['pid']}\n{p['at_unix']}\n{extra}".encode()
            Ed25519PublicKey.from_public_bytes(bytes.fromhex(self.project_pk)).verify(bytes.fromhex(p["sig"]), msg)
            self.heartbeats += 1
            result = {}
        elif method == "mesh.unregister":
            assert p["session"] in self.sessions
            msg = f"weftos-mesh-local-session-v1\nunregister\n{p['session']}\n{p['pid']}\n{p['at_unix']}\n".encode()
            Ed25519PublicKey.from_public_bytes(bytes.fromhex(self.project_pk)).verify(bytes.fromhex(p["sig"]), msg)
            self.sessions.remove(p["session"])
            self.unregisters += 1
            result = {}
        elif method == "project.anchor.submit":
            if self.fail_anchors:
                return dict(ok=False, error_kind="parent_unavailable")
            sig = p["sig"]
            body = {k: v for k, v in p.items() if k != "sig"}
            Ed25519PublicKey.from_public_bytes(bytes.fromhex(self.project_pk)).verify(
                bytes.fromhex(sig), b"weftos-project-anchor-v1\n" + canonical(body))
            assert p["project_id"] == PROJECT
            if not self.anchors or p != self.anchors[-1]:
                assert p["seq"] == len(self.anchors) + 1
                # ProjectAnchorStmt.hash() includes domain and canonical statement with sig.
                previous = hashlib.sha256(b"weftos-project-anchor-v1\n" + canonical(self.anchors[-1])).hexdigest() if self.anchors else None
                assert p["prev_anchor"] == previous
                self.anchors.append(p)
            result = dict(user_seq=len(self.anchors), user_event_hash="cd" * 32)
        else:
            raise AssertionError("unscoped parent operation: " + method)
        return dict(ok=True, result=result)

    def serve(self):
        while not self.done.is_set():
            try:
                conn, _ = self.sock.accept()
            except socket.timeout:
                continue
            with conn:
                try:
                    conn.settimeout(3)
                    data = conn.makefile("rb").readline(MAX_FRAME + 1)
                    assert len(data) <= MAX_FRAME
                    reply = self.dispatch(json.loads(data))
                    conn.sendall(canonical(reply) + b"\n")
                except Exception as error:
                    self.errors.append(repr(error))

    def close(self):
        self.done.set()
        self.thread.join(4)
        self.sock.close()


def forwarded(parent, method, params=None):
    stamp = int(time.time() * 1000)
    params = {} if params is None else params
    digest = hashlib.sha256(canonical(params)).hexdigest()
    msg = f"weftos-project-forward-v2\n{PROJECT}\n{stamp}\n{method}\n{digest}\n{key_id(parent.project_pk)}".encode()
    return dict(method=method, params=params, project=PROJECT, proto=1,
                forward=dict(project_id=PROJECT, issued_at_ms=stamp, sig=parent.key.sign(msg).hex()))


def main():
    global STARTUP
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--guest", type=Path, required=True)
    parser.add_argument("--state-root", type=Path, required=True)
    parser.add_argument("--startup-timeout", type=int, default=STARTUP, help="seconds a runner may take to compile and answer")
    args = parser.parse_args()
    STARTUP = args.startup_timeout
    runner, guest = args.runner.resolve(strict=True), args.guest.resolve(strict=True)
    base = args.state_root.absolute()
    if len(os.fsencode(base / "run/kernel.sock")) > 103:
        raise ValueError("state root too long for macOS UDS; use a short worktree-local path")
    base.mkdir(mode=0o700, parents=False, exist_ok=False)
    base = base.resolve()
    root, run = base / "project", base / "run"
    (root / ".weftos").mkdir(parents=True, mode=0o700)
    run.mkdir(mode=0o700)
    parent = Parent(base / "parent.sock", root)
    artifact_hash = hashlib.sha256(guest.read_bytes()).hexdigest()
    config = dict(adapter="wasmtime-project-v1", artifact=str(guest), artifact_sha256=artifact_hash,
                  project_root=str(root), parent_socket=str(parent.path), runtime_dir=str(run), project_id=PROJECT,
                  user_pubkey=public(parent.key), spawn_nonce="fixture-spawn", parent_policy=parent.policy(),
                  depth=1, parent=key_id(public(parent.key)), lifetime_fuel=10_000_000_000_000,
                  memory_bytes=256 * 1024 * 1024, lifetime_secs=180)
    config_path = base / "launch.json"
    children = []

    def start(values=None):
        config_path.write_bytes(canonical(config if values is None else values))
        os.chmod(config_path, 0o600)
        log = (base / f"runner-{len(children)}.log").open("wb")
        proc = subprocess.Popen([str(runner), str(config_path)], stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                env={"PATH": os.environ.get("PATH", "")})
        log.close()
        children.append(proc)
        return proc

    def ready(proc):
        def probe():
            if proc.poll() is not None:
                raise AssertionError(f"guest exited: {proc.returncode}; inspect {base}")
            try:
                return wire_call(run / "kernel.sock", dict(method="kernel.status"))["ok"]
            except (OSError, RuntimeError):
                return False
        wait_for(probe)

    def stop(proc):
        proc.terminate()
        proc.wait(timeout=8)

    try:
        # Wrong artifact refuses before any guest registration.
        p = start(dict(config, artifact_sha256="00" * 32))
        assert p.wait(timeout=STARTUP) != 0 and parent.registrations == 0
        # Explicit adapter and lifetime fuel are enforced before useful guest work.
        p = start(dict(config, adapter="logical"))
        assert p.wait(timeout=STARTUP) != 0 and parent.registrations == 0
        p = start(dict(config, lifetime_fuel=1))
        assert p.wait(timeout=STARTUP) != 0 and parent.registrations == 0
        # A correctly loaded guest refuses a forged parent policy.
        bad = parent.policy()
        bad["sig"] = "00" * 64
        p = start(dict(config, parent_policy=bad))
        assert p.wait(timeout=STARTUP) != 0 and parent.registrations == 0
        # Refuse a forged registration acknowledgement, even with a valid cert.
        parent.bad_ack = True
        p = start()
        assert p.wait(timeout=STARTUP) != 0
        parent.bad_ack = False
        parent.fail_anchors = True
        p = start()
        ready(p)
        first_key = (root / ".weftos/project.key").read_bytes()
        # Non-echo test: two calls change the SAME certified guest's chain.
        before = wire_call(run / "kernel.sock", dict(method="kernel.status"))["result"]["chain_seq"]
        req = forwarded(parent, "chain.append", dict(source="fixture", kind="fixture.event", payload=dict(n=1)))
        assert wire_call(run / "kernel.sock", req)["ok"]
        assert not wire_call(run / "kernel.sock", req)["ok"], "replayed forward accepted"
        after = wire_call(run / "kernel.sock", dict(method="kernel.status"))["result"]["chain_seq"]
        assert after > before
        forged = forwarded(parent, "chain.append", dict(source="fixture", kind="fixture.event"))
        forged["params"]["kind"] = "changed.after.signing"
        assert not wire_call(run / "kernel.sock", forged)["ok"]
        wrong_project = forwarded(parent, "chain.status")
        wrong_project["project"] = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
        assert not wire_call(run / "kernel.sock", wrong_project)["ok"]
        # Challenge-bound adoption includes artifact hash, PID, root and socket.
        nonce = secrets.token_hex(32)
        h = wire_call(run / "kernel.sock", dict(method="kernel.handshake", params=dict(challenge=nonce)))["result"]
        sig = h.pop("guest_sig")
        Ed25519PublicKey.from_public_bytes(bytes.fromhex(parent.project_pk)).verify(
            bytes.fromhex(sig), b"weftos-wasm-project-handshake-v1\n" + nonce.encode() + b"\n" + canonical(h))
        assert h["artifact_sha256"] == artifact_hash and h["pid"] == p.pid
        assert h["socket"] == str(run / "kernel.sock")
        assert h["root_sha256"] == hashlib.sha256(os.fsencode(root)).hexdigest()
        # Locks refuse a second runner; first remains responsive.
        duplicate = start()
        assert duplicate.wait(timeout=STARTUP) != 0
        ready(p)
        # A parent restart causes certified re-registration in the same process.
        initial = parent.registrations
        parent.sessions.clear()
        parent.adopted_pid = None
        wait_for(lambda: parent.adoption_refusals > 0)
        assert p.poll() is None and parent.registrations == initial
        # Model the supervisor filing a verified expired session, never the
        # register endpoint granting its own nonce-less authorization.
        challenge = secrets.token_hex(32)
        proof = wire_call(run / "kernel.sock", dict(method="kernel.handshake", params=dict(challenge=challenge)))["result"]
        signature = bytes.fromhex(proof.pop("guest_sig"))
        Ed25519PublicKey.from_public_bytes(bytes.fromhex(parent.project_pk)).verify(
            signature, b"weftos-wasm-project-handshake-v1\n" + challenge.encode() + b"\n" + canonical(proof))
        assert proof["pid"] == p.pid and proof["project_id"] == PROJECT
        assert proof["adapter"] == "wasmtime-project-v1" and proof["sandbox"] == "wasmtime"
        assert proof["socket"] == str(run / "kernel.sock")
        assert proof["root_sha256"] == hashlib.sha256(os.fsencode(root)).hexdigest()
        assert proof["artifact_sha256"] == config["artifact_sha256"]
        parent.adopted_pid = p.pid
        wait_for(lambda: parent.registrations > initial)
        wait_for(lambda: parent.heartbeats > 0)
        assert p.poll() is None
        # A byte trickle must not monopolize the guest's sole host-call loop.
        beats_before_trickle = parent.heartbeats
        slow = socket.socket(socket.AF_UNIX)
        slow.connect(str(run / "kernel.sock"))
        def trickle():
            try:
                for _ in range(80):
                    slow.sendall(b"x")
                    time.sleep(.05)
            except OSError:
                pass
            finally:
                slow.close()
        trickler = threading.Thread(target=trickle, daemon=True)
        trickler.start()
        wait_for(lambda: parent.heartbeats > beats_before_trickle, seconds=5)
        trickler.join(timeout=1)
        assert not trickler.is_alive() and p.poll() is None
        # Anchors persist before delivery. Restart guest and recover same key/chain.
        pending = root / ".weftos/chain/anchor-wasm-pending.json"
        wait_for(pending.exists)
        pending_bytes = pending.read_bytes()
        stop(p)
        parent.fail_anchors = False
        p = start()
        ready(p)
        wait_for(lambda: bool(parent.anchors))
        assert canonical(parent.anchors[0]) == canonical(json.loads(pending_bytes))
        assert (root / ".weftos/project.key").read_bytes() == first_key
        assert wire_call(run / "kernel.sock", dict(method="kernel.status"))["result"]["chain_seq"] > after
        # Signed tighten-only update and persistent rollback floor.
        update = forwarded(parent, "governance.parent.update", dict(policy=parent.policy(2)))
        assert wire_call(run / "kernel.sock", update)["ok"]
        time.sleep(.01)
        rollback = forwarded(parent, "governance.parent.update", dict(policy=parent.policy(1)))
        assert not wire_call(run / "kernel.sock", rollback)["ok"]
        stop(p)
        # A valid but older policy is refused at restart as well.
        p = start()
        assert p.wait(timeout=STARTUP) != 0
        config["parent_policy"] = parent.policy(2)
        # Project overlay prevents a previously allowed mutation.
        (root / ".weftos/overlay.toml").write_text('schema = 1\n[[deny]]\nid = "no-append"\nactions = ["chain.append"]\nreason = "test"\n')
        p = start()
        ready(p)
        assert not wire_call(run / "kernel.sock", forwarded(parent, "chain.append", dict(source="fixture", kind="blocked")))["ok"]
        assert not wire_call(run / "kernel.sock", dict(method="kernel.shutdown"))["ok"]
        assert p.poll() is None
        anchors_before_shutdown = len(parent.anchors)
        assert wire_call(run / "kernel.sock", forwarded(parent, "kernel.shutdown"))["ok"]
        assert p.wait(timeout=8) == 0 and parent.unregisters > 0
        assert len(parent.anchors) > anchors_before_shutdown
        # Idle lifetime cancellation also works when the guest is in host polling.
        p = start(dict(config, lifetime_secs=2))
        ready(p)
        assert p.wait(timeout=8) != 0
        p = start()
        ready(p)
        # Revocation is out of the guest's writable preopen and kills the runner.
        (run / "revoked").write_text("revoked")
        assert p.wait(timeout=8) != 0
        assert not parent.errors, parent.errors
        print(f"PASS: certified persistent guest lifecycle; receipts in {base}")
    finally:
        for proc in children:
            if proc.poll() is None:
                stop(proc)
        parent.close()


if __name__ == "__main__":
    main()
