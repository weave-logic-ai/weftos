# Session handoff — 2026-04-25

Pick-up doc for the next session. Reflects `phase3-node-identity` at
commit `04df62ca` plus two **uncommitted** post-handoff fixes (extension
RPC allowlist, leaf-carrot recursion guard). Branch is 15 commits ahead
of `master` and pushed to `origin/phase3-node-identity`.

## Post-handoff fixes (uncommitted) — 2026-04-25 evening

The hand-back from the prior session had two visible bugs the user hit
on first re-test:

1. **Toggle button click did nothing** — `control.set_enabled` was
   shipped on the daemon and wired into the explorer's
   `control_toggle.rs`, but never added to the VSCode extension's
   `ALLOWED_METHODS` proxy allowlist
   (`extensions/vscode-weft-panel/src/extension.ts:39`). The viewer
   uses fire-and-forget (`reply: Some(reply_channel().0)` —
   immediately dropped), so the proxy's `"method not allowed"`
   rejection was eaten silently. **Fix:** added `control.set_enabled`
   + `control.list` to the allowlist; `out/extension.js` recompiled.
   Rule of thumb captured in
   `~/.claude/projects/-home-aepod-dev-clawft/memory/feedback_extension_rpc_allowlist.md`:
   any new RPC the WASM panel will fire must also be allowlisted, or
   it dies silently.

2. **Carrot on a leaf node crashed the GUI** — kernel
   `SubstrateService::list(prefix)` deliberately returns
   `[{ path: prefix, has_value: true, child_count: 0 }]` when
   `prefix` is itself a leaf carrying a value, so a caller can ask
   "is this a leaf?" without a separate read. Locked by the kernel
   test `list_leaf_prefix_returns_itself`. The Explorer's
   `render_node` recursed on every child of an expanded prefix —
   so clicking the `▸` on any leaf made `tree_children["<leaf>"]`
   contain itself, and the row rendered itself inside itself
   indefinitely → WASM stack overflow → "the app breaks." **Fix in
   `crates/clawft-gui-egui/src/explorer/tree.rs`:**
   - Suppress the expand caret on leaves
     (`is_leaf = has_value && child_count == 0`). Leaves render
     padded to the same column width with no `▸` glyph.
   - Defensive `if child.path == prefix { continue }` in the
     recursive child loop. Belt-and-suspenders.
   - New parser test
     `parse_list_response_preserves_leaf_self_reference` documents
     that we do NOT filter the kernel's leaf-as-self reply at parse
     time — the recursion guard is in `render_node`. If a future
     refactor moves the filter, the kernel-side leaf-probe semantic
     will break for any external caller.
   Memory at `project_substrate_list_leaf_self.md` captures the
   contract.

Both fixes are tested clean: `scripts/build.sh check`, `clippy`, and
`test` (with `WEFTOS_RUNTIME_DIR=/tmp/nonexistent-weftos-$$` to dodge
the documented `clawft-rpc` no-daemon flake) all green. WASM bundle
rebuilt at 18:03 (`webview/wasm/clawft_gui_egui_bg.wasm`,
5,788,867 b).

User has not yet visually confirmed the toggle flips after webview
reload — that's the first thing to check on resume. If confirmed,
both fixes plus 2026-04-25 daytime work bundle into one commit on
`phase3-node-identity`.

---

## Right this second — what's running

**End-to-end live system, all of it on one daemon + one ESP32:**

- **Daemon** (`weaver kernel start --foreground`) — node-id `n-046780`,
  keyfile at `.weftos/runtime/node.key` (0600). Generated at first
  run; same id across restarts. Listening on
  `unix:.weftos/runtime/kernel.sock` and `tcp:0.0.0.0:9471`.
- **ESP32** (`weftos-mic-node` firmware, INMP441 MEMS mic) — node-id
  `n-bfc4cd`. Calls `node.register` after WiFi up, signs every
  `substrate.publish` over the canonical
  `node_publish_payload(path, value_json, ts, node_id)`. Wire
  shape: alphabetical-keyed JSON `value` (locked in dialog file
  resolution).
- **Whisper service** (`clawft-service-whisper`, in-process tokio
  task) — subscribes to
  `substrate/n-bfc4cd/sensor/mic/pcm_chunk`, windows into 2-s
  buffers, POSTs to `whisper.cpp` HTTP at `127.0.0.1:8123` (model
  `ggml-large-v3-turbo-q5_0.bin`). Publishes transcripts to
  `substrate/n-046780/derived/transcript/n-bfc4cd/mic` via
  `publish_gated`.
