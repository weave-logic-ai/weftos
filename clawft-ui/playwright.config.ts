/**
 * Playwright config for the clawft-ui dashboard E2E suite (WEFT-314).
 *
 * The suite runs against the Vite dev server with MSW mocks
 * (`VITE_MOCK_API=true`) so CI stays deterministic and does not need a
 * live `weft gateway` process. A second job that targets a real gateway
 * is a follow-up.
 *
 * On developer machines:
 *
 *   npm run test:e2e:install   # one-time chromium download
 *   npm run test:e2e           # headless run
 *   npm run test:e2e:ui        # interactive runner
 *
 * From the build wrapper:
 *
 *   scripts/build.sh ui-e2e    # installs deps + chromium + runs the suite
 */

import { defineConfig, devices } from "@playwright/test";

const PORT = Number(process.env.CLAWFT_UI_E2E_PORT ?? 4173);
const BASE_URL = process.env.CLAWFT_UI_E2E_BASE_URL ?? `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  // Failures must emit screenshots + traces so CI postmortem is possible.
  reporter: process.env.CI ? [["github"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  // Boot Vite preview against the production bundle with MSW enabled.
  // Skip via CLAWFT_UI_E2E_SKIP_SERVER=1 if a separate process already
  // serves the UI (e.g. a real gateway).
  webServer: process.env.CLAWFT_UI_E2E_SKIP_SERVER
    ? undefined
    : {
        command: "npm run build && VITE_MOCK_API=true npm run preview -- --port " + PORT,
        url: BASE_URL,
        reuseExistingServer: !process.env.CI,
        timeout: 180_000,
      },
});
