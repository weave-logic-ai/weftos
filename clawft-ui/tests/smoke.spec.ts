/**
 * Dashboard smoke tests (WEFT-314).
 *
 * These tests intentionally exercise the user-visible bootstrap path
 * (page loads, root component renders, no console errors) rather than
 * any individual feature. Feature-specific suites land alongside the
 * features they cover.
 */

import { test, expect } from "@playwright/test";

test.describe("Dashboard bootstrap", () => {
  test("loads the dashboard with a recognizable title", async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error") {
        consoleErrors.push(msg.text());
      }
    });

    await page.goto("/");

    // Vite's index.html ships with a <title>; assert it loaded so we know
    // the SPA shell came down before any client-side render kicked in.
    await expect(page).toHaveTitle(/clawft|vite|react/i);

    // The root mount point must exist (and contain at least one child) so
    // we know React mounted.
    await expect(page.locator("#root")).toBeAttached();
    await page.waitForFunction(
      () => document.querySelector("#root")?.children.length ?? 0 > 0,
    );

    // Filter out the predictable MSW activation log; anything else is a
    // real regression and should fail the suite.
    const realErrors = consoleErrors.filter(
      (e) => !/mocking enabled|MSW/i.test(e),
    );
    expect(realErrors, `console errors during boot:\n${realErrors.join("\n")}`).toEqual([]);
  });

  test("strips the ?token= query param after first paint (WEFT-309)", async ({ page }) => {
    await page.goto("/?token=test-token-uuid&mode=mock");

    // The use-auth hook persists the token to localStorage and replaces
    // the URL via history.replaceState. After the first effect fires the
    // address bar should no longer contain ?token=.
    await page.waitForFunction(() => !window.location.search.includes("token="));
    const url = new URL(page.url());
    expect(url.searchParams.get("token")).toBeNull();

    const stored = await page.evaluate(() => localStorage.getItem("clawft-token"));
    expect(stored).toBe("test-token-uuid");
  });
});

test.describe("Feature surfaces (placeholders)", () => {
  // These are scaffolded but skipped until the corresponding routes have
  // stable selectors. Each is tracked as a follow-up so we don't regress
  // the green smoke run while features stabilize.
  // TODO(WEFT-314 follow-up): WebChat streaming round-trip via MSW.
  test.skip("WebChat streaming round-trip", async () => {});
  // TODO(WEFT-314 follow-up): Canvas command flow.
  test.skip("Canvas command flow", async () => {});
  // TODO(WEFT-314 follow-up): Browser-mode bootstrap (?mode=wasm).
  test.skip("Browser-mode bootstrap", async () => {});
});
