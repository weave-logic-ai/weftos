# Session handoff — 2026-04-26

Pick-up doc for the next session. Reflects `phase3-node-identity` at
commit `6ebc8ad5`. Branch is **23 commits ahead** of `master`. All
green: `scripts/build.sh check` + `clippy` clean; targeted tests pass
across every touched crate (15 + 7 + 15 + 34 + 215 + 41 = 327 unit
tests + integration). The full-workspace `cargo test --workspace`
deadlocks against any live daemon's runtime dir — this is a
documented flake, NOT a regression. Run targeted tests instead:

```bash
cargo test -p clawft-service-llm \
          -p clawft-service-terminal \
          -p clawft-service-classify \
          -p clawft-service-whisper \
          -p clawft-gui-egui \
          -p clawft-weave --lib
```

This session was a single big push: the LLM connect, then a
five-agent parallel wave that landed chat / terminal / VAD-classifier
/ mesh-canonical write gate / GUI cleanups simultaneously.
**Merge-to-master is now within reach.** The user's gating rule was
"kernel + mesh + chain + explorer all work well, plus terminal and
clawft chat window" — every item on that list is in the tree. The
remaining holdouts are the live-verify and a doc/UX polish pass; see
"Open loops" at the bottom.

---

## What's new this session

### LLM connect (commit `a05e22ac`)

The daemon now talks to a locally-hosted llama-server and exposes a
single synchronous chat-completions RPC.

- New crate **`clawft-service-llm`** — mirrors `clawft-service-whisper`
  exactly (in-flight semaphore=1, health probe + retry-or-not error
  taxonomy, wiremock unit tests). Posts to OpenAI-compat
  `/v1/chat/completions`. 15/15 unit tests green.
- **Why a new crate, not `clawft-llm`** — the existing crate is the
  general provider abstraction (OpenAI/Anthropic/native + routing +
  failover + SSE), targets browser+native, brings in `clawft-types`
  + `eml-core` + `uuid` + a futures stack. For the daemon's
  "POST one prompt to a single localhost endpoint" use case that
  surface is overkill and the dependency edge would couple the
  daemon-only HTTP wrapper to a browser-targeted abstraction. Crate
  doc at the top of `lib.rs` explains.
- **Daemon wiring** — `DAEMON_LLM` (`OnceLock<Arc<LlmClient>>`),
  background tokio task probes `/health` at boot,
  `control.set_enabled {target:"llm"}` source-cuts the call, initial
  control intent published under `substrate/<daemon-node>/control/services/llm`.
- **Protocol** — `LlmPromptParams { prompt | messages | system |
  temperature | max_tokens }` + `LlmPromptResult { completion |
  finish_reason | prompt_tokens | completion_tokens | model }`.
  Streaming is **deliberately deferred** — when the chat window grows
  a per-token UI, it lands as `llm.prompt_stream` mirroring
  `substrate.subscribe`'s connection-takeover pattern, NOT as a
  breaking change.
- **Verified live** at `127.0.0.1:8111` against
  `Qwen3.6-35B-A3B-UD-IQ2_XXS.gguf`. `llm.prompt {prompt:"Reply with
  exactly: hello clawft"}` returns the expected completion;
  `control.set_enabled {target:"llm", enabled:false}` cuts subsequent
  calls before any HTTP hit.

Memory: `~/.claude/projects/-home-aepod-dev-clawft/memory/project_llm_service_shipped.md`

### Chat window panel (commit `e23807fb` → merged in `b96d413a`)

Egui chat surface that talks to `llm.prompt`.

- New module `crates/clawft-gui-egui/src/explorer/chat.rs`. Holds
  `ChatView { history, draft, in_flight }` across paints.
- Daemon publishes `substrate/<daemon-node>/ui/chat` with
  `{ "kind": "chat", "model": "<llama-server model>" }` next to the
  initial control intents. Panel matches `kind == "chat"` at
  priority 40 (wins over Workshop=30 and control_toggle=25).
- Existing `Live::Command::Raw { reply }` + `try_recv_reply` already
  return `Result<Value, String>` — the chat panel reuses the
  Workshop drain pattern; no new RPC plumbing needed.