- **whisper.cpp server** — separate process at
  `~/llama.cpp/whisper-src/build/bin/whisper-server` started via
  `~/llama.cpp/whisper-server.sh start`.

**Just verified live this session:**

- RMS values flowing at ~1 Hz on `substrate/n-bfc4cd/sensor/mic/rms`
  (current sample: `rms_db=-57.5 peak_db=-45.5` at tick 3049 — quiet
  room).
- Transcripts flowing at ~0.5 Hz on the daemon's transcript path
  (current sample: `text="Okay.\n" window=870000-872000ms` at
  tick 3058).
- Sensor enable/disable cuts pcm_chunk traffic at the ESP32 within
  ~2.5 s (firmware polls control intent every 2 s). Toggled
  off/on, observed substrate tick flat for the off window then
  resume.

**Cross-Claude dialog file** at
`/mnt/c/Users/aepod/OneDrive/Desktop/mentra/docs/clawft-dialog.md`
(=`~/dev/mentra/docs/clawft-dialog.md` from WSL via OneDrive
mount). Append-only, role-tagged. Both kernel and firmware Claudes
read from + post to it. Watch it when iterating wire formats.

### What the user should do on return

1. The daemon and whisper-server should still be up. If not:
   ```bash
   bash ~/llama.cpp/whisper-server.sh start
   RUST_LOG=info nohup ~/.cargo/bin/weaver kernel start --foreground \
       > ~/dev/clawft/.weftos/runtime/kernel.log 2>&1 &
   ```
2. **`Ctrl+Shift+P` → "Developer: Reload Webviews"** in Cursor.
   The WASM bundle was rebuilt at the end of this session (15:37);
   the Cursor webview probably has the old one cached.
3. Open the WeftOS panel. Tree should show:
   ```
   ▾ n-046780 (daemon)
       control/
         services/whisper       — toggle
         sensors/n-bfc4cd/mic/  — toggles for pcm_chunk + rms
       derived/transcript/n-bfc4cd/mic   — live transcript
   ▾ n-bfc4cd (esp32-mic-node)
       sensor/mic/rms           — INMP441 RMS bars
       sensor/mic/pcm_chunk     — clean metadata view (no lockup)
   ```
4. Click `pcm_chunk`. Should render PcmChunkViewer (sample rate,
   channels, ~8000 samples / 500 ms, encoded byte count) — NOT
   the JSON fallback's 21-KB string. If it locks up, the WASM
   reload didn't take.
5. Click any `control/services/whisper` or `control/sensors/...`
   path. Should render the toggle viewer with Enable/Disable
   button. Click to toggle; effect lands within ~2.5 s.

---

## What shipped this session (2026-04-24 evening → 2026-04-25)

### Phase 3 — node identity + write gate (commits 1–8)

All on `phase3-node-identity` branch. Prior session ended with a flat
substrate path scheme (`substrate/sensor/mic` etc.) and unsigned
publishes accepted. This phase landed the node-identity invariant.

- `cfb69319` **NodeRegistry + publish_gated** — kernel infra. Node-id
  is `n-<6-hex>` BLAKE3 prefix of an Ed25519 pubkey; gate enforces
  `path.starts_with("substrate/<node-id>/")` for every write.
  `node_publish_payload(path, value_json, ts, node_id)` is the
  canonical signing layout.
- `21d4e89a` Node-id format finalised (`n-<6-hex>`) per
  `.planning/sensors/JOURNALED-NODE-ESP32.md` §2.2.
- `7fdf696f` **Daemon identity bootstrap** — generates and persists
  ed25519 keypair on first run, registers self at boot.
- `72c115e2` **`node.register` RPC** — proof-of-possession registration
  mirroring `agent.register` but for nodes. Returns deterministic
  `node_id`.
- `9c99ee10` **`substrate.publish` gate flip** — every publish must
  carry `node_id` + `node_signature` + `node_ts`. Hard reject on
  unsigned, cross-node, top-level-flat, unknown-node. Existing
  integration tests grew a `TestNode` helper that registers and
  signs.
- `2216a4d3` **Explorer cleanup** — drops the synthetic
  "substrate/" header row; tree now starts cleanly with node-ids
  as the top level.
