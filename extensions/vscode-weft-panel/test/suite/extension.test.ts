// VSCode panel end-to-end smoke (WEFT-486 / M6-B SCAFFOLD).
//
// Boots a headless VSCode host with the WeftOS Dev Panel installed,
// then exercises the extension's command surface and webview shell.
//
// Scope shipped today:
//   - Activation succeeds; the package's `weft.openPanel` command is
//     registered.
//   - `executeCommand("weft.openPanel")` resolves without throwing
//     (panel construction completes; the webview HTML is assigned).
//
// SCAFFOLD — explicitly NOT shipped today (deferred to 0.8.x):
//   - Chip-icon DOM assertion. The chip strip is rendered inside the
//     egui canvas via the wasm bundle, so DOM-side `webview.html`
//     introspection cannot see chip elements directly. Closing this
//     gap requires either:
//       (a) a test-only mock-provider injection that publishes a known
//           chip set into a DOM-side overlay the test can query, or
//       (b) a screenshot-diff harness running against the canvas.
//     Tracked as a 0.8.x followup; this scaffold lets that work plug
//     into an already-wired CI surface instead of standing one up
//     from scratch later.
//
// The Mocha suite is loaded by `suite/index.ts`; the host is launched
// by `runTest.ts` (under `xvfb-run` in CI).

import * as assert from "node:assert";
import * as vscode from "vscode";
// eslint-disable-next-line @typescript-eslint/no-require-imports
const { suite, test } = require("mocha");

suite("weft-panel: smoke", () => {
    test("extension is present", () => {
        const ext = vscode.extensions.getExtension(
            "weavelogic.vscode-weft-panel",
        );
        assert.ok(ext, "weavelogic.vscode-weft-panel extension not found");
    });

    test("extension activates", async () => {
        const ext = vscode.extensions.getExtension(
            "weavelogic.vscode-weft-panel",
        );
        assert.ok(ext, "extension not present");
        await ext!.activate();
        assert.strictEqual(ext!.isActive, true, "extension failed to activate");
    });

    test("weft.openPanel command is registered", async () => {
        const cmds = await vscode.commands.getCommands(true);
        assert.ok(
            cmds.includes("weft.openPanel"),
            "weft.openPanel not registered — package.json contributes drift?",
        );
    });

    test("weft.openPanel resolves without throwing", async () => {
        // Panel construction creates a webview, assigns html, wires the
        // postMessage bridge, and starts the daemon-allowlist refresh
        // (which fails-open against an offline daemon under test).
        // This proves the host wiring is intact even without a daemon.
        // `executeCommand` returns a `Thenable`, not a `Promise`. Wrap
        // in a thunk so `assert.doesNotReject`'s overloads pick the
        // Promise<unknown> path.
        await assert.doesNotReject(
            async () => {
                await vscode.commands.executeCommand("weft.openPanel");
            },
            "weft.openPanel threw",
        );
    });

    // ------------------------------------------------------------------
    // SCAFFOLD: chip-icon assertion stub. Skipped until the 0.8.x
    // followup lands a DOM-introspectable chip surface (see file header).
    // ------------------------------------------------------------------
    test.skip("[0.8.x] chip strip exposes >=1 chip element", () => {
        // Followup will:
        //  1. Inject a mock-provider chip set into the webview via a
        //     test-only postMessage hook OR enable the panel's debug
        //     a11y overlay so chip ids land in the DOM.
        //  2. Read panel.webview.html (or use a custom message round-trip)
        //     to assert the chip elements / count.
        //  3. Tear the panel down between tests so leakage doesn't
        //     bleed across cases.
        assert.fail("not yet implemented — see 0.8.x followup");
    });
});