- `ChatMessage` is local to the GUI crate (not re-exported from
  weave) so gui-egui doesn't pull a tokio-server edge.
- `Explorer::on_select` and `Explorer::close` reset the chat view
  so a hidden panel can't keep an in-flight `llm.prompt` against
  llama-server's single-batch slot.
- Scope cuts (deferred): no streaming, no markdown, no system-prompt
  UI, no model picker, no on-disk persistence.

### Terminal pane (commit `a509cd14` → merged in `ced776bd`)

Daemon-side PTY service, surfaced as an explorer panel.

- New crate **`clawft-service-terminal`** wrapping `portable-pty 0.8`.
  Public `TerminalManager` over `DashMap<SessionId, Arc<TerminalSession>>`.
  Auto-detects shell (`requested → $SHELL → /bin/bash → /bin/sh`),
  sets `TERM=xterm-256color`, **runs the blocking reader on a
  dedicated OS thread** (a tokio worker would stall on
  `std::io::Read`).
- Four RPCs: `terminal.spawn { rows, cols, shell?, cwd? }` →
  `{ session_id, rows, cols, shell }`, `terminal.write { id, data
  /*base64*/ }`, `terminal.resize`, `terminal.close`.
- Output: each PTY chunk is base64-encoded and published to
  `substrate/<daemon-node>/derived/terminal/<session_id>` as
  `{ data, ts_ms, exit }`. The egui panel polls the path every
  250 ms (dedupes by tick), base64-decodes UTF-8-lossy into a
  sticky-bottom `ScrollArea`.
- Sentinel: `substrate/<daemon-node>/ui/terminal` with `{ "kind":
  "terminal" }`. **Convention** locked in by both this and the chat
  panel: `substrate/<daemon-node>/ui/<name>` with at minimum
  `{ "kind": "<name>" }` is how new top-level UI surfaces declare
  themselves.
- `Explorer::on_select` / `close` now take `&Arc<Live>` so they can
  fire `terminal.close` on navigate-away.
- Sharp edges flagged for follow-up:
  - **ANSI escapes render as literal `\u{1b}[...`** — `vte` parser is
    the next iteration.
  - **Output dedup is by `tick`**, not append-only-log. Two chunks
    with identical bytes between two polls collapse into one. Fix is
    a per-session output sub-stream path or tick-bumping append.
  - **`take_events()` is single-consumer** — a chat agent and a panel
    can't both watch the same session yet. Daemon-side drain is the
    only consumer today; fan-out is the next change.
  - **`Drop` on `Terminal` can't fire `terminal.close`** (no `Live`
    handle). Explorer's explicit `close()` and `on_select()` cover
    happy paths; if neither runs (process kill), the daemon-side
    session lives until daemon shutdown. Acceptable since no
    background traffic.
  - **No SIGWINCH handshake** — we just call `MasterPty::resize`
    (ioctl). In-shell apps see it on next read.

### Audio-classifier Stage (commit `dbf521f8` → merged in `be8366d3`)

The Stage between PcmSource and WhisperInference per
`.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md` §R2.

- New crate **`clawft-service-classify`** with `ClassifierBackend`
  trait — the seam for the future llama.cpp-hosted classifier.
- Initial backend `EnergyClassifier` is RMS-dBFS threshold (default
  `-45 dB`, override via `VAD_RMS_THRESHOLD_DB`); emits
  `Classification { class: String, confidence, rms_db, sample_rate,
  samples, ts_ms, source_node, source_seq }`. **`class` is a String
  on purpose** — a future classifier emits `"music"` / `"noise"` /
  `"speech"` / etc. without breaking subscribers (per the
  `project_vad_classifier_via_llamacpp.md` memory).
- `ClassifierService::spawn` mirrors `WhisperService::spawn`
  (subscribe → decode b64→i16le → backend → `publish_gated`) with
  the same `service_enabled` / `source_enabled` `Arc<AtomicBool>`
  flags.
