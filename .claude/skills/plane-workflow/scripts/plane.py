#!/usr/bin/env python3
"""Plane HTTP-API wrapper for the plane-workflow skill.

Usage:
    plane.sh <subcommand> [args...]

Subcommands:
    refresh-ids                     Re-pull states/cycles/labels into ids.json.
    ensure-labels                   Create any labels in labels.json that
                                    don't yet exist; cache UUIDs into ids.json.
    me                              Print the current user's UUID + email.
    list-states                     List all states.
    list-cycles                     List all cycles.
    list-labels                     List all labels.
    list-issues [--cycle C]         List work items (optionally one cycle).
    list-cycle <cycle-name>         List items in a named cycle.
    search <query>                  Substring search across name + description.

    create-issue --name N [--priority P] [--state-name S] [--cycle C]
                 [--labels L1,L2,...] [--description-md PATH | --description STR]
                 [--assignee me|UUID]
                 Create a work item. Prints sequence_id (WEFT-N) + UUID.

    add-to-cycle <cycle-name> <issue-id>...
                 Add one or more issue UUIDs to a cycle.

    transition <issue-id> <state-name> [--assignee me|UUID]
                 PATCH the issue's state (and assignee, if --assignee given).

    defer <issue-id> <new-cycle-name> --reason "..."
                 Move issue between cycles + post a comment with the reason.

    close <issue-id> --shipped TEXT --commits SHA[,SHA]
                 [--tests TEXT] [--build TEXT] [--followups WEFT-N[,...]]
                 Transition to Done + post a structured close comment.

    comment <issue-id> [--body-md PATH | --body STR]
                 Post a comment on a work item.

    check                           Validate label coverage + cycle assignment
                                    + acceptance-criteria presence on all
                                    audit-finding work items.

ids.json (sibling references/ dir) is the cached UUID source-of-truth.
labels.json is the canonical label set.

Environment: PLANE_API_KEY (required, exported by plane.sh wrapper).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Iterable

# ---------------------------------------------------------------------------
# Paths + identifiers
# ---------------------------------------------------------------------------

SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent
REFS = SKILL_DIR / "references"
IDS_FILE = REFS / "ids.json"
LABELS_FILE = REFS / "labels.json"


def load_ids() -> dict:
    return json.loads(IDS_FILE.read_text())


def save_ids(ids: dict) -> None:
    IDS_FILE.write_text(json.dumps(ids, indent=2) + "\n")


# ---------------------------------------------------------------------------
# HTTP
# ---------------------------------------------------------------------------


class PlaneError(RuntimeError):
    pass


def _api_key() -> str:
    k = os.environ.get("PLANE_API_KEY", "")
    if not k:
        raise PlaneError("PLANE_API_KEY not set; run via scripts/plane.sh")
    return k


_LAST_REQ = 0.0
_RATE_INTERVAL = 0.25  # 4 req/sec — Plane rate-limits both GET and write


def request(
    method: str,
    path: str,
    body: dict | None = None,
    *,
    expect_status: tuple[int, ...] = (200, 201, 204),
    _retry: int = 0,
) -> Any:
    """Issue an HTTP request against the Plane API.

    `path` is appended to api_base + workspace + project. If the path starts
    with `/abs:`, the prefix is stripped and the rest is used verbatim
    (escape hatch for /workspaces/<slug>/members/ etc.).
    """
    ids = load_ids()
    base = ids["api_base"]
    ws = ids["workspace_slug"]
    proj = ids["project_id"]

    if path.startswith("/abs:"):
        url = base + path[len("/abs:") :]
    else:
        url = f"{base}/v1/workspaces/{ws}/projects/{proj}{path}"

    headers = {
        "X-API-Key": _api_key(),
        "Accept": "application/json",
        # Cloudflare in front of api.plane.so bans the default
        # `Python-urllib/X.Y` user-agent (1010 browser_signature_banned).
        # Pretend to be curl, which is on the allowlist.
        "User-Agent": "curl/8.5.0",
    }
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"

    # Universal throttle — Plane rate-limits GET as well as POST/PATCH.
    global _LAST_REQ
    delta = time.monotonic() - _LAST_REQ
    if delta < _RATE_INTERVAL:
        time.sleep(_RATE_INTERVAL - delta)
    _LAST_REQ = time.monotonic()

    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            status = resp.getcode()
            raw = resp.read()
    except urllib.error.HTTPError as e:
        if e.code == 429 and _retry < 5:
            backoff = 2.0 * (2 ** _retry)
            time.sleep(backoff)
            return request(
                method, path, body,
                expect_status=expect_status, _retry=_retry + 1,
            )
        body_txt = e.read().decode("utf-8", errors="replace")
        raise PlaneError(
            f"HTTP {e.code} on {method} {url}\n  body: {body_txt[:400]}"
        ) from None

    if status not in expect_status:
        raise PlaneError(f"unexpected status {status} on {method} {url}")

    if not raw:
        return None
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return raw.decode("utf-8", errors="replace")


def paginate(path: str, *, per_page: int = 100, max_pages: int = 50) -> Iterable[dict]:
    """Paginate Plane responses. Plane's cursor format is `total:size:offset`.

    Some Plane endpoints return a fixed cursor that doesn't advance — we
    detect and break the loop. Defensive `max_pages` cap stops runaway
    pagination.
    """
    seen_cursors: set[str] = set()
    cursor: str | None = None
    pages = 0
    while pages < max_pages:
        sep = "&" if "?" in path else "?"
        suffix = f"{sep}per_page={per_page}"
        if cursor:
            suffix += f"&cursor={urllib.parse.quote(cursor)}"
        d = request("GET", path + suffix)
        pages += 1
        if isinstance(d, list):
            yield from d
            return
        results = d.get("results", []) if isinstance(d, dict) else []
        yield from results
        next_cursor = d.get("next_cursor") if isinstance(d, dict) else None
        if not next_cursor or next_cursor in seen_cursors:
            return
        seen_cursors.add(next_cursor)
        # Plane's cursor encodes total:size:offset. If offset hasn't advanced
        # past what we've already paginated, treat as end-of-data.
        try:
            total, size, offset = (int(x) for x in next_cursor.split(":"))
            if offset == 0 or len(results) < size:
                return
        except (ValueError, AttributeError):
            pass
        cursor = next_cursor


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def get_state_id(ids: dict, name: str) -> str:
    s = ids["states"].get(name)
    if not s:
        raise PlaneError(f"unknown state: {name!r}; known: {list(ids['states'])}")
    return s["id"]


def get_cycle_id(ids: dict, name: str) -> str:
    c = ids["cycles"].get(name)
    if not c:
        raise PlaneError(f"unknown cycle: {name!r}; known: {list(ids['cycles'])}")
    return c


def get_label_ids(ids: dict, names: list[str]) -> list[str]:
    cache = ids.get("labels", {}) or {}
    out: list[str] = []
    missing: list[str] = []
    for n in names:
        v = cache.get(n)
        if not v:
            missing.append(n)
        else:
            out.append(v)
    if missing:
        raise PlaneError(
            f"missing label UUIDs: {missing}; run `plane.sh ensure-labels` first"
        )
    return out


def md_to_html(md: str) -> str:
    """Minimal markdown → HTML (good enough for Plane work-item bodies)."""
    lines = md.splitlines()
    html: list[str] = []
    in_ul = False
    in_code = False
    for line in lines:
        s = line.rstrip()
        if s.startswith("```"):
            if in_code:
                html.append("</code></pre>")
                in_code = False
            else:
                html.append("<pre><code>")
                in_code = True
            continue
        if in_code:
            html.append(escape_html(s))
            continue
        if not s.strip():
            if in_ul:
                html.append("</ul>")
                in_ul = False
            html.append("")
            continue
        m = re.match(r"^(#{1,6}) +(.*)$", s)
        if m:
            if in_ul:
                html.append("</ul>")
                in_ul = False
            level = len(m.group(1))
            html.append(f"<h{level}>{escape_html(m.group(2))}</h{level}>")
            continue
        m = re.match(r"^[-*] +(.*)$", s)
        if m:
            if not in_ul:
                html.append("<ul>")
                in_ul = True
            html.append(f"<li>{inline_md(m.group(1))}</li>")
            continue
        if in_ul:
            html.append("</ul>")
            in_ul = False
        html.append(f"<p>{inline_md(s)}</p>")
    if in_code:
        html.append("</code></pre>")
    if in_ul:
        html.append("</ul>")
    return "\n".join(h for h in html if h is not None)


def inline_md(s: str) -> str:
    s = escape_html(s)
    s = re.sub(r"`([^`]+)`", r"<code>\1</code>", s)
    s = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", s)
    s = re.sub(r"\*([^*]+)\*", r"<em>\1</em>", s)
    s = re.sub(
        r"\[([^\]]+)\]\(([^)]+)\)",
        lambda m: f'<a href="{escape_html(m.group(2))}">{m.group(1)}</a>',
        s,
    )
    return s


def escape_html(s: str) -> str:
    return (
        s.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
    )


def md_to_stripped(md: str) -> str:
    """Plain-text rendering of markdown for description_stripped."""
    s = re.sub(r"```[\s\S]*?```", "", md)
    s = re.sub(r"`([^`]+)`", r"\1", s)
    s = re.sub(r"\*\*([^*]+)\*\*", r"\1", s)
    s = re.sub(r"\*([^*]+)\*", r"\1", s)
    s = re.sub(r"^#+\s+", "", s, flags=re.M)
    s = re.sub(r"^[-*]\s+", "  • ", s, flags=re.M)
    s = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", s)
    return s.strip()


def split_csv(s: str) -> list[str]:
    return [x.strip() for x in s.split(",") if x.strip()]


# ---------------------------------------------------------------------------
# Subcommands
# ---------------------------------------------------------------------------


def cmd_refresh_ids(_args) -> None:
    ids = load_ids()
    states = list(paginate("/states/"))
    ids["states"] = {
        s["name"]: {"id": s["id"], "group": s["group"]} for s in states
    }
    cycles = list(paginate("/cycles/"))
    ids["cycles"] = {c["name"]: c["id"] for c in cycles if c["name"] in ids.get("cycles", {})} or {
        c["name"]: c["id"] for c in cycles
    }
    # Preserve the canonical 0.7.x..1.0.x naming
    by_name = {c["name"]: c["id"] for c in cycles}
    for k in ("0.7.x", "0.8.x", "0.9.x", "1.0.x"):
        if k in by_name:
            ids["cycles"][k] = by_name[k]
    labels = list(paginate("/labels/"))
    ids["labels"] = {l["name"]: l["id"] for l in labels}
    ids["last_refreshed"] = time.strftime("%Y-%m-%d")
    save_ids(ids)
    print(f"refreshed: {len(states)} states, {len(cycles)} cycles, {len(labels)} labels")


def cmd_ensure_labels(_args) -> None:
    ids = load_ids()
    canon = json.loads(LABELS_FILE.read_text())
    existing = list(paginate("/labels/"))
    by_name = {l["name"]: l for l in existing}

    cache: dict[str, str] = ids.get("labels", {}) or {}
    created = 0
    for group in ("workstream", "finding_type"):
        for spec in canon.get(group, []):
            name = spec["name"]
            if name in by_name:
                cache[name] = by_name[name]["id"]
                continue
            payload = {"name": name}
            if "color" in spec:
                payload["color"] = spec["color"]
            if "description" in spec:
                payload["description"] = spec["description"]
            r = request("POST", "/labels/", payload)
            cache[name] = r["id"]
            created += 1
            print(f"  created label: {name}")
    ids["labels"] = cache
    save_ids(ids)
    print(f"ensure-labels: {len(cache)} cached, {created} newly created")


def cmd_me(_args) -> None:
    ids = load_ids()
    ws = ids["workspace_slug"]
    members = request("GET", f"/abs:/v1/workspaces/{ws}/members/")
    rows = members if isinstance(members, list) else members.get("results", [])
    for m in rows:
        # owner of the API key shows up as `is_active=true` and matching
        # display_name of the human; without get_me, return first match
        # (in single-user workspaces this is fine).
        print(json.dumps({k: m.get(k) for k in ("id", "member", "email", "display_name")}, indent=2))


def cmd_list_states(_args) -> None:
    for s in paginate("/states/"):
        print(f"{s['id']}  {s['group']:10}  {s['name']}")


def cmd_list_cycles(_args) -> None:
    for c in paginate("/cycles/"):
        print(f"{c['id']}  {c['name']}")


def cmd_list_labels(_args) -> None:
    for l in paginate("/labels/"):
        print(f"{l['id']}  {l.get('color',''):8}  {l['name']}")


def cmd_list_issues(args) -> None:
    if args.cycle:
        ids = load_ids()
        cid = get_cycle_id(ids, args.cycle)
        rows = list(paginate(f"/cycles/{cid}/cycle-issues/"))
        for r in rows:
            iid = r.get("issue") or r.get("issue_detail", {}).get("id")
            iss = request("GET", f"/issues/{iid}/")
            print(f"WEFT-{iss['sequence_id']:<5}  {iss['name']}")
        return
    for i in paginate("/issues/"):
        print(f"WEFT-{i['sequence_id']:<5}  {i['name']}")


def cmd_list_cycle(args) -> None:
    args.cycle = args.cycle_name
    cmd_list_issues(args)


def cmd_search(args) -> None:
    q = args.query.lower()
    matches = []
    for i in paginate("/issues/"):
        hay = (i["name"] + " " + (i.get("description_stripped") or "")).lower()
        if q in hay:
            matches.append(i)
    for i in matches:
        print(f"WEFT-{i['sequence_id']:<5}  {i['name']}")
    print(f"-- {len(matches)} match(es)")


def cmd_create_issue(args) -> None:
    ids = load_ids()
    desc_md = ""
    if args.description_md:
        desc_md = Path(args.description_md).read_text()
    elif args.description:
        desc_md = args.description

    payload: dict[str, Any] = {"name": args.name}
    if args.priority:
        payload["priority"] = args.priority
    if args.state_name:
        payload["state"] = get_state_id(ids, args.state_name)
    if args.labels:
        payload["labels"] = get_label_ids(ids, split_csv(args.labels))
    if args.assignee:
        payload["assignees"] = [_resolve_user(ids, args.assignee)]
    if desc_md:
        payload["description_html"] = md_to_html(desc_md)
        payload["description_stripped"] = md_to_stripped(desc_md)

    issue = request("POST", "/issues/", payload)

    if args.cycle:
        cid = get_cycle_id(ids, args.cycle)
        request(
            "POST",
            f"/cycles/{cid}/cycle-issues/",
            {"issues": [issue["id"]]},
        )

    print(json.dumps({
        "sequence_id": f"WEFT-{issue['sequence_id']}",
        "id": issue["id"],
        "name": issue["name"],
        "cycle": args.cycle,
    }, indent=2))


def cmd_add_to_cycle(args) -> None:
    ids = load_ids()
    cid = get_cycle_id(ids, args.cycle_name)
    request(
        "POST",
        f"/cycles/{cid}/cycle-issues/",
        {"issues": list(args.issue_ids)},
    )
    print(f"added {len(args.issue_ids)} issue(s) to cycle {args.cycle_name}")


def cmd_transition(args) -> None:
    ids = load_ids()
    payload: dict[str, Any] = {"state": get_state_id(ids, args.state_name)}
    if args.assignee:
        payload["assignees"] = [_resolve_user(ids, args.assignee)]
    request("PATCH", f"/issues/{args.issue_id}/", payload)
    print(f"transitioned {args.issue_id} → {args.state_name}")


def cmd_defer(args) -> None:
    ids = load_ids()
    new_cid = get_cycle_id(ids, args.new_cycle)
    issue = request("GET", f"/issues/{args.issue_id}/")
    # Find current cycle (if any). Plane returns it under `cycle_id` or via
    # /issues/<id>/cycle-issues; fall back to scanning each cycle.
    cur_cid = issue.get("cycle_id")
    if not cur_cid:
        for cname, cid in ids["cycles"].items():
            for r in paginate(f"/cycles/{cid}/cycle-issues/"):
                iid = r.get("issue") or r.get("issue_detail", {}).get("id")
                if iid == args.issue_id:
                    cur_cid = cid
                    break
            if cur_cid:
                break
    if cur_cid and cur_cid != new_cid:
        try:
            request(
                "DELETE",
                f"/cycles/{cur_cid}/cycle-issues/{args.issue_id}/",
                expect_status=(200, 204, 404),
            )
        except PlaneError as e:
            print(f"warn: removing from old cycle: {e}", file=sys.stderr)
    request(
        "POST",
        f"/cycles/{new_cid}/cycle-issues/",
        {"issues": [args.issue_id]},
    )
    body_md = (
        f"**Deferred to {args.new_cycle} on {time.strftime('%Y-%m-%d')}**\n\n"
        f"**Reason**: {args.reason}\n"
    )
    request(
        "POST",
        f"/issues/{args.issue_id}/comments/",
        {"comment_html": md_to_html(body_md)},
    )
    print(f"deferred {args.issue_id} → {args.new_cycle}")


def cmd_close(args) -> None:
    ids = load_ids()
    body_lines = [
        f"**Closed: {time.strftime('%Y-%m-%d')}**",
        "",
        "**Shipped**",
        args.shipped,
        "",
        "**Commit(s)**",
    ]
    for sha in split_csv(args.commits):
        body_lines.append(f"- `{sha}`")
    body_lines.append("")
    if args.tests:
        body_lines += ["**Tests**", args.tests, ""]
    if args.build:
        body_lines += ["**Build gate**", args.build, ""]
    if args.followups:
        body_lines += ["**Follow-ups spawned**"]
        for f in split_csv(args.followups):
            body_lines.append(f"- {f}")
    body_md = "\n".join(body_lines)

    request(
        "POST",
        f"/issues/{args.issue_id}/comments/",
        {"comment_html": md_to_html(body_md)},
    )
    request(
        "PATCH",
        f"/issues/{args.issue_id}/",
        {"state": get_state_id(ids, "Done")},
    )
    print(f"closed {args.issue_id}")


def cmd_comment(args) -> None:
    body_md = ""
    if args.body_md:
        body_md = Path(args.body_md).read_text()
    elif args.body:
        body_md = args.body
    else:
        raise PlaneError("--body or --body-md required")
    request(
        "POST",
        f"/issues/{args.issue_id}/comments/",
        {"comment_html": md_to_html(body_md)},
    )
    print(f"commented on {args.issue_id}")


def cmd_check(_args) -> None:
    ids = load_ids()
    audit_label = ids["labels"].get("audit-finding")
    fail = 0
    for i in paginate("/issues/?expand=labels,state"):
        labels = i.get("labels") or []
        # `labels` here is a list of UUIDs (string), not objects, on /issues/
        if audit_label and audit_label not in labels:
            continue
        wsslug = next((l for l in labels if isinstance(l, str)), None)
        # We can't dereference label name from UUID here without a cache lookup.
        ws_names = [n for n, uuid in ids["labels"].items() if uuid in labels]
        ws_match = any(n.startswith("ws") for n in ws_names)
        finding_match = any(
            n in {"bug", "stub", "gap", "orphan", "governance",
                   "tech-debt", "docs", "tests", "tooling", "security",
                   "performance"}
            for n in ws_names
        )
        desc = (i.get("description_stripped") or "").lower()
        ac_present = "acceptance criteria" in desc

        problems = []
        if not ws_match:
            problems.append("missing ws-label")
        if not finding_match:
            problems.append("missing finding-type label")
        if not ac_present:
            problems.append("missing AC section")

        if problems:
            fail += 1
            print(f"WEFT-{i['sequence_id']}  {i['name']}: {', '.join(problems)}")
    if fail == 0:
        print("check: clean")
    else:
        print(f"check: {fail} item(s) need attention")
        sys.exit(1)


def cmd_batch_create(args) -> None:
    ids = load_ids()
    items: list[dict] = []
    for f in args.spec_files:
        loaded = json.loads(Path(f).read_text())
        if not isinstance(loaded, list):
            raise PlaneError(f"{f}: expected a JSON list")
        items.extend(loaded)

    print(f"batch-create: {len(items)} item(s) across {len(args.spec_files)} spec file(s)")
    mapping: dict[str, str] = {}
    failures: list[tuple[str, str]] = []

    for i, spec in enumerate(items, start=1):
        name = spec.get("name")
        if not name:
            failures.append(("<no-name>", "missing 'name' field"))
            continue
        if args.dry_run:
            print(f"  [{i:>3}/{len(items)}] DRY would create: {name}")
            continue
        try:
            payload: dict[str, Any] = {"name": name}
            if spec.get("priority"):
                payload["priority"] = spec["priority"]
            payload["state"] = get_state_id(ids, spec.get("state_name", "Todo"))
            if spec.get("labels"):
                payload["labels"] = get_label_ids(ids, spec["labels"])
            if spec.get("description_md"):
                payload["description_html"] = md_to_html(spec["description_md"])
                payload["description_stripped"] = md_to_stripped(spec["description_md"])
            issue = request("POST", "/issues/", payload)
            wnum = f"WEFT-{issue['sequence_id']}"
            mapping[name] = wnum
            if spec.get("cycle"):
                cid = get_cycle_id(ids, spec["cycle"])
                request("POST", f"/cycles/{cid}/cycle-issues/",
                        {"issues": [issue["id"]]})
            print(f"  [{i:>3}/{len(items)}] {wnum} ← {name[:80]}")
        except PlaneError as e:
            failures.append((name, str(e)))
            print(f"  [{i:>3}/{len(items)}] FAIL {name[:60]}: {str(e)[:120]}",
                  file=sys.stderr)

    if args.map_out:
        Path(args.map_out).write_text(json.dumps(mapping, indent=2) + "\n")
    print(f"\nbatch-create: created {len(mapping)} item(s); {len(failures)} failure(s)")
    if failures:
        for n, e in failures[:5]:
            print(f"  FAIL: {n[:80]}\n        {e[:200]}", file=sys.stderr)
        sys.exit(1)


def _resolve_user(ids: dict, who: str) -> str:
    if who != "me":
        return who
    ws = ids["workspace_slug"]
    members = request("GET", f"/abs:/v1/workspaces/{ws}/members/")
    rows = members if isinstance(members, list) else members.get("results", [])
    if not rows:
        raise PlaneError("no workspace members visible to API key")
    return rows[0].get("member") or rows[0].get("id")


# ---------------------------------------------------------------------------
# argparse plumbing
# ---------------------------------------------------------------------------


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(prog="plane.sh", description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)

    sub.add_parser("refresh-ids").set_defaults(func=cmd_refresh_ids)
    sub.add_parser("ensure-labels").set_defaults(func=cmd_ensure_labels)
    sub.add_parser("me").set_defaults(func=cmd_me)
    sub.add_parser("list-states").set_defaults(func=cmd_list_states)
    sub.add_parser("list-cycles").set_defaults(func=cmd_list_cycles)
    sub.add_parser("list-labels").set_defaults(func=cmd_list_labels)

    li = sub.add_parser("list-issues")
    li.add_argument("--cycle")
    li.set_defaults(func=cmd_list_issues)

    lc = sub.add_parser("list-cycle")
    lc.add_argument("cycle_name")
    lc.set_defaults(func=cmd_list_cycle)

    sr = sub.add_parser("search")
    sr.add_argument("query")
    sr.set_defaults(func=cmd_search)

    ci = sub.add_parser("create-issue")
    ci.add_argument("--name", required=True)
    ci.add_argument("--priority", choices=["urgent", "high", "medium", "low", "none"])
    ci.add_argument("--state-name", default="Todo")
    ci.add_argument("--cycle")
    ci.add_argument("--labels")
    ci.add_argument("--assignee")
    ci.add_argument("--description")
    ci.add_argument("--description-md")
    ci.set_defaults(func=cmd_create_issue)

    ac = sub.add_parser("add-to-cycle")
    ac.add_argument("cycle_name")
    ac.add_argument("issue_ids", nargs="+")
    ac.set_defaults(func=cmd_add_to_cycle)

    tr = sub.add_parser("transition")
    tr.add_argument("issue_id")
    tr.add_argument("state_name")
    tr.add_argument("--assignee")
    tr.set_defaults(func=cmd_transition)

    df = sub.add_parser("defer")
    df.add_argument("issue_id")
    df.add_argument("new_cycle")
    df.add_argument("--reason", required=True)
    df.set_defaults(func=cmd_defer)

    cl = sub.add_parser("close")
    cl.add_argument("issue_id")
    cl.add_argument("--shipped", required=True)
    cl.add_argument("--commits", required=True)
    cl.add_argument("--tests", default="")
    cl.add_argument("--build", default="")
    cl.add_argument("--followups", default="")
    cl.set_defaults(func=cmd_close)

    co = sub.add_parser("comment")
    co.add_argument("issue_id")
    co.add_argument("--body")
    co.add_argument("--body-md")
    co.set_defaults(func=cmd_comment)

    sub.add_parser("check").set_defaults(func=cmd_check)

    bc = sub.add_parser("batch-create")
    bc.add_argument("spec_files", nargs="+",
                    help="JSON files, each a list of "
                         "{name, priority, cycle, labels, description_md}")
    bc.add_argument("--dry-run", action="store_true")
    bc.add_argument("--map-out",
                    help="Write a {name → WEFT-N} JSON map to this path.")
    bc.set_defaults(func=cmd_batch_create)

    args = p.parse_args(argv)
    try:
        args.func(args)
    except PlaneError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
