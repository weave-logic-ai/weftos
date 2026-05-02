// WeftOS dev-panel extension.
//
// M0: static HTML + two sample RPC buttons. M1 (this file): replaces
// the static UI with the egui surface compiled to wasm, loaded into
// the webview, and wired to the daemon through a typed postMessage
// bridge.
//
// Scope references:
//   .planning/symposiums/compositional-ui/session-7-dev-panel-embedding.md
//   .planning/symposiums/compositional-ui/adrs/adr-011-dev-panel-embedding-hybrid.md
//
// Out of scope for M1: WSP-0.1 verbs (raw kernel.* RPC only),
// voice/capture sidecar (M2), workspace editor topics (M2),
// active-radar typed return channel (M2).

import * as vscode from "vscode";
import { randomUUID } from "node:crypto";
import { watch as fsWatch, type FSWatcher } from "node:fs";
import { resolveSocketPath, rpcCall, RpcError } from "./rpc";

const VIEW_TYPE = "weft.panel";

// Allowed RPC methods the extension will proxy from the webview.
//
// **WEFT-250**: This list used to drift every time the daemon added
// an RPC. The proxy now auto-fetches the daemon's method list on
// connect (`daemon.list_methods` / `kernel.list_methods` — the first
// one the daemon answers wins) and merges the response into this
// static seed. The static set is kept as a *fallback* for the
// daemon-doesn't-yet-support-introspection case AND as the documented
// minimum surface that the panel relies on. Methods returned by the
// daemon are intersected with the runtime allowlist before being
// proxied; so the daemon's authority gate is still the real
// enforcement, the proxy just refuses to forward anything it has
// never been told about.
//
// History:
//   - M1: four read methods the wasm `Live` polls.
//   - M1.5.1a: two built-in admin-app write verbs (ADR-015).
//   - M1.5.1d: cluster.* + chain.* projections.
//   - M1.5.2 reserved `sensor.mic.status` for the audio bridge.
//   - M1.5.3: substrate.read / substrate.subscribe / substrate.list
//     so the WASM Live loop drives Snapshot through these verbs.
//   - control.set_enabled / control.list (Phase 3 control plane).
//   - llm.prompt + agent.chat for the chat panel.
//   - terminal.* for PTY-backed shell sessions.
const STATIC_ALLOWED_METHODS = new Set<string>([
    "kernel.status",
    "kernel.ps",
    "kernel.services",
    "kernel.logs",
    "kernel.kill-process",
    "kernel.restart-service",
    "cluster.status",
    "cluster.nodes",
    "chain.status",
    "chain.tail",
    "sensor.mic.status",
    // Ontology Explorer Phase 0 (2026-04-23): the WASM `Live` loop inside
    // the webview drives `Snapshot` through these two verbs — same code
    // the native GUI runs. Without them the tray chip icons never go
    // green and the mic gauge bound to $substrate/sensor/mic.rms_db
    // never sees bridge-published values. `substrate.publish` stays
    // blocked; the webview is a viewer, not a writer.
    "substrate.read",
    "substrate.subscribe",
    // Ontology Explorer Phase 1: tree enumeration. Request is
    // { prefix, depth }, response is { children: [...], tick }. The
    // webview-hosted Explorer panel calls this on each tree-node
    // expand to fetch immediate children from the daemon.
    "substrate.list",
    // Phase 3 control plane (commit a8bdc631): the Explorer's
    // control-intent toggle viewer fires `control.set_enabled` to
    // flip a sensor or service on/off; `control.list` is the read
    // counterpart for the upcoming control-overview panel. The
    // daemon's gate enforces authority — this allowlist just opens
    // the proxy.
    "control.set_enabled",
    "control.list",
    // LLM service: synchronous chat completion against the local
    // llama.cpp endpoint. Wired in the daemon at boot via DAEMON_LLM;
    // the chat window panel calls this for each user turn.
    "llm.prompt",
    // WeftOS Concierge: identity-aware tool-using chat. Wraps `llm.prompt`
    // with a built-in tool surface (read_file, list_directory) that lets
    // the assistant inspect the workspace before answering. Replaces
    // `llm.prompt` as the chat panel's wire for user turns; same llama.cpp
    // server underneath. Plan: docs/plans/chat-agent-v1.md §5.
    "agent.chat",
    // Terminal service: PTY-backed shell sessions hosted in the
    // daemon, surfaced as an Explorer panel. Output is published
    // to substrate (via the existing `substrate.read` proxy);
    // these four verbs cover spawn / write / resize / close. The
    // daemon's own per-session ownership check is the real gate —
    // this allowlist just keeps the webview from reaching arbitrary
    // RPC surface.
    "terminal.spawn",
    "terminal.write",
    "terminal.resize",
    "terminal.close",
]);