- **Whisper is now gated on classifier output via substrate, not via
  a Rust edge.** New `WhisperServiceConfig` fields `classifier_input:
  Option<String>` + `gate_window_ms: u64` (default `1500`, ≈ two
  pcm_chunk periods at the firmware's 2 Hz cadence — long enough to
  bridge inter-syllabic pauses, short enough that sustained quiet
  closes the gate). The chunk-receive arm checks an internal
  `is_speech` flag updated by a background subscriber task.
- 15 classify-crate tests + 4 new whisper tests
  (skip-on-silence / process-on-speech / stickiness / wire-pin).

Memory: `~/.claude/projects/-home-aepod-dev-clawft/memory/project_vad_classifier_via_llamacpp.md`

Surprise the agent flagged: the existing whisper service runs
everything in one fat `tokio::select!` rather than the R2
Source/Stage/Sink split, so adding the gate was a tactical patch
rather than a slot in a clean Stage layer. The journal's "small
refactor" estimate is real and waiting.

### Mesh-canonical write gate, R3.6 (commit `8be9e70d`)

Daemon-class nodes can now write `substrate/_derived/...` under an
explicit grant.

- `DerivedWriteGrant { grantee_node_id, topic, issued_at_ms, scope }`
  with `GrantScope::{ExactTopic, TopicPrefix}`. Stored in a sibling
  `DashMap<(grantee, topic), grant>` on `NodeRegistry`.
- New method **`SubstrateService::publish_gated_with_grants(node,
  path, value, &NodeRegistry)`** adds the tier split. Mesh-canonical
  paths (`substrate/_derived/...`) consult the grant table; everything
  else falls through to the existing per-node-prefix rule. Legacy
  `publish_gated` keeps strict semantics so unmigrated callers still
  reject `_derived/` writes — opt-in by switching to the new method.
- Daemon issues itself `transcript` / `classify` / `terminal` grants
  right after node-identity bootstrap.
- **Whisper dual-publish migration** — publishes to BOTH
  `substrate/_derived/transcript/<src>/mic` (canonical, R3.2) AND the
  legacy `substrate/<daemon>/derived/transcript/<src>/mic`. Legacy
  publish wrapped in `// REMOVE AFTER PHASE 4` and tracing target
  `"deprecated"`. Subscribers in this repo all discover via
  `substrate.list/read` walks rather than hardcoded subscriptions, so
  the migration risk is limited to operator-driven `substrate.subscribe`
  invocations against the old path — none of which live in this tree.
- 9 new node_registry tests + 7 new substrate_service tests + 3 new
  integration tests in `crates/clawft-weave/tests/derived_grant_gate.rs`.

Open follow-ups (R3.6 explicitly out of scope):
- No multi-node grant federation — single daemon issues to itself.
- No grant revocation API — grants permanent for daemon lifetime.
- No transparent migration of `substrate.subscribe` paths.

### Disable-device toggle bug fixes (commit `8bc6fb23`)

Two post-handoff bugs from the prior session that prevented the
sensor toggle from working:

1. **Extension RPC allowlist gap** — `control.set_enabled` /
   `control.list` shipped on the daemon and wired into the toggle
   viewer, but never added to `extensions/vscode-weft-panel/src/extension.ts`'s
   `ALLOWED_METHODS`. Toggle viewer fires fire-and-forget, so the
   proxy reject was eaten silently. Fix: 2 lines added to allowlist.
2. **Leaf-node carrot recursion → WASM stack overflow** — kernel's
   `substrate.list` deliberately returns `[{path:prefix,
   has_value:true, child_count:0}]` when prefix is itself a value-
   carrying leaf (so callers can leaf-probe without a separate
   read). The Explorer's `render_node` recursed on every child of an
   expanded prefix; clicking the ▸ on a leaf put the leaf into
   `ex.expanded` and the next list-response populated
   `tree_children["<leaf>"]` with a single entry whose path was the
   leaf itself — infinite recursion → WASM stack overflow. Fix:
   suppress caret on leaves (`is_leaf = has_value && child_count ==
   0`) + defensive `if child.path == prefix { continue }` belt-
   and-suspenders. New parser test asserts we do NOT filter the
   kernel's leaf-as-self response at parse time — the recursion guard
   lives in `render_node`, not in the parser, because filtering at
   the parser would silently break any external caller that uses
   `substrate.list` as a leaf probe.

