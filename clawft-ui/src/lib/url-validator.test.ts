/**
 * Unit tests for the cors_proxy URL validator (WEFT-310).
 *
 * Uses Node's built-in `node:test` runner so it can be executed without a
 * separate test framework install. Run with:
 *
 *   node --experimental-strip-types --test clawft-ui/src/lib/url-validator.test.ts
 *
 * or, once Node ≥22.6 is the floor, simply `node --test`.
 */

import { describe, it } from "node:test";
import { strict as assert } from "node:assert";
import { validateCorsProxyUrl } from "./url-validator.ts";

describe("validateCorsProxyUrl", () => {
  it("accepts an empty string (proxy is optional)", () => {
    assert.equal(validateCorsProxyUrl("").valid, true);
    assert.equal(validateCorsProxyUrl("   ").valid, true);
    assert.equal(validateCorsProxyUrl(null).valid, true);
    assert.equal(validateCorsProxyUrl(undefined).valid, true);
  });

  it("accepts an HTTPS URL", () => {
    const r = validateCorsProxyUrl("https://proxy.example.com/v1");
    assert.equal(r.valid, true, r.error);
  });

  it("accepts http://localhost", () => {
    assert.equal(validateCorsProxyUrl("http://localhost:8080/").valid, true);
    assert.equal(validateCorsProxyUrl("http://127.0.0.1:8080/proxy").valid, true);
    assert.equal(validateCorsProxyUrl("http://[::1]:8080/").valid, true);
  });

  it("rejects http:// against a public host", () => {
    const r = validateCorsProxyUrl("http://proxy.example.com/");
    assert.equal(r.valid, false);
    assert.match(r.error ?? "", /HTTPS/i);
  });

  it("rejects unsupported schemes", () => {
    const r = validateCorsProxyUrl("ftp://files.example.com/");
    assert.equal(r.valid, false);
    assert.match(r.error ?? "", /scheme/i);
  });

  it("rejects malformed input", () => {
    const r = validateCorsProxyUrl("not a url");
    assert.equal(r.valid, false);
  });
});
