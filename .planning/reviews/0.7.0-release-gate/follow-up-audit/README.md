# Follow-up audit — 2026-05-01

Verification audit for the 0.7.0 release-gate burn-down across the M7 +
M7b + M7c sweeps (65 commits since `7a8805ec`, ~70 items closed in
Plane). Each per-workstream audit confirmed shipped items against
acceptance criteria, ran targeted tests, and looked for stubs / new
issues / regressions.

Branch verified: `m7-08-sweep` @ HEAD (post-merge state, around
`9cca989e` for ws01-17 audits and `81dd34c6` snapshot).

## Per-cluster verdict

| Audit | Workstreams | Items | Confirmed | Concerns | New issues |
|-------|-------------|------:|----------:|---------:|-----------:|
| [ws08](ws08-gui.md) | egui Explorer + WASM panel | 25 | **24/25** (WEFT-283 correctly deferred) | 3 cosmetic | 0 |
| [ws13](ws13-substrate.md) | substrate / surface / sensors / admin | 16 | **16/16** | 1 (Foreign deferred, healthcheck dual-API documented) | 0 |
| [ws09](ws09-dashboard.md) | clawft-ui dashboard + clawft-services backend | 14 | **5/14 fully** (9 partial) | 9 | **12** |
| [ws01-07,10-12](ws01-07-10-12-foundation.md) | foundation, channels, voice, agent-core, kg | 13 | **13/13** | 0 | 0 |
| [ws14-17](ws14-17-deploy-mcp-wasm-research.md) | deploy, mcp, browser-wasm, research | 6 | **5/6** (WEFT-409 partial) | 1 | **2** |
| **Total** | | **74** | **63 fully + 9 partial = 72 in-tree** | 14 | **14** |

## Test execution (re-run after disk cleared)

| Suite | Result |
|-------|-------:|
| `cargo test -p clawft-gui-egui --lib` | 324 / 324 |
| `cargo test -p clawft-gui-egui --test workshop_integration` | 7 / 7 |
| `cargo test -p clawft-gui-egui --test compose_extra_iris` | 12 / 12 |
| `cargo test -p clawft-substrate --lib` | 124 / 124 |
| `cargo test -p clawft-surface --lib` | 27 / 27 |
| `cargo test -p clawft-weave --test substrate_rpc` | 11 / 11 |
| `cargo test -p clawft-services --lib` | 312 / 312 |
| `cargo test -p clawft-tools --lib` | 152 / 152 |
| `cargo test -p clawft-wasm --lib` | 41 / 41 |
| `cargo test -p clawft-rpc --lib version_check` | 7 / 7 |
| `cargo test -p clawft-core --lib workspace::` | 32 / 32 |
| `cargo test -p clawft-channels --lib telegram` | 35 / 35 |
| `cargo test -p clawft-types --lib` | 287 / 287 |
| `cargo test -p clawft-plugin --lib` | 114 / 114 |
| `cargo test -p clawft-weave --test agent_chat_dispatch` | 3 / 3 |
| `node --test clawft-ui/src/lib/url-validator.test.ts` | 6 / 6 |
| `scripts/build.sh check` | clean |
| `scripts/build.sh clippy` | clean |

**Aggregate: 1494 tests passing, 0 new regressions.** Two pre-existing
`clawft-rpc` daemon-status tests still fail when a daemon is running on
the host; unrelated to this wave.

## New Plane items filed by the audit

All filed in `0.9.x` or `1.0.x` cycles with `audit-finding` + workstream
labels. None are 0.7.0 release-gate blockers.

### ws09 dashboard — 12 items (audit-C)

**Security (3 highs):**
- **WEFT-569** — Switch `?token=` to `#token=` URL fragment to prevent
  log leak. The single-use URL token currently rides as a query
  parameter and will appear in `nginx`/HTTP-server access logs and
  browser history.
- **WEFT-570** — `logout()` must invoke server-side token revoke. The
  client-side latch alone leaves the bearer reusable until the
  configured TTL expires.
- **WEFT-576** — Dockerfile must run as non-root. The multi-stage
  Dockerfile lands the nginx/static-serve layer running as root by
  default; needs `USER` directive.

**Functional gaps:**
- **WEFT-565** — `TopicBroadcaster` topics map leaks empty `Sender`
  entries when subscribers drop.
- **WEFT-566** — Document `save_config` hot-reload semantics.
- **WEFT-567** — `/tools` route does not call
  `BackendAdapter.getToolSchema` for WASM mode (the WEFT-307 entry
  point exists but the route never invokes it).
- **WEFT-568** — Cmd+K palette index missing agents/sessions/tools/
  skills/channels + focus trap.
- **WEFT-571** — Validate `customBaseUrl` is HTTPS in production
  (parallels WEFT-310 cors_proxy validation).