Memories:
- `~/.claude/projects/-home-aepod-dev-clawft/memory/feedback_extension_rpc_allowlist.md`
- `~/.claude/projects/-home-aepod-dev-clawft/memory/project_substrate_list_leaf_self.md`

### GUI cleanups bundle (commits `5a067eef`, `5a194c8b`, `551d25a2` → merged in `29702177`)

- **Mic tray-chip gauge migration.** New `crates/clawft-gui-egui/src/live/mic_discovery.rs`
  walks a `substrate.list` response and returns the first child path
  ending in `/sensor/mic/rms`. Native driver runs discovery once per
  (re)connect. `None` → chip dimmed "no mic." 7 unit tests including
  a defensive check that the legacy flat path does NOT match.
- **Decoded waveform mini-plot in `PcmChunkViewer`.** 40 px painter
  line, full row width, normalized [-1, 1]. Decode-time decimation
  caps at 60 points; `MIN_DECODE_INTERVAL_MS = 250` rate-limits
  decode to ≤4 Hz. Cache keyed by `(path, start_ts_ms)` in egui
  memory so repaints between decodes are free. 7 new viewer tests.
- **Workshop-watcher example migration.** `crates/clawft-gui-egui/examples/workshop-watcher.rs`
  was broken since the Phase 3 gate flip (publishes were unsigned).
  Inline `LocalNode` (adapted from `clawft-weave/tests/substrate_rpc.rs::TestNode`)
  generates ephemeral keypair, registers, signs every publish over
  `node_publish_payload`. Default publish path now
  `substrate/<this-example's-node-id>/ui/workshop/<name>`. One-shot
  `substrate.canonical_publish_payload` self-check at boot.
  **Live verification deferred** — example builds clean, runtime
  test against a daemon is the next step.

---

## Right this second — what's running

**Daemon:** old binary (PID 78737, started 2026-04-25 21:37 from
commit `a05e22ac`). Still listening on
`unix:.weftos/runtime/kernel.sock` and `tcp:0.0.0.0:9471`. Whisper
service still publishing transcripts; ESP32 `n-bfc4cd` still
connected. Node-id `n-046780`.

**The merged tree carries new daemon RPCs (`llm.prompt`,
`terminal.*`) and a new daemon-side service (classify) that the
running binary doesn't know about.** Restart the daemon to pick them
up — see the "On return" block below.

**Whisper-server:** `/home/aepod/llama.cpp/whisper-src/build/bin/whisper-server`
at `127.0.0.1:8123` (Qwen3 STT). Health endpoint `GET /health`.
Start/stop via `~/llama.cpp/whisper-server.sh`.

**llama-server (LLM):** `127.0.0.1:8111` running
`Qwen3.6-35B-A3B-UD-IQ2_XXS.gguf`. The new daemon will probe this on
boot and log `llm service: healthy url=http://127.0.0.1:8111` if it
answers.

**ESP32 (`weftos-mic-node`):** node-id `n-bfc4cd`, WiFi-DHCP
`192.168.1.178`. Polls control paths every 2 s. Will reconnect
automatically when the daemon comes back up.

**Cross-Claude dialog file:**
`/mnt/c/Users/aepod/OneDrive/Desktop/mentra/docs/clawft-dialog.md`
(=`~/dev/mentra/docs/clawft-dialog.md` from WSL). Append-only,
role-tagged. Read it before changing any wire format.

### What the user should do on return

1. **Confirm fresh artifacts.**
   ```bash
   ls -la ~/.cargo/bin/weaver ~/dev/clawft/target/release/weaver
   ls -la ~/dev/clawft/extensions/vscode-weft-panel/webview/wasm/clawft_gui_egui_bg.wasm
   ```
   The 2026-04-26 session ends with both built fresh against `6ebc8ad5`.
