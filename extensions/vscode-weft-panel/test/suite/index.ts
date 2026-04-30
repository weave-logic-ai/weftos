// Mocha runner for the VSCode panel E2E test suite.
// Loaded by `@vscode/test-electron` once the headless host has finished
// activating the extension. WEFT-486 / M6-B (SCAFFOLD).
import * as path from "node:path";
import { glob } from "glob";
// eslint-disable-next-line @typescript-eslint/no-require-imports
const Mocha = require("mocha");

export async function run(): Promise<void> {
    const mocha = new Mocha({
        ui: "tdd",
        color: true,
        timeout: 60_000, // panel boot includes wasm fetch on first paint
    });

    const testsRoot = path.resolve(__dirname, "..");
    const files = await glob("**/*.test.js", { cwd: testsRoot });
    files.forEach((f: string) => mocha.addFile(path.resolve(testsRoot, f)));

    await new Promise<void>((resolve, reject) => {
        try {
            mocha.run((failures: number) => {
                if (failures > 0) {
                    reject(new Error(`${failures} test(s) failed.`));
                } else {
                    resolve();
                }
            });
        } catch (err) {
            reject(err);
        }
    });
}
