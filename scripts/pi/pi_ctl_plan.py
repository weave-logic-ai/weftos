"""Pure helpers for the Mac side of the placement stage (card mesh-placement-12).

The Mac controller is a real, isolated `weaver` daemon (own HOME, runtime
dir, node key and chain under a temp dir; mesh transport off, so it binds
no port and never meets the operator's daemon or ~/.clawft), driven only
through the `weaver workload` CLI: the same path an operator uses.
"""
import json
import re

RELEASE_URL = "https://github.com/cognitum-one/cogs/releases"
MAC_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"
HEX64_RE = re.compile(r"^[0-9a-f]{64}$")
KEYGEN_RE = re.compile(r"^public key ([0-9a-f]{64})$", re.M)
# Unix socket paths are limited (SUN_LEN, 104 on macOS).
MAX_SOCKET_PATH = 100


def mac_env(work, docker=None):
    """Environment for every Mac-side weaver process: all state under `work`.
    `docker` = (cli dir, DOCKER_HOST) lets the container adapter reach the
    operator's engine (its images and containers, not its config)."""
    env = {
        "PATH": MAC_PATH,
        "HOME": work + "/home",
        "WEFTOS_RUNTIME_DIR": work + "/runtime",
        "TMPDIR": work + "/tmp",
        "XDG_CONFIG_HOME": work + "/home/.config",
        "XDG_DATA_HOME": work + "/home/.local/share",
        "XDG_CACHE_HOME": work + "/home/.cache",
        "XDG_STATE_HOME": work + "/home/.local/state",
    }
    if docker:
        env["PATH"] = docker[0] + ":" + MAC_PATH
        env["DOCKER_HOST"] = docker[1]
    return env


def env_cmd(work, argv, docker=None):
    """argv under `env -i` with only the isolated environment."""
    env = mac_env(work, docker)
    return ["env", "-i"] + ["%s=%s" % kv for kv in sorted(env.items())] + list(argv)


def socket_fits(work):
    return len(work + "/runtime/kernel.sock") < MAX_SOCKET_PATH


def parse_keygen(out):
    """Public key printed by `weaver workload keygen`, or None."""
    m = KEYGEN_RE.search(out or "")
    return m.group(1) if m else None


def seed_bytes(key_file_text):
    """The raw 32-byte node key (daemon `node.key`) from a hex key file."""
    text = (key_file_text or "").strip()
    if not HEX64_RE.match(text):
        raise ValueError("key file is not 64 hex")
    return bytes.fromhex(text)


def mislabeled_elf():
    """An x86-64 ELF header, shipped as the `aarch64` binary of a package:
    the facts say the Pi fits it natively; only the Pi's adapter admission
    self-check (ELF e_machine) can tell, before anything executes."""
    head = b"\x7fELF" + bytes([2, 1, 1, 0]) + bytes(8) + (2).to_bytes(2, "little") \
        + (62).to_bytes(2, "little")
    return head + bytes(64 - len(head))


IMAGE_RE = re.compile(r"^[a-z0-9][a-z0-9._/-]{0,127}@sha256:[0-9a-f]{64}$")


def peers_json(pi_host, port, key=None):
    """workload-peers.json: the Pi's workload-host, operator-paired; with
    `key` (64 hex) the tier is bound to that node key."""
    entry = {"addr": "%s:%d" % (pi_host, int(port)), "tier": "paired"}
    if key is not None:
        if not HEX64_RE.match(key or ""):
            raise ValueError("peer key is not 64 hex")
        entry["key"] = key
    return [entry]


def pi_key(status, pi_node):
    """The Pi target's key as the Mac learned it (`workload status --json`)."""
    for t in (status or {}).get("targets") or []:
        if t.get("node_id") == pi_node and HEX64_RE.match(t.get("public_key") or ""):
            return t["public_key"]
    return None


def container_name(iid):
    """The Docker adapter's container name for an instance id (as
    `container_cmd::container_name`)."""
    return "weftos-" + "".join(c.lower() if (c.isascii() and c.isalnum()) or c == "-" else "-"
                               for c in iid)


def container_json(image):
    """workload-container.json: a Docker adapter on the Mac for aarch64,
    with an operator-pinned local base image."""
    if not IMAGE_RE.match(image or ""):
        raise ValueError("--mac-container must be name@sha256:<64 hex>")
    return {"engine": "docker", "base_image": image, "arches_native": ["aarch64"]}


def load_json(text):
    try:
        v = json.loads(text or "")
    except ValueError:
        return None
    return v if isinstance(v, dict) else None


