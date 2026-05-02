# vscode-weft-panel (M1)

The WeftOS dev panel for VSCode / Cursor. Hosts the full
`clawft-gui-egui` shell (boot splash, warped-grid desktop, tray, live
kernel status) as a wasm bundle inside a `WebviewPanel`, with RPC
proxied to the daemon through the extension host.

Architecture & rationale:
`.planning/symposiums/compositional-ui/session-7-dev-panel-embedding.md`
and `adrs/adr-011-dev-panel-embedding-hybrid.md`.
Predicate framing: `.planning/symposiums/compositional-ui/foundations.md`.

## What it does

- Registers the command `WeftOS: Open Panel` (`weft.openPanel`).
- Opens a sovereign-posture `WebviewPanel` (editor area,
  `retainContextWhenHidden`, `WebviewPanelSerializer` for reload
  survival).
- Bootstraps the egui surface: loads the wasm bundle + wasm-bindgen
  JS glue from `webview/wasm/`, calls `weft_start("weft-canvas")`.
- Installs `window.__weftPostToHost` so the wasm `live::wasm_live`
  transport can post JSON RPC-request messages up to the extension
  host; extension proxies them to the daemon UDS and posts
  RPC-response messages back via `webview.postMessage` (the default
  `window 'message'` channel the wasm side already listens on).
- Allowlists four methods for the panel's use:
  `kernel.status`, `kernel.ps`, `kernel.services`, `kernel.logs`.

## Build

Two steps — the wasm bundle, then the extension.

```bash
# 1) Build the egui-wasm bundle into webview/wasm/
#    (requires: rustup target add wasm32-unknown-unknown,
#     cargo install wasm-pack)
extensions/vscode-weft-panel/scripts/build-wasm.sh

# 2) Compile the TypeScript extension
cd extensions/vscode-weft-panel
npm install       # first time only
npm run compile   # tsc -p .
# optional: npm run watch
```

The wasm artifacts are `.gitignore`d — rebuild on every source change
via the script above (or `watch` if you wire one up).

## Install (VSCode)

`npm run package` calls `vsce package`. Two notes from the WEFT-289
currency check (2026-04-30):

- **Build the wasm bundle first** (step 1 of Build above). `vsce package`
  globs `webview/wasm/` only if the artifacts are present on disk; an
  empty bundle ships an extension that loads, throws on `init()`, and
  surfaces the "Failed to load the wasm bundle" fallback card.
- This package.json deliberately omits `repository`. Pass
  `--allow-missing-repository` (or run via `npx`) so vsce doesn't
  refuse to package.

```bash
npm install -g @vscode/vsce       # one-time
extensions/vscode-weft-panel/scripts/build-wasm.sh   # from repo root
cd extensions/vscode-weft-panel
vsce package --allow-missing-repository    # vscode-weft-panel-0.0.1.vsix
# or, no-install:
#   npx --yes @vscode/vsce package --allow-missing-repository
code --install-extension vscode-weft-panel-0.0.1.vsix
```

Or, for unpacked iteration: palette → `Developer: Install Extension
from Location…` → point it at this directory.

## Install (Cursor)

Cursor is a VSCode fork and accepts the same `.vsix`:

```bash
cursor --install-extension vscode-weft-panel-0.0.1.vsix
```

Or palette → `Developer: Install Extension from Location…`.

## Use

1. Start the daemon:
   ```bash
   cargo run -p clawft-weave --bin weaver -- kernel start
   ```
2. Palette → **WeftOS: Open Panel**.
3. Watch the boot splash → desktop shell render *inside* Cursor. The
   sidebar pill flips to green within ~1s; Status shows live kernel
   values.

If you see the fallback "Failed to load the wasm bundle" card, step 1
of Build wasn't run. See `SMOKE.md` for the full smoke test and the
known gaps (voice/capture sidecar, typed active-radar channel, WSP
verbs — all deferred to M2/M3).