- `359b97f4` **Whisper service wired into the daemon** — also
  schema-aligned the in-crate `PcmChunk` deserializer to the
  ESP32's actual emission shape (`{ data, encoding, format,
  sample_rate, channels, samples, start_ts_ms }` — richer than
  the original spec). Old `pcm_b64` / `seq` / `chunk_ms` accepted
  as serde aliases for backward-compat with `publish_wav.rs`.

### Cross-Claude byte-layout debug (dialog-driven)

Firmware Claude shipped flashed firmware with signing logic; every
`substrate.publish` returned `signature verify failed`. They
hypothesised the daemon re-serializes through `serde_json::Value`
and BTreeMap-alphabetises keys. **Confirmed correct** (workspace
doesn't enable `serde_json/preserve_order`). Posted the worked
example with hex bytes; firmware shipped Option A (alphabetical
emit) and signatures verified.

`8c631f14` shipped `substrate.canonical_publish_payload` echo RPC
so the next byte-layout drift is a one-shot diff instead of
three-Claude triangulation. Firmware's `node.identity` request
landed as `c656cd38`.

### Phase 2 of the control plane (commit a8bdc631)

User asked: "we have to be able to enable/disable any sensor from
explore." Strict requirement: cut traffic at the source, not
soft-disable on the consumer.

**Substrate-backed control plane:**

- Path scheme:
  `substrate/<authority-node>/control/{services,sensors}/<target>`.
  Authority writes under its own prefix; subjects subscribe to
  intents from authorities they trust. No gate exception needed.
- `ControlIntent` value: `{ enabled, kind, target, label,
  updated_at_ms }`.
- RPCs: `control.set_enabled { kind, target, enabled, label? }`
  and `control.list`.
- `ControlFlags` registry on the daemon — Arc<AtomicBool> per
  target, shared between RPC handlers and consumer-side enforcement.
- Whisper service grew `service_enabled` + `source_enabled` flags
  in `WhisperServiceConfig`. Both checked in the chunk-receive
  loop. Wiremock `.expect(0)` tests verify zero `/inference`
  calls when either flag is false.
- Initial intents published at boot for: whisper service,
  pcm_chunk sensor, rms sensor.

**Explorer toggle viewer** (`control_toggle.rs`) — shape-matches
control intent, renders Enable/Disable button, fires
`control.set_enabled` on click. Lives outside the
`SubstrateViewer` trait because it needs Live for RPCs (same
pattern as Workshop).

**Firmware contract** posted to dialog: subscribe to
`substrate/<daemon-node>/control/sensors/<own-node>/<sensor-tail>`
after register, stop emit task entirely on `enabled: false`.

**Firmware shipped poll-based subscribe** (2-s cadence — they
explained why subscribe-on-same-socket would need request-response
demux they didn't want to build for human-scale latency). Verified
live: pcm_chunk tick stops advancing within the 2.5-s budget after
toggle-off, resumes cleanly on toggle-on.

`c656cd38` **`node.identity` RPC** — returns the running daemon's
own `{ node_id, label, registered_at }`. Firmware uses this on
first connect to drop the `#define DAEMON_NODE_ID` it had
hardcoded.

### Whisper transcripts proven live

`Yes.` and `Yeah, it seems to get it.` came through during a 30-s
subscribe stream. "Thank you." hallucinations on near-silence are a
known whisper-large-v3-turbo artifact (training data has lots of
YouTube outros). Useful Sensors' Moonshine specifically advertises
not doing this — good candidate when we add a VAD pre-filter Stage.

### Build-id banner (commit 8c631f14)

User suggested putting a build-id at the top of every binary's
output. Added `crates/clawft-weave/build.rs` that captures git
short hash + UTC timestamp. Daemon now prints
`weaver 0.6.19 · git <hash> · built <iso>` as the first stdout
line and the same goes through tracing INFO. `weaver --version`
upgraded to match.

### Explorer pcm_chunk lockup fix (commit 04df62ca)

Last-minute bug: clicking `pcm_chunk` in the tree locked up the
GUI. Root cause: `JsonFallbackViewer::paint_string` cached its
expand state by `(path, length)`. Once expanded once, every
subsequent same-length value (every 500-ms chunk is exactly
21,336 b64 chars) re-laid out a 21-KB monospace galley each
frame, choking the render thread.

Fix:
- Hard cap `STR_INLINE_HARD_MAX = 4096` in `json_fallback.rs`. Above
  that, never inline-render in full — clipped preview + size badge
  + copy-to-clipboard button.
- New dedicated `PcmChunkViewer` at priority 20 — matches the
  firmware's emission envelope and renders metadata only. Decoded
  waveform plot deferred (would need throttled b64 decode).

