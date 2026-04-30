// Headless VSCode E2E test entry point (WEFT-486 / M6-B SCAFFOLD).
//
// Downloads a pinned VSCode build, launches it with the
// `vscode-weft-panel` extension installed, and runs the Mocha suite at
// `out/test/suite/index.js`. Designed to run under `xvfb-run` in CI.
//
// SCAFFOLD-LEVEL only — see `suite/extension.test.ts` for the limited
// assertions wired today and the WEFT-XXX (0.8.x) followup that will
// expand them to cover the chip-icon rendering path.
import * as path from "node:path";
import { runTests } from "@vscode/test-electron";

async function main(): Promise<void> {
    try {
        // Extension under test = this extension. Lets the harness find
        // package.json and load `out/extension.js`.
        // __dirname at runtime: .../extensions/vscode-weft-panel/out-test
        const extensionDevelopmentPath = path.resolve(__dirname, "..");
        // Compiled Mocha entry — emitted by `tsc -p test/` into out-test/suite/.
        const extensionTestsPath = path.resolve(__dirname, "./suite/index");

        // Empty workspace — no folder open. The panel doesn't need a
        // workspace to render, only to resolve the daemon socket. Tests
        // that need a workspace can pass `--folder-uri` later.
        await runTests({
            extensionDevelopmentPath,
            extensionTestsPath,
            launchArgs: ["--disable-extensions", "--disable-workspace-trust"],
        });
    } catch (err) {
        console.error("Failed to run tests:", err);
        process.exit(1);
    }
}

void main();