2. **The daemon was restarted at end-of-session.** If it isn't up:
   ```bash
   bash ~/llama.cpp/whisper-server.sh start
   cd ~/dev/clawft && RUST_LOG=info nohup ~/.cargo/bin/weaver kernel start --foreground \
       > ~/dev/clawft/.weftos/runtime/kernel.log 2>&1 &
   ```
3. **`Ctrl+Shift+P` → "Developer: Reload Webviews"** in Cursor —
   the WASM bundle was rebuilt at 23:05.
4. Open the WeftOS panel. Tree should show:
   ```
   ▾ n-046780 (daemon)
       control/
         services/whisper       — toggle
         services/llm           — toggle           ← new
         services/classify      — toggle           ← new
         sensors/n-bfc4cd/mic/  — toggles
       derived/transcript/n-bfc4cd/mic    — live transcript (legacy path, dual-published)
       derived/classify/n-bfc4cd/mic      — live classification ← new
       derived/terminal/<sid>             — appears once a session is spawned
       ui/chat                ← new sentinel; click to open chat panel
       ui/terminal            ← new sentinel; click to open terminal panel
   ▾ _derived/                                    ← new mesh-canonical tier
       transcript/n-bfc4cd/mic  — live transcript (canonical path, R3.2)
   ▾ n-bfc4cd (esp32-mic-node)
       sensor/mic/rms / pcm_chunk
   ```
5. **Try the chat window:** click `ui/chat`. Type a message, hit
   Enter. Should round-trip through `llm.prompt`. Qwen3 burns most
   tokens on its `<think>` block, so set max_tokens generously
   (default 512 in the daemon config; not exposed in UI yet).
6. **Try the terminal:** click `ui/terminal`. Should auto-spawn a
   shell (sees `$SHELL` first, falls back to bash). Type `echo hi`,
   press Enter, see output. ANSI sequences render literal — that's
   the documented vte-parser follow-up.
7. **Click `pcm_chunk`:** should render PcmChunkViewer with the new
   waveform mini-plot underneath the metadata.

---

## Runtime state

### Daemon binary

- Binary: `/home/aepod/.cargo/bin/weaver` — installed from
  `phase3-node-identity` tip `6ebc8ad5`. Last `cargo install` at
  end-of-session.
- Process: PID was `78737` early in session, restarted at
  end-of-session against the new binary.
- Unix socket: `.weftos/runtime/kernel.sock`
- TCP relay: `127.0.0.1:9471` (also `0.0.0.0:9471` for ESP32 LAN
  via WSL2 mirrored networking on `192.168.1.73`).
- Log: `.weftos/runtime/kernel.log`. Daemon node-id is `n-046780`.

### Branches

- `phase3-node-identity` — tip `6ebc8ad5`. Pushed to origin (need
  `git push` if you want it on the remote — local-only at end of
  session). 23 commits ahead of `master`.
- All five worktrees (`agent-aa797dec06afad46f`,
  `agent-a282fd684e8be096d`, `agent-a449c31cd9ac44da4`,
  `agent-a776be7c32eaa4fcd`, plus the unused
  `agent-af74d2cb05c70b68b`) cleaned up. Stale branches deleted.

### llama-server

- Binary: `/home/aepod/llama.cpp/build/bin/llama-server`
- Port: `127.0.0.1:8111`
- Model: `Qwen3.6-35B-A3B-UD-IQ2_XXS.gguf`
- Health: `GET http://127.0.0.1:8111/health` → `{"status":"ok"}`
- The daemon's `clawft-service-llm` boot probe will log this status
  line.

---

## Architectural decisions this session (read in order for context)

- **`crates/clawft-service-llm/src/lib.rs`** — top-of-file rationale
  for why this is a separate crate from `clawft-llm`.
- **`crates/clawft-service-classify/src/`** — `ClassifierBackend`
  trait shape. Future llama.cpp-hosted classifier swaps in by
  implementing this trait against a HTTP endpoint; subscribers don't
  change.
- **`crates/clawft-kernel/src/node_registry.rs`** — `DerivedWriteGrant`
  + `GrantScope` types, `issue_derived_grant`,
  `has_derived_grant`. Sibling table to `NodeRegistration` per
  R3.6's "separate permission" mandate.
