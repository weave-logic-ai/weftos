/**
 * The SPA service worker must never answer for the playground (ADR-102 D2):
 * the page holds a bearer token and must reach the gateway or fail, not be
 * replaced by the cached dashboard shell.
 *
 *   node --experimental-strip-types --test src/playground/sw.test.ts
 */

import { describe, it } from "node:test";
import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";

type Listener = (event: unknown) => void;

function loadWorker(): Listener {
  const src = readFileSync(new URL("../../public/sw.js", import.meta.url), "utf8");
  let onFetch: Listener | undefined;
  const self = {
    location: { origin: "http://gw" },
    addEventListener: (type: string, fn: Listener) => {
      if (type === "fetch") onFetch = fn;
    },
    skipWaiting: () => {},
    clients: { claim: () => {} },
  };
  runInNewContext(src, { self, caches: { open: async () => ({ match: async () => undefined }) }, URL, fetch: () => Promise.reject(new Error("offline")) });
  assert.ok(onFetch, "sw.js registered no fetch listener");
  return onFetch;
}

/** True when the worker took the request over (called respondWith). */
function handled(onFetch: Listener, path: string, mode = "navigate"): boolean {
  let responded = false;
  onFetch({
    request: { method: "GET", url: `http://gw${path}`, mode },
    respondWith: () => {
      responded = true;
    },
  });
  return responded;
}

describe("service worker and /playground", () => {
  const sw = loadWorker();

  it("bypasses every playground URL", () => {
    for (const p of ["/playground", "/playground/", "/playground.html"]) {
      assert.equal(handled(sw, p), false, p);
    }
  });

  it("still serves the SPA shell for other navigations", () => {
    assert.equal(handled(sw, "/"), true);
    assert.equal(handled(sw, "/agents"), true);
    assert.equal(handled(sw, "/playground-notes"), true);
  });

  it("still bypasses the API", () => {
    assert.equal(handled(sw, "/api/health"), false);
  });
});
