#!/usr/bin/env python3
"""Boot an isolated real user daemon and verify its child RPC endpoint.

Requires a freshly built weaver with the Phase 4 child listener. All state
lives under this checkout's target directory; no installed daemon is used.
"""
import argparse
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time


def rpc(path, method, params=None, project=None):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(5)
        client.connect(str(path))
        client.sendall((json.dumps({"id": "smoke", "proto": 1,
                                   "method": method, "params": params or {},
                                   "auth": "admin", "project": project}) + "\n").encode())
        with client.makefile("rb") as response:
            line = response.readline(1024 * 1024)
        if not line.endswith(b"\n"):
            raise AssertionError("missing or oversized RPC response")
        return json.loads(line)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    root = Path(__file__).resolve().parents[2]
    scratch = root / "target" / "p4s"
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="run-", dir=scratch) as directory:
        work = Path(directory)
        home, runtime = work / "home", work / "run"
        if len(str(runtime / "child-ipc" / "child.sock").encode()) >= 104:
            raise RuntimeError("checkout path is too long for a macOS Unix socket; run from the short main checkout")
        home.mkdir()
        runtime.mkdir()
        config = work / "config.json"
        config.write_text(json.dumps({"kernel": {
            "mesh": {"enabled": False, "service": "off"},
            "llm": {"service_url": "http://127.0.0.1:0", "model": "smoke-only"}
        }}))
        env = {"HOME": str(home), "WEFTOS_RUNTIME_DIR": str(runtime),
               "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "TMPDIR": str(work),
               "RUST_LOG": "info"}
        log = work / "daemon.log"
        with log.open("wb") as output:
            child = subprocess.Popen(
                [str(binary), "kernel", "start", "--foreground", "--profile", "user",
                 "--config", str(config), "--new-chain"],
                cwd=work, env=env, stdin=subprocess.DEVNULL,
                stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            owner = runtime / "kernel.sock"
            restricted = runtime / "child-ipc" / "child.sock"
            try:
                deadline = time.monotonic() + 45
                while not (owner.exists() and restricted.exists()):
                    if child.poll() is not None:
                        raise AssertionError(f"daemon exited during startup: {child.returncode}")
                    if time.monotonic() >= deadline:
                        if owner.exists() and rpc(owner, "kernel.status").get("ok") is True:
                            raise AssertionError("owner endpoint is healthy but child listener is missing")
                        raise AssertionError("daemon did not become ready")
                    time.sleep(0.1)
                status = rpc(owner, "kernel.status")
                assert status.get("ok") is True, status
                for method in ("auth.token.issue", "project.revoke", "kernel.shutdown"):
                    response = rpc(restricted, method)
                    assert response.get("ok") is False, response
                    assert response.get("error_kind") == "child_endpoint_method_denied", response
                unclaimed = rpc(restricted, "mesh.challenge")
                assert unclaimed.get("error_kind") == "project_required", unclaimed
                project_root = work / "project"
                project_root.mkdir()
                registered = rpc(owner, "project.register", {"root": str(project_root)})
                assert registered.get("ok") is True, registered
                project_id = registered["result"]["project"]["id"]
                challenge = rpc(restricted, "mesh.challenge", project=project_id)
                assert challenge.get("error_kind") == "invalid_params", challenge
                assert child.poll() is None, "child endpoint shut down the daemon"
                result = rpc(owner, "kernel.shutdown")
                assert result.get("ok") is True, result
                assert child.wait(timeout=20) == 0
                assert not owner.exists(), "owner socket was not cleaned up"
                assert not restricted.exists(), "child socket was not cleaned up"
                print("PASS real user daemon: owner RPC, child method ceiling, bootstrap dispatch, cleanup")
            except BaseException:
                print(log.read_text(errors="replace")[-12000:])
                raise
            finally:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=5)


if __name__ == "__main__":
    main()
