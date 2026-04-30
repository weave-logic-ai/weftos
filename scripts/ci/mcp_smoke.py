#!/usr/bin/env python3
"""End-to-end smoke test for `weft mcp-server` (WEFT-487).

Spawns the binary on stdio, performs the MCP `initialize` handshake,
asserts `tools/list` reports at least one tool, then calls
`tools/call` against the simplest read-only tool that is exposed
(currently `read_file` against `Cargo.toml`) and asserts a result
content block came back.

The test is wire-shape only — it does not assert specific tool names
beyond confirming the registry is populated, so adding/removing
tools should not break this gate.

Exit code:
    0 — all assertions passed
    1 — assertion failed (output explains which step)
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time

BIN = os.environ.get("WEFT_BIN", "target/debug/weft")
TIMEOUT_SECS = 30


def _send(proc: subprocess.Popen, payload: dict) -> None:
    line = json.dumps(payload) + "\n"
    assert proc.stdin is not None
    proc.stdin.write(line.encode("utf-8"))
    proc.stdin.flush()


def _recv(proc: subprocess.Popen) -> dict:
    assert proc.stdout is not None
    deadline = time.monotonic() + TIMEOUT_SECS
    while time.monotonic() < deadline:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("mcp-server closed stdout before sending a response")
        line = line.decode("utf-8", errors="replace").strip()
        if not line:
            continue
        # Skip log noise that isn't valid JSON-RPC.
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(obj, dict) and ("result" in obj or "error" in obj or "method" in obj):
            return obj
    raise TimeoutError(f"mcp-server timed out after {TIMEOUT_SECS}s")


def _expect_result(resp: dict, label: str) -> dict:
    if "error" in resp:
        raise AssertionError(f"{label}: server returned error: {resp['error']}")
    if "result" not in resp:
        raise AssertionError(f"{label}: response had no 'result': {resp}")
    return resp["result"]


def main() -> int:
    if not os.path.isfile(BIN) or not os.access(BIN, os.X_OK):
        print(f"FAIL: WEFT_BIN={BIN} is not an executable file", file=sys.stderr)
        return 1

    print(f"[smoke] launching {BIN} mcp-server")
    proc = subprocess.Popen(
        [BIN, "mcp-server"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env={**os.environ, "RUST_LOG": "warn"},
    )
    try:
        # ── 1. initialize ──────────────────────────────────────────
        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {}},
                    "clientInfo": {"name": "ci-smoke", "version": "0.0.0"},
                },
            },
        )
        init = _expect_result(_recv(proc), "initialize")
        assert "protocolVersion" in init, f"initialize: missing protocolVersion: {init}"
        print(f"[smoke] initialize OK; server protocolVersion={init['protocolVersion']}")

        # MCP requires the initialized notification before further requests.
        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {},
            },
        )

        # ── 2. tools/list ──────────────────────────────────────────
        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {},
            },
        )
        listed = _expect_result(_recv(proc), "tools/list")
        tools = listed.get("tools", [])
        assert isinstance(tools, list) and tools, (
            f"tools/list: expected at least one tool, got {tools!r}"
        )
        names = [t.get("name") for t in tools if isinstance(t, dict)]
        print(f"[smoke] tools/list OK; {len(tools)} tools (sample: {names[:5]})")

        # Pick a tool we can safely call. Prefer the builtin
        # read_file (exact or namespaced); fall back to the first
        # listed tool. The smoke is about wire shape, not domain
        # success — even a missing-file error is a valid result
        # envelope that proves the round-trip works.
        target = None
        for candidate in ("read_file", "builtin__read_file"):
            if candidate in names:
                target = candidate
                break
        if target is None:
            target = names[0]

        if "read_file" in target:
            args = {"path": "Cargo.toml"}
        else:
            args = {}
        _send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": target, "arguments": args},
            },
        )
        called = _expect_result(_recv(proc), f"tools/call {target}")
        # MCP shape: result has "content": [...] (and optional isError).
        content = called.get("content")
        assert isinstance(content, list), (
            f"tools/call {target}: missing 'content' list, got {called!r}"
        )
        # We accept is_error=true here — the smoke is about wire shape,
        # not domain success. A misnamed path still produces a valid
        # result envelope.
        print(f"[smoke] tools/call {target} OK; content blocks={len(content)}")

        print("[smoke] PASS")
        return 0
    except (AssertionError, TimeoutError, RuntimeError) as e:
        print(f"FAIL: {e}", file=sys.stderr)
        if proc.stderr is not None:
            tail = proc.stderr.read()
            if tail:
                sys.stderr.write("--- mcp-server stderr ---\n")
                sys.stderr.write(tail.decode("utf-8", errors="replace"))
        return 1
    finally:
        try:
            proc.stdin.close()  # type: ignore[union-attr]
        except Exception:
            pass
        try:
            proc.terminate()
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