- **`crates/clawft-kernel/src/substrate_service.rs`** —
  `publish_gated_with_grants` is the new opt-in surface. Old
  `publish_gated` kept strict for unmigrated callers.
- **`crates/clawft-service-terminal/src/session.rs`** — PTY reader
  on dedicated OS thread, sentinel convention
  `substrate/<daemon-node>/ui/<name>`.
- **`crates/clawft-gui-egui/src/explorer/{chat,terminal}.rs`** — the
  two new non-trait viewer modules. Both register at the same
  dispatch arm in `mod.rs:paint_detail` next to `control_toggle`
  and `workshop`.
- **`.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md` §R3.6** — the
  governance design doc for what we just shipped. Read alongside
  R3.0 (two-tier path rule) + R3.3 (Sink gate splits by tier).

---

## Open loops carried forward

Higher-priority follow-ups first.

1. **Live verify the new surfaces.** First boot of the new daemon
   binary. Confirm:
   - `llm service: healthy` line appears in kernel.log
   - `ui/chat` panel round-trips a prompt
   - `ui/terminal` spawns a shell and round-trips `echo hi`
   - `_derived/transcript/...` shows live transcripts
   - `derived/classify/...` shows speech/silence transitions
   - Old `<daemon>/derived/transcript/...` still receives the
     dual-publish (will go away after Phase 4)
2. **PR / merge to master.** User's gating list (kernel, mesh,
   chain, explorer, terminal, chat) is now complete in-tree.
   Open the PR via
   `https://github.com/weave-logic-ai/weftos/pull/new/phase3-node-identity`
   — or run `/ultrareview phase3-node-identity` first for a
   multi-agent review pass.
3. **Drop the legacy whisper publish.** When the dual-publish
   window ends, delete the `output_path_legacy` field + the
   `// REMOVE AFTER PHASE 4` block in
   `crates/clawft-service-whisper/src/service.rs`, and update the
   Explorer's transcript discovery to look under `_derived/...`
   only. No external subscribers in this tree depend on the legacy
   path; only operator-driven `substrate.subscribe` calls.
4. **`vte` parser for the terminal panel.** Today ANSI escape
   sequences render as literal `\u{1b}[...`. Adding the `vte` crate
   to the egui terminal would render colors + cursor moves. Keep
   the panel itself thin; the parser is the only thing that needs
   to grow.
5. **Streaming `llm.prompt_stream`.** When the chat window grows a
   per-token UI, the streaming sibling lands as a connection-takeover
   RPC mirroring `substrate.subscribe`'s pattern. The
   `llm.prompt` shape is already designed not to break. Markdown
   rendering in the chat panel is the obvious cosmetic follow-up.
6. **Real audio classifier.** `EnergyClassifier` is the floor.
   Silero VAD ONNX is the obvious next step (one more
   `ClassifierBackend` impl); a llama.cpp-hosted multi-class
   classifier comes after that and unlocks `"music"` / `"noise"` /
   `"speech"` distinctions per the saved memory. Trait shape
   already accommodates both.
7. **Whisper service Source/Stage/Sink refactor.** The classifier
   gate landed as a tactical patch in the existing
   `tokio::select!`; the journal's R2 split is still waiting.
   Worth doing before adding any more pipeline stages.
8. **Terminal output dedup as append-log** rather than tick. Two
   identical chunks between polls collapse today. Per-session
   sub-stream substrate path or tick-bumping append.
9. **Multi-surface terminal attach.** `take_events()` is
   single-consumer today; a fan-out broadcast channel is the
   change. Becomes load-bearing when a chat agent + a panel both
   want to watch the same session.
10. **Multi-node grant federation** (R3.6 deferred). Today only the
    daemon-class node issues grants to itself. When federated
    kernel-class nodes appear, grant issuance becomes a
    cross-signature flow. R3.8's authenticate path is also waiting.