WASM rebuilt at end of session — 5,788,743 bytes, mtime 15:37.

### Sensor planning docs landed (commit d1ccec83)

Background-agent pass produced `.planning/sensors/`:
- `JOURNALED-SENSOR-MIC.md` — INMP441 as first journaled sensor.
- `JOURNALED-NODE-ESP32.md` — ESP32-S3 as first journaled node.
- `HEALTHCHECK-CONTRACT.md` — generic node + sensor health shape.
- `EXPLORER-MANAGEMENT-SURFACE.md` — proposal for the toggle UI we
  partially shipped this session.

### Pipeline-primitive journal R2 + R3 revisions (commit df51d832)

`.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md` now carries:
- **R2** — Source/Stage/Sink split. Pure stages; identity at the
  Sink; placement axis collapses.
- **R3** — Two-tier path rule (node-private vs mesh-canonical),
  Q1 federation resolved as election (not redundancy), Sink grows
  `pipeline_id` / `process_id` / `target_tier`, governance via
  `DerivedWriteGrant`, audit envelope `(node_id, process_id,
  pipeline_id, ts, signature)`, attest-vs-authenticate (chose
  attest for now), 4 new open questions.

---

## Runtime state

### Daemon

- Binary: `/home/aepod/.cargo/bin/weaver` — installed from
  `phase3-node-identity` tip. Last `cargo install` at the end of
  this session.
- Process: PID `91033` at end of session. Confirm with
  `pgrep -af 'weaver kernel'`.
- Unix socket: `.weftos/runtime/kernel.sock`
- TCP relay: `127.0.0.1:9471` (also bound `0.0.0.0:9471` for ESP32
  LAN access via WSL2 mirrored networking on `192.168.1.73`).
- Log: `.weftos/runtime/kernel.log`. Daemon node-id is `n-046780`.
- Substrate tick: ~3000+ at end of session.

### Whisper-server

- Binary: `/home/aepod/llama.cpp/whisper-src/build/bin/whisper-server`
- Process: PID `14038` at end of session.
- Port: `127.0.0.1:8123`. Health endpoint `GET /health` → `{"status":"ok"}`.
- Model: `ggml-large-v3-turbo-q5_0.bin` (16-kHz mono i16le input).
- Start/stop/status: `~/llama.cpp/whisper-server.sh {start|stop|status}`.

### ESP32 (`weftos-mic-node`)

- Owned by Windows-side firmware Claude (`weftos-mic-node/` repo).
- Node-id `n-bfc4cd`. WiFi-DHCP `192.168.1.178`. Connects to
  `192.168.1.73:9471`.
- Polls control paths every 2 s; respects `enabled: bool`.
- Hardcoded `#define DAEMON_NODE_ID "n-046780"` — firmware Claude
  said next iteration switches to `node.identity` RPC.

### Cursor extension

- Source: `extensions/vscode-weft-panel/`. Latest `out/extension.js`
  picks up `node.register` allowlist (already shipped in prior
  session).
- WASM bundle: `extensions/vscode-weft-panel/webview/wasm/clawft_gui_egui_bg.wasm`
  — rebuilt at 15:37 today. Gitignored; regenerate with
  `extensions/vscode-weft-panel/scripts/build-wasm.sh`.
- After every WASM rebuild: `Ctrl+Shift+P` → "Developer: Reload
  Webviews".

### Branches

- `phase3-node-identity` — tip `04df62ca`. Pushed to origin. 15
  commits ahead of `master`. Open PR URL surfaced by the remote:
  `https://github.com/weave-logic-ai/weftos/pull/new/phase3-node-identity`.
- `master` — unchanged from prior session.
- Remote `origin` is technically still at `clawft.git`; GitHub
  redirected this repo to `weftos.git`. `git remote set-url origin
  git@github.com:weave-logic-ai/weftos.git` cleans up the "this
  repository moved" warning. Not urgent.

---

## Key architectural decisions this session

Read in order for context:

- **`.planning/sensors/PIPELINE-PRIMITIVE-JOURNAL.md` §R3** — node
  vs actor split, two-tier path rule (node-private vs
  mesh-canonical), election-not-redundancy, governance shape, audit
  envelope, attest-vs-authenticate. Most load-bearing for what's
  next.
- **`.planning/sensors/JOURNALED-NODE-ESP32.md`** — node-id format
  spec, key custody (plain NVS for MVP, eFuse later), ESP32 as
  first journaled node.