/// Mutable runtime allowlist. Starts as a copy of `STATIC_ALLOWED_METHODS`
/// and is refreshed on connect via `refreshAllowlist`.
const ALLOWED_METHODS = new Set<string>(STATIC_ALLOWED_METHODS);

/// Names the daemon might publish for its method-list introspection
/// RPC. Tried in order on connect; the first one to return a list of
/// strings wins. WEFT-250.
const INTROSPECTION_RPCS: readonly string[] = [
    "daemon.list_methods",
    "kernel.list_methods",
    "system.list_methods",
];

/// Refresh `ALLOWED_METHODS` against the daemon. Idempotent: every
/// call rebuilds the runtime set from `STATIC_ALLOWED_METHODS` plus
/// whatever the daemon advertises. Failures are logged + swallowed;
/// the static fallback keeps the panel usable against an old daemon.
async function refreshAllowlist(socketPath: string): Promise<void> {
    for (const method of INTROSPECTION_RPCS) {
        try {
            const resp = await rpcCall(
                socketPath,
                { method, params: null, id: randomUUID() },
                5_000,
            );
            const list = extractMethodList(resp.result);
            if (list && list.length > 0) {
                ALLOWED_METHODS.clear();
                for (const m of STATIC_ALLOWED_METHODS) ALLOWED_METHODS.add(m);
                for (const m of list) {
                    if (typeof m === "string") ALLOWED_METHODS.add(m);
                }
                console.log(
                    `weft: refreshed allowlist via ${method} — ` +
                        `${ALLOWED_METHODS.size} methods (` +
                        `${list.length} from daemon, ${STATIC_ALLOWED_METHODS.size} static)`,
                );
                return;
            }
        } catch (err) {
            // Method not implemented or daemon unreachable — try
            // the next candidate.
            void err;
        }
    }
    console.log(
        "weft: daemon did not answer any introspection RPC; " +
            `using static allowlist (${STATIC_ALLOWED_METHODS.size} methods)`,
    );
}

/// Pull a list of method names out of an introspection response.
/// Accepts either a bare `string[]` result or `{ methods: string[] }`.
function extractMethodList(result: unknown): string[] | undefined {
    if (Array.isArray(result)) {
        return result.filter((x): x is string => typeof x === "string");
    }
    if (result && typeof result === "object" && "methods" in result) {
        const m = (result as { methods: unknown }).methods;
        if (Array.isArray(m)) {
            return m.filter((x): x is string => typeof x === "string");
        }
    }
    return undefined;
}

interface WasmRpcRequest {
    type: "rpc-request";
    id: number;
    method: string;
    params?: unknown;
}

interface WebviewReadyMessage {
    type: "ready";
}

type WebviewInbound = WasmRpcRequest | WebviewReadyMessage;

export function activate(context: vscode.ExtensionContext): void {
    const openCmd = vscode.commands.registerCommand("weft.openPanel", () => {
        createOrShowPanel(context);
    });
    context.subscriptions.push(openCmd);

    const serializer: vscode.WebviewPanelSerializer = {
        async deserializeWebviewPanel(panel: vscode.WebviewPanel): Promise<void> {
            wirePanel(context, panel);
        },
    };
    context.subscriptions.push(
        vscode.window.registerWebviewPanelSerializer(VIEW_TYPE, serializer),
    );
}

export function deactivate(): void {}

let currentPanel: vscode.WebviewPanel | undefined;

function createOrShowPanel(context: vscode.ExtensionContext): void {
    if (currentPanel) {
        currentPanel.reveal(vscode.ViewColumn.Active);
        return;
    }

    const panel = vscode.window.createWebviewPanel(
        VIEW_TYPE,
        "WeftOS Panel",
        vscode.ViewColumn.Active,
        {
            enableScripts: true,
            retainContextWhenHidden: true,
            localResourceRoots: [
                vscode.Uri.joinPath(context.extensionUri, "media"),
                vscode.Uri.joinPath(context.extensionUri, "webview"),
            ],
        },
    );
    wirePanel(context, panel);
}

