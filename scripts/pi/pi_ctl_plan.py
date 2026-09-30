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


def mac_env(work):
    """Environment for every Mac-side weaver process: all state under `work`."""
    return {
        "PATH": MAC_PATH,
        "HOME": work + "/home",
        "WEFTOS_RUNTIME_DIR": work + "/runtime",
        "TMPDIR": work + "/tmp",
        "XDG_CONFIG_HOME": work + "/home/.config",
        "XDG_DATA_HOME": work + "/home/.local/share",
        "XDG_CACHE_HOME": work + "/home/.cache",
        "XDG_STATE_HOME": work + "/home/.local/state",
    }


def env_cmd(work, argv):
    """argv under `env -i` with only the isolated environment."""
    env = mac_env(work)
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


def peers_json(pi_host, port):
    """workload-peers.json: the Pi's workload-host, operator-paired."""
    return [{"addr": "%s:%d" % (pi_host, int(port)), "tier": "paired"}]


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
MAC_NEEDS = (("workload.placement", "workload.place"), ("workload.placement", "workload.refuse"),
             ("workload.host", "workload.refuse"))


def _has(events, need):
    kinds = [(e.get("source"), e.get("kind")) for e in (events or [])]
    return any(k == need[1] and (need[0] == "mesh_artifact" or s == need[0]) for s, k in kinds)


def judge(r, pi_node, mac_node):
    """The two-node acceptance from the CLI results `r` (dict of explain,
    place, place_rc, status, reports, bad, bad_rc, pin, pin_rc, mac_chain,
    pi_chain). Returns (ok, [reasons])."""
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
    elif len(tried) < 2:
        why.append("after the Pi's admission refusal the next candidate was not tried")
    pin = r.get("pin") or {}
    first = (pin.get("attempts") or [{}])[0]
    if r.get("pin_rc") == 0 or pin.get("placed") or first.get("code") != "admission":
        why.append("pinning this Mac was not refused at admission")
    for need in PI_NEEDS:
        if not _has(r.get("pi_chain"), need):
            why.append("Pi chain lacks %s/%s" % need)
    for need in MAC_NEEDS:
        if not _has(r.get("mac_chain"), need):
            why.append("Mac chain lacks %s/%s" % need)
    return not why, why


def chain_rows(events):
    """(source, kind) of placement-related chain rows, for evidence."""
    return [(e.get("source"), e.get("kind")) for e in (events or [])
            if str(e.get("source", "")).startswith(("workload", "mesh_artifact"))]