- **`crates/clawft-kernel/src/node_registry.rs`** — canonical
  signing payloads (`node_register_payload`,
  `node_publish_payload`), `path_belongs_to`, `node_id_from_pubkey`.
- **`crates/clawft-weave/src/control.rs`** — control-plane shape +
  flag registry. Read with the dialog file's
  "[kernel] 2026-04-25 — Sensor enable/disable control plane" entry.
- **`/mnt/c/Users/aepod/OneDrive/Desktop/mentra/docs/clawft-dialog.md`**
  — running cross-Claude transcript. Read before changing any wire
  format.

---

## Open loops carried forward

1. **VAD pre-filter as a Stage** between PcmSource and
   WhisperInference. Per the user's "we would classify the audio
   first and if it is speech deal with it." Eliminates the "Thank
   you." hallucinations on near-silence and saves whisper cycles.
   Useful Sensors' Moonshine is the obvious model; some open VAD
   options (Silero VAD ONNX) also fit.
2. **`clawft-service-llm`** for Qwen3.6-35B at `127.0.0.1:8111`
   (already running via `~/llama.cpp/llama-server`). Same shape as
   the whisper crate: HTTP client, daemon-side tokio task, gated
   publishes. Two design questions still open in the prior
   discussion:
   - Substrate path semantics for prompt/completion (subscribe to
     a prompt queue path, publish to a completion path? or RPC?).
   - Streaming: emit per-token via successive replaces, or just
     final result?
3. **GUI tray-chip mic gauge** still hardcoded to legacy
   `substrate/sensor/mic`. Either (a) walk `substrate.list
   "substrate"` to find any `*/sensor/mic`, (b) call `node.identity`
   to learn the daemon-id and the explorer ESP32 discovery
   protocol, or (c) just retire the chip and point to the Explorer
   panel.
4. **`workshop-watcher` example** annotated as broken-until-migrated.
   Need to make it call `node.register` once + sign every publish.
   Lower priority — it's a dev tool.
5. **Mesh-canonical write gate** (R3.6 in journal). Today
   `substrate/_derived/...` paths can't be written through the gate
   because no node owns the prefix. Capability + eligibility
   branch is sketched in R3.3 but not implemented. Will become
   load-bearing when (a) federated kernel-class nodes appear, or
   (b) we want transcripts at a stable mesh-canonical path
   instead of `substrate/<daemon-node>/derived/...`.
6. **Decoded waveform** in `PcmChunkViewer`. Currently metadata-only.
   Throttled b64 decode + tiny line plot would be useful confidence
   signal. Per-frame decode of 16 KB is the perf concern — needs a
   sample-rate cap.
7. **Other long-string-blob sensors** if any ship that aren't
   pcm_chunk-shaped. The hard cap in `json_fallback.rs` covers
   them generically but a dedicated viewer is always better.
8. **Ed25519 sub-key delegation for processes** — current scheme is
   "attest" (process_id is a label, signed by node key). Authenticate
   (per-process keys with delegation) becomes load-bearing when
   WASM apps share a daemon. R3.8 in journal has the plan.
9. **`substrate.canonical_publish_payload` boot self-check on
   firmware** — firmware Claude said they'll wire a one-shot
   hex-compare against their own buffer at boot. Not blocking;
   nice-to-have for next protocol drift.
10. **PR / merge to master.** Branch is stable, all commits green.
    Open the PR via the URL above when ready, or run `/ultrareview
    phase3-node-identity` first if you want a multi-agent review
    pass.

---

## Useful one-liners

Live transcript stream:

```bash
{ echo '{"id":"sub","method":"substrate.subscribe","params":{"path":"substrate/n-046780/derived/transcript/n-bfc4cd/mic"}}'; sleep 999; } \
  | nc 127.0.0.1 9471
```

Daemon node-identity:

```bash
echo '{"id":"t","method":"node.identity","params":{}}' | nc 127.0.0.1 9471
```

Toggle a sensor off:

```bash
echo '{"id":"t","method":"control.set_enabled","params":{"kind":"sensor","target":"n-bfc4cd/mic/pcm_chunk","enabled":false,"label":"Mic PCM chunks"}}' \
  | nc 127.0.0.1 9471
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

Full phase gate before pushing more:

```bash
scripts/build.sh gate
```

Rebuild the WASM bundle for the Cursor extension:

```bash
bash extensions/vscode-weft-panel/scripts/build-wasm.sh
# then in Cursor: Ctrl+Shift+P → Developer: Reload Webviews
```