function wirePanel(context: vscode.ExtensionContext, panel: vscode.WebviewPanel): void {
    currentPanel = panel;
    panel.webview.options = {
        enableScripts: true,
        localResourceRoots: [
            vscode.Uri.joinPath(context.extensionUri, "media"),
            vscode.Uri.joinPath(context.extensionUri, "webview"),
        ],
    };
    panel.webview.html = renderHtml(context, panel.webview);

    const cwd = getWorkspaceCwd();
    const socketPath = resolveSocketPath(cwd);

    // WEFT-250: refresh the allowlist against the daemon on connect.
    // This call races the panel's "ready" handshake — that's fine,
    // since the wasm `Live` only starts firing RPCs after `ready`,
    // and rpc-request handling reads the (live-mutated) ALLOWED_METHODS
    // at dispatch time.
    void refreshAllowlist(socketPath);

    panel.webview.onDidReceiveMessage(
        async (raw: unknown) => {
            const msg = raw as WebviewInbound | null;
            if (!msg || typeof msg !== "object") {
                return;
            }
            if (msg.type === "ready") {
                void panel.webview.postMessage({ type: "hello", socketPath });
                return;
            }
            if (msg.type === "rpc-request") {
                await handleRpc(panel, socketPath, msg);
            }
        },
        undefined,
        context.subscriptions,
    );

    const hotReload = installWasmHotReload(context, panel);

    panel.onDidDispose(
        () => {
            hotReload.dispose();
            if (currentPanel === panel) {
                currentPanel = undefined;
            }
        },
        null,
        context.subscriptions,
    );
}

/// Dev loop: watch the built wasm bundle and re-render the webview HTML
/// when it changes. `renderHtml` mints a fresh cache-bust token each
/// call, so VSCode's module/resource loader fetches new bytes instead
/// of serving the previous wasm-bindgen output from cache.
///
/// Rebuild trigger on the author's side is the existing
/// `extensions/vscode-weft-panel/scripts/build-wasm.sh` — e.g. run it
/// under `cargo watch`:
///   cargo watch -w crates/clawft-gui-egui -s \
///     'extensions/vscode-weft-panel/scripts/build-wasm.sh'
///
/// Watches the directory (not the file) so the watcher survives
/// rename-on-write patterns that wasm-bindgen uses on some platforms.
/// Debounces burst writes (wasm-bindgen produces .js and .wasm almost
/// simultaneously; one reload is enough).
function installWasmHotReload(
    context: vscode.ExtensionContext,
    panel: vscode.WebviewPanel,
): vscode.Disposable {
    const wasmDir = vscode.Uri.joinPath(
        context.extensionUri,
        "webview",
        "wasm",
    ).fsPath;

    let debounceTimer: NodeJS.Timeout | undefined;
    let watcher: FSWatcher | undefined;
    let disposed = false;

    try {
        watcher = fsWatch(wasmDir, { persistent: false }, (_eventType, filename) => {
            if (disposed) return;
            // wasm-bindgen emits both the JS glue and the wasm bytes;
            // either change is a valid trigger.
            if (
                filename !== "clawft_gui_egui_bg.wasm" &&
                filename !== "clawft_gui_egui.js"
            ) {
                return;
            }
            if (debounceTimer) clearTimeout(debounceTimer);
            debounceTimer = setTimeout(() => {
                if (disposed) return;
                panel.webview.html = renderHtml(context, panel.webview);
                void vscode.window.setStatusBarMessage(
                    "$(sync) WeftOS: reloaded wasm bundle",
                    2000,
                );
            }, 200);
        });
    } catch (err) {
        // wasm dir may not exist on first activation (before build-wasm.sh
        // has run). Fail silently — user runs the build script, reopens
        // the panel, and hotload re-attaches on the next wirePanel call.
        console.warn("weft: wasm hot-reload watcher failed to attach:", err);
    }

    return {
        dispose: () => {
            disposed = true;
            if (debounceTimer) clearTimeout(debounceTimer);
            watcher?.close();
        },
    };
}

// Per-method RPC timeout. The default (`rpc.ts`'s 3000ms) is right for
// the daemon-local control verbs that round-trip in milliseconds, but
// `llm.prompt` proxies to a llama.cpp server doing CPU/GPU inference;
// even a short completion against a 35B-A3B model takes 5–30 s, and
// a longer one can run minutes. Anything that calls a model server
// gets the long bucket; everything else keeps fast-fail semantics so
// a stopped daemon surfaces immediately on the chips.
const LLM_TIMEOUT_MS = 300_000;
function timeoutForMethod(method: string): number | undefined {
    if (method === "llm.prompt" || method === "agent.chat") return LLM_TIMEOUT_MS;
    return undefined; // fall through to rpcCall's default
}