11. **Mic gauge live-update.** Discovery is single-shot per connect;
    new mic-bearing nodes joining mid-session won't appear until the
    daemon connection cycles. Cheap fix: rerun discovery on every
    slow tick that returns no transcript.
12. **Pre-existing clippy warnings** in
    `crates/clawft-gui-egui/src/explorer/viewers/{pcm_chunk,
    time_series}.rs` and `control_toggle.rs` test modules
    (`assertions_on_constants`, `approx_constant`). Easy sweep,
    out-of-scope for this session.
13. **Workshop-watcher live verify.** Builds clean against the new
    gate; runtime test against a running daemon is the only thing
    left. Promote past "developer tool" once that's done.
14. **Sub-key delegation for processes** (R3.8). Current scheme is
    "attest" (process_id is a label, signed by node key).
    Authenticate (per-process keys with delegation) becomes
    load-bearing when WASM apps share a daemon.
15. **`substrate.canonical_publish_payload` boot self-check on
    firmware.** Firmware Claude said they'll wire this; not
    blocking. Nice-to-have for next protocol drift.

---

## Useful one-liners

Live transcript stream (canonical path):

```bash
{ echo '{"id":"sub","method":"substrate.subscribe","params":{"path":"substrate/_derived/transcript/n-bfc4cd/mic"}}'; sleep 999; } \
  | nc 127.0.0.1 9471
```

Live classifier stream:

```bash
{ echo '{"id":"sub","method":"substrate.subscribe","params":{"path":"substrate/n-046780/derived/classify/n-bfc4cd/mic"}}'; sleep 999; } \
  | nc 127.0.0.1 9471
```

LLM prompt:

```bash
echo '{"id":"t","method":"llm.prompt","params":{"prompt":"hello clawft","max_tokens":400}}' \
  | nc -q 60 127.0.0.1 9471
```

Spawn a terminal session (CLI test of the daemon-side service):

```bash
echo '{"id":"t","method":"terminal.spawn","params":{"rows":24,"cols":80}}' | nc -q 5 127.0.0.1 9471
```

Toggle a service off:

```bash
echo '{"id":"t","method":"control.set_enabled","params":{"kind":"service","target":"llm","enabled":false}}' \
  | nc 127.0.0.1 9471
```

Daemon node-identity:

```bash
echo '{"id":"t","method":"node.identity","params":{}}' | nc 127.0.0.1 9471
```

Verify a publish payload (canonical bytes the verifier sees):

```bash
echo '{"id":"t","method":"substrate.canonical_publish_payload","params":{"path":"substrate/n-bfc4cd/sensor/mic/rms","value":{"rms_db":-26.4,"peak_db":-12.1,"sample_rate":16000,"available":true,"samples_in_window":16000,"characterization":"Rate"},"node_id":"n-bfc4cd","node_ts":12345}}' \
  | nc 127.0.0.1 9471
```

Walk the substrate tree from the daemon root:

```bash
echo '{"id":"t","method":"substrate.list","params":{"prefix":"substrate","depth":4}}' \
  | nc 127.0.0.1 9471 | python3 -m json.tool
```

Targeted test sweep (this is the green-build benchmark — full
workspace test deadlocks against any live daemon's runtime dir):

```bash
cargo test -p clawft-service-llm \
          -p clawft-service-terminal \
          -p clawft-service-classify \
          -p clawft-service-whisper \
          -p clawft-gui-egui \
          -p clawft-weave --lib
```

Full phase gate before pushing more:

```bash
scripts/build.sh gate
```

Rebuild the WASM bundle for the Cursor extension:

```bash
bash extensions/vscode-weft-panel/scripts/build-wasm.sh
# then in Cursor: Ctrl+Shift+P → Developer: Reload Webviews
```

Install the fresh daemon binary (requires daemon to be stopped first
— text-busy otherwise):

```bash
pkill -TERM -f 'weaver kernel start' && sleep 2
cp ~/dev/clawft/target/release/weaver ~/.cargo/bin/weaver
RUST_LOG=info nohup ~/.cargo/bin/weaver kernel start --foreground \
    > ~/dev/clawft/.weftos/runtime/kernel.log 2>&1 &
```