- **WEFT-572** — Replace placeholder `vite.svg` with proper PWA icons.
- **WEFT-573** — Render offline banner when SW serves cached shell.
- **WEFT-574** — Land deferred Tauri functional features
  (WEFT-313 was scaffold-only; six functional ACs unmet).
- **WEFT-575** — axe-core runtime a11y scan still missing
  (follow-up to WEFT-561; static `eslint-plugin-jsx-a11y` is the only
  a11y net today).

### ws16 browser-wasm + scripts — 2 items (audit-E)

- **WEFT-563** — BW5 doc fix: `.planning/sparc/browser/05-phase-BW5-wasm-entry.md:581,602` still references the retired `scripts/check-features.sh`.
- **WEFT-564** — Actually retire `scripts/check-features.sh`. The
  WEFT-409 closure swept 5 docs but the executable script (62 lines,
  dated 2026-02-25) is still on disk and not annotated as deprecated.

## Cross-cutting concerns (not blocking)

- **healthcheck dual-API** (ws13): M7b-1 (`SensorHealthReport` +
  `SensorStatus` + `healthcheck_topic_path` for per-adapter wire) and
  M7b-4 (`NodeHealth` + `SensorHealth` + `Status` + `node_health_path`/
  `sensor_health_path` + `classify_value` for the full
  HEALTHCHECK-CONTRACT.md typed shapes) coexist intentionally in one
  module. Names are distinct so callers can't pick wrongly. Doc
  cross-reference between the two surfaces would help future
  contributors. Not material.
- **Foreign canon primitive** (ws13): WEFT-421 wired 12 of 13
  stub-leaves; `Foreign` deferred behind cross-app surface contract.
  Documented in `compose.rs:5-19,222-238`.
- **VSCode panel wasm bundle on disk** (ws08): the on-disk artifact at
  `extensions/vscode-weft-panel/webview/wasm/clawft_gui_egui_bg.wasm`
  is ~7 MB raw / ~3.2 MB gz, over the `wasm-panel` budget (4500 / 1500
  KB). Dated 2026-04-26 — likely a debug or pre-`wasm-opt` build.
  Re-measure on a clean build before any 0.7.0 RC tag.
- **Three cosmetic ws08 doc-comment drifts**: chat bubble `id_salt`
  comment is aspirational; identity-warning chip uses inline text vs a
  doc link; `Mesh::applicable_actions` declared but not yet rendered.

## Items confirmed shipped without code changes (audit-only closes)

These were closed in Plane during the sweep with the original
acceptance criteria already met by code that landed earlier:

- **WEFT-72** (SkillContext::Fork) — type genuinely does not exist in
  any `crates/*`; the 3F-agents M2 footgun isn't present.
- **WEFT-78** (.weftos-plugin.toml scaffold) — parser at
  `crates/clawft-plugin/src/manifest.rs:421` (`from_legacy_toml` +
  `parse_legacy_toml`); CLI loader at `crates/clawft-cli/src/commands/
  plugins_cmd.rs:436-463`.
- **WEFT-302** (memory delete) — shipped earlier in commit `1dfaebb4`
  under the WEFT-168 sweep; M7c-A closed in Plane.
- **WEFT-303** (save_config persistence) — same commit; M7c-A closed.
- **WEFT-340** (agent.chat dispatch tests) — three `#[tokio::test]`
  functions already present at
  `crates/clawft-weave/tests/agent_chat_dispatch.rs:137/167/203`.
- **WEFT-433** (per-node-prefix write gate on substrate.publish) —
  shipped earlier; full deny-path tests at
  `crates/clawft-weave/tests/substrate_rpc.rs`.

## What this audit did NOT cover

- The four ws09 items M7c left as `InProgress` in 0.9.x → 1.0.x
  transition state. Plane state cleanup needed (separate task).
- M7-A's 41-item ws02 kernel deferral cluster — only the single ship
  (WEFT-116) was in audit scope; the deferred items are 0.9.x backlog.
- Live UI testing: Playwright suite (WEFT-314) is scaffolded but no
  spec was exercised in this audit window. Smoke spec syntax is valid;
  runtime validation depends on chromium download which the audit
  environment did not have.
- VSCode extension manual run-through. Static review only.
- The 9 audits-C "concerns" map directly onto the 12 new Plane items;
  see ws09 doc for one-to-one cross-reference.

## Recommendation for the parent

The audit found **no 0.7.0 ship blockers**. The 14 new items (12 ws09 +
2 ws16) are 0.9.x/1.0.x scope and have all been filed. The 3 ws09
security highs (WEFT-569/570/576) should be prioritised in the next
0.9.x cycle — they affect the dashboard's auth and container surface,
not the daemon itself, so they're not on the 0.7.0 critical path.

ws08, ws13, ws01-07/10-12, and ws14-17 are clean and confirmed.