async function handleRpc(
    panel: vscode.WebviewPanel,
    socketPath: string,
    req: WasmRpcRequest,
): Promise<void> {
    if (!ALLOWED_METHODS.has(req.method)) {
        void panel.webview.postMessage({
            type: "rpc-response",
            id: req.id,
            ok: false,
            error: `method not allowed: ${req.method}`,
        });
        return;
    }

    try {
        const resp = await rpcCall(
            socketPath,
            {
                method: req.method,
                params: req.params ?? null,
                id: randomUUID(),
            },
            timeoutForMethod(req.method),
        );
        void panel.webview.postMessage({
            type: "rpc-response",
            id: req.id,
            ok: resp.ok,
            result: resp.result ?? null,
            error: resp.error,
        });
    } catch (err) {
        const message = err instanceof RpcError ? err.message : String(err);
        void panel.webview.postMessage({
            type: "rpc-response",
            id: req.id,
            ok: false,
            error: message,
        });
    }
}

function getWorkspaceCwd(): string | undefined {
    const folder = vscode.workspace.workspaceFolders?.[0];
    return folder?.uri.fsPath;
}

function renderHtml(context: vscode.ExtensionContext, webview: vscode.Webview): string {
    const nonce = makeNonce();
    // Cache-bust the JS glue + wasm so the hot-reload watcher's html
    // reassign actually fetches fresh bytes. VSCode's webview resource
    // loader honors query strings for cache identity; the wasm-bindgen
    // JS passes `module_or_path` through untouched, so the suffix rides
    // along on the wasm fetch as well.
    const cacheBust = Date.now().toString(36);
    const wasmRoot = vscode.Uri.joinPath(context.extensionUri, "webview", "wasm");
    const jsUri = webview
        .asWebviewUri(vscode.Uri.joinPath(wasmRoot, "clawft_gui_egui.js"))
        .toString();
    const wasmUri = webview
        .asWebviewUri(vscode.Uri.joinPath(wasmRoot, "clawft_gui_egui_bg.wasm"))
        .toString();
    const jsUrl = `${jsUri}?v=${cacheBust}`;
    const wasmUrl = `${wasmUri}?v=${cacheBust}`;

    // CSP note: `wasm-unsafe-eval` is required to instantiate WebAssembly
    // modules inside a webview. `script-src` allows the nonce'd bootstrap
    // plus the wasm-bindgen JS glue from `localResourceRoots`.
    const csp = [
        "default-src 'none'",
        `script-src 'nonce-${nonce}' ${webview.cspSource} 'wasm-unsafe-eval'`,
        `style-src 'nonce-${nonce}' ${webview.cspSource} 'unsafe-inline'`,
        `img-src ${webview.cspSource} data: blob:`,
        `font-src ${webview.cspSource}`,
        `connect-src ${webview.cspSource} blob:`,
        "worker-src blob:",
    ].join("; ");

    const wasmNotFoundHint = wasmUri;

    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta http-equiv="Content-Security-Policy" content="${csp}" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>WeftOS Panel</title>
  <style nonce="${nonce}">
    html, body { margin: 0; padding: 0; height: 100%; background: #04040a;
      color: #e0dee8; font-family: system-ui, sans-serif; overflow: hidden; }
    /* eframe 0.29 attaches a ResizeObserver directly to the canvas with
       ContentBox sizing. Give the canvas explicit width+height against a
       fixed-inset parent so the content box is well-defined at mount and
       tracks window resizes deterministically — flex-grow sizing left the
       canvas with an ambiguous content box in the Cursor webview. */
    #weft-root { position: fixed; inset: 0; }
    #weft-canvas { width: 100%; height: 100%; display: block; outline: none; }
    #weft-fallback { position: absolute; inset: 0; padding: 24px; font-size: 13px;
      color: #aaa; display: none; overflow: auto; background: #04040a; z-index: 5; }
    #weft-fallback code { background: #1a1a24; padding: 2px 6px; border-radius: 3px;
      color: #c4a25c; }
    body.loading #weft-splash { position: fixed; inset: 0; display: grid;
      place-items: center; font-size: 13px; color: #7a7a86;
      background: #04040a; z-index: 10; }
    body:not(.loading) #weft-splash { display: none; }
  </style>
</head>
<body class="loading">
  <div id="weft-splash">loading egui shell…</div>
  <div id="weft-root">
    <canvas id="weft-canvas" tabindex="0"></canvas>
    <div id="weft-fallback">
      <p><strong>Failed to load the wasm bundle.</strong></p>
      <p>Run <code>extensions/vscode-weft-panel/scripts/build-wasm.sh</code>
         from the repo root and reload the panel.</p>
      <p>Expected at <code>${wasmNotFoundHint}</code>.</p>
    </div>
  </div>
  <script nonce="${nonce}">
    // Bootstrap-level diagnostics: if the module script below fails to
    // parse/import (CSP, 404, syntax), neither its try/catch nor any
    // console logging reaches the splash. Catch at window level and
    // render the failure into the DOM so devtools are not required.
    (function () {
      const splash = document.getElementById("weft-splash");
      function surface(stage, err) {
        try {
          const text = err && err.stack ? err.stack : String(err);
          if (splash) {
            splash.style.whiteSpace = "pre-wrap";
            splash.style.padding = "24px";
            splash.style.textAlign = "left";
            splash.style.color = "#ff8080";
            splash.textContent = "[" + stage + "] " + text;
          }
        } catch (_) {}
      }
      window.addEventListener("error", (ev) => surface("error", ev.error || ev.message));
      window.addEventListener("unhandledrejection", (ev) => surface("unhandledrejection", ev.reason));
      // Watchdog: if nothing has removed the loading class within 8s,
      // the module script most likely never executed (CSP block on the
      // static import). Flag it visibly.
      setTimeout(() => {
        if (document.body.classList.contains("loading") && splash && !splash.dataset.err) {
          splash.style.whiteSpace = "pre-wrap";
          splash.style.padding = "24px";
          splash.style.textAlign = "left";
          splash.style.color = "#f0c060";
          splash.textContent =
            "[watchdog] module script did not finish in 8s.\\n" +
            "Likely causes:\\n" +
            "  • CSP blocked the import of clawft_gui_egui.js\\n" +
            "  • Wasm bundle missing or URL mismatch\\n" +
            "  • wasm-bindgen init or weft_start threw before DOM update\\n" +
            "Open: Developer: Open Webview Developer Tools for console.";
        }
      }, 8000);
    })();
  </script>
  <script type="module" nonce="${nonce}">
    import init, { weft_start } from "${jsUrl}";

    const vscode = acquireVsCodeApi();
    const splash = document.getElementById("weft-splash");
    const setSplash = (text, color) => {
      if (!splash) return;
      splash.dataset.err = "1";
      splash.style.whiteSpace = "pre-wrap";
      splash.style.padding = "24px";
      splash.style.textAlign = "left";
      splash.style.color = color || "#ff8080";
      splash.textContent = text;
    };

    // Bridge exposed to the wasm module. clawft_gui_egui::live::wasm_live
    // looks up window.__weftPostToHost and calls it with a JSON value.
    // Request shape:  { type: "rpc-request", id, method, params }
    // Response shape: { type: "rpc-response", id, ok, result?, error? }
    window.__weftPostToHost = (payload) => {
      try {
        vscode.postMessage(payload);
      } catch (e) {
        console.error("postMessage failed", e);
      }
    };

    try {
      setSplash("init: fetching wasm…", "#7a7a86");
      await init({ module_or_path: "${wasmUrl}" });
      setSplash("init: mounting egui…", "#7a7a86");
      // Remove loading class BEFORE awaiting weft_start so the canvas
      // becomes visible even if start() resolves slowly. If start
      // throws, catch restores the fallback below.
      document.body.classList.remove("loading");
      await weft_start("weft-canvas");
      vscode.postMessage({ type: "ready" });
    } catch (e) {
      console.error("wasm boot failed", e);
      const msg = e && e.stack ? e.stack : String(e);
      document.body.classList.remove("loading");
      const fb = document.getElementById("weft-fallback");
      if (fb) {
        fb.style.display = "block";
        const pre = document.createElement("pre");
        pre.style.color = "#ff8080";
        pre.style.whiteSpace = "pre-wrap";
        pre.textContent = msg;
        fb.appendChild(pre);
      }
      document.getElementById("weft-canvas").style.display = "none";
    }
  </script>
</body>
</html>`;
}

function makeNonce(): string {
    const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let out = "";
    for (let i = 0; i < 32; i += 1) {
        out += chars.charAt(Math.floor(Math.random() * chars.length));
    }
    return out;
}