def mac_ready(status, pi_node):
    """This daemon's node id once its own host is pinned and the Pi's host is
    a reachable, paired target (from `workload status --json`), else None."""
    if not status:
        return None
    me = status.get("controller")
    targets = status.get("targets") or []
    local = any(t.get("node_id") == me and t.get("tier") == "pinned" for t in targets)
    pi = any(t.get("node_id") == pi_node and t.get("tier") == "paired" and t.get("reachable")
             for t in targets)
    return me if (me and local and pi) else None


def count_reports(stdout):
    """(count, first) of anomaly-detect report lines (JSON with `stats`)."""
    reports = []
    for line in (stdout or "").splitlines():
        try:
            v = json.loads(line.strip())
        except ValueError:
            continue
        if isinstance(v, dict) and "stats" in v:
            reports.append(v)
    return len(reports), (reports[0] if reports else None)


PI_NEEDS = (("mesh_artifact", "artifact.fetch"), ("workload.host", "workload.place"),
            ("workload.runtime", "workload.start"), ("workload.runtime", "workload.stop"),
            ("workload.host", "workload.refuse"))
MAC_NEEDS = (("workload.placement", "workload.place"), ("workload.placement", "workload.refuse"))
# Only a Mac with a container adapter is dispatched to, so only then does
# its own host refuse (the mislabeled binary) on its chain.
MAC_CONTAINER_NEEDS = (("workload.host", "workload.refuse"),)


def _has(events, need):
    kinds = [(e.get("source"), e.get("kind")) for e in (events or [])]
    return any(k == need[1] and (need[0] == "mesh_artifact" or s == need[0]) for s, k in kinds)


def judge(r, pi_node, mac_node, mac_container=False):
    """The two-node acceptance from the CLI results `r` (dict of explain,
    place, place_rc, status, reports, bad, bad_rc, pin, pin_rc, pin_status,
    peer_key_pinned, mac_chain, pi_chain). `mac_container`: the Mac serves
    a Docker adapter. Returns (ok, [reasons])."""
    why = []
    want = "PLACED on %s via aarch64-native (tier native" % pi_node
    ex = (r.get("explain") or {}).get("explain", "")
    if want not in ex or (mac_node or "?") not in ex:
        why.append("`weaver workload explain` did not choose the Pi over this Mac")
    place = r.get("place") or {}
    placed = place.get("placed") or {}
    if r.get("place_rc") != 0 or placed.get("node_id") != pi_node:
        why.append("`weaver workload place` did not place on the Pi (rc %s)" % r.get("place_rc"))
    if ((r.get("status") or {}).get("status") or {}).get("state") != "running":
        why.append("the placed cog was not running on the Pi")
    if not r.get("reports"):
        why.append("the cog produced no anomaly reports")
    bad = r.get("bad") or {}
    tried = bad.get("attempts") or [{}]
    if r.get("bad_rc") == 0 or bad.get("placed") or tried[0].get("node_id") != pi_node \
            or tried[0].get("code") != "admission" \
            or "ELF machine" not in (tried[0].get("reason") or ""):
        why.append("the Pi's admission self-check did not refuse the mislabeled binary")
    elif mac_container and (len(tried) < 2 or tried[1].get("node_id") != mac_node
                            or tried[1].get("code") != "admission"):
        why.append("after the Pi's admission refusal the next candidate was not tried")
    elif not mac_container and len(tried) != 1:
        why.append("a node without a container adapter was dispatched to")
    pin = r.get("pin") or {}
    pinned = pin.get("placed") or {}
    if mac_container:
        # The Mac serves a real container adapter: a pin runs it there.
        if r.get("pin_rc") != 0 or pinned.get("node_id") != mac_node \
                or pinned.get("variant") != "aarch64-container":
            why.append("pinning this Mac did not run the cog in its container adapter")
        elif ((r.get("pin_status") or {}).get("status") or {}).get("state") != "running":
            why.append("the cog pinned to this Mac was not running in its container")
    elif r.get("pin_rc") == 0 or pinned or pin.get("attempts"):
        # No container adapter: never offered, so nothing is dispatched.
        why.append("pinning this Mac without a container adapter was not refused by the engine")
    if not r.get("peer_key_pinned"):
        why.append("the Pi stayed paired only by address, not by its pinned key")
    for need in PI_NEEDS:
        if not _has(r.get("pi_chain"), need):
            why.append("Pi chain lacks %s/%s" % need)
    for need in MAC_NEEDS + (MAC_CONTAINER_NEEDS if mac_container else ()):
        if not _has(r.get("mac_chain"), need):
            why.append("Mac chain lacks %s/%s" % need)
    return not why, why


def chain_rows(events):
    """(source, kind) of placement-related chain rows, for evidence."""
    return [(e.get("source"), e.get("kind")) for e in (events or [])
            if str(e.get("source", "")).startswith(("workload", "mesh_artifact"))]
