/**
 * Unit tests for the playground's pure logic (ADR-102 D2).
 *
 *   node --experimental-strip-types --test src/playground/core.test.ts
 *
 * (`npm run test:unit` runs every `*.test.ts` under src/.)
 */

import { describe, it } from "node:test";
import { strict as assert } from "node:assert";
import {
  buildArgs,
  buildCurl,
  buildPath,
  callApi,
  deref,
  initializeBody,
  listOperations,
  prettyBody,
  redact,
  remaining,
  rpcBody,
  schemaToFields,
  takeFragmentToken,
  toolsFromList,
  type FetchLike,
} from "./core.ts";

// A fixture token; not a real credential.
const TOKEN = "wft_0123456789abcdef0123456789abcdef";

describe("takeFragmentToken", () => {
  it("reads #token= and leaves the rest of the fragment", () => {
    assert.deepEqual(takeFragmentToken(`#token=${TOKEN}`), { token: TOKEN, rest: "" });
    assert.deepEqual(takeFragmentToken(`#tab=rest&token=${TOKEN}`), { token: TOKEN, rest: "tab=rest" });
  });

  it("accepts the short key and ignores a missing or blank token", () => {
    assert.equal(takeFragmentToken(`#t=${TOKEN}`).token, TOKEN);
    assert.equal(takeFragmentToken("").token, null);
    assert.equal(takeFragmentToken("#token=").token, null);
    assert.equal(takeFragmentToken("#other=1").token, null);
  });

  it("does not honour a query-string style token", () => {
    assert.equal(takeFragmentToken(`?token=${TOKEN}`).token, null);
  });
});

describe("remaining", () => {
  const now = Date.parse("2026-10-02T12:00:00Z");

  it("formats minutes and hours", () => {
    assert.equal(remaining("2026-10-02T12:14:05Z", now)?.label, "14m 05s");
    assert.equal(remaining("2026-10-02T13:02:03Z", now)?.label, "1h 02m 03s");
  });

  it("reports expiry and never goes negative", () => {
    const r = remaining("2026-10-02T11:00:00Z", now);
    assert.deepEqual(r, { expired: true, secs: 0, label: "expired" });
  });

  it("is null without a usable expiry", () => {
    assert.equal(remaining(undefined, now), null);
    assert.equal(remaining("not a date", now), null);
  });
});

describe("buildCurl", () => {
  const req = { method: "get", url: "http://localhost:18789/api/agents" };

  it("masked: reads the token from the environment, never inlines it", () => {
    const c = buildCurl(req, TOKEN, true);
    assert.ok(c.includes('"Authorization: Bearer $WEFT_TOKEN"'));
    assert.ok(!c.includes(TOKEN));
    assert.ok(c.includes("-X GET"));
  });

  it("unmasked: the token is in the Authorization header", () => {
    const c = buildCurl(req, TOKEN, false);
    assert.ok(c.includes(`Authorization: Bearer ${TOKEN}`));
  });

  it("never puts the token in the URL, masked or not", () => {
    for (const masked of [true, false]) {
      const url = buildCurl(req, TOKEN, masked).split("\n")[0];
      assert.ok(!url.includes(TOKEN));
      assert.ok(!/[?&](token|access_token)=/.test(buildCurl(req, TOKEN, masked)));
    }
  });

  it("adds a JSON body, shell-quoted", () => {
    const c = buildCurl({ method: "POST", url: "http://h/mcp", body: `{"a":"it's"}` }, TOKEN, true);
    assert.ok(c.includes("--data-raw '{\"a\":\"it'\\''s\"}'"));
    assert.ok(c.includes("Content-Type: application/json"));
  });

  it("falls back to the env var when there is no token", () => {
    assert.ok(buildCurl(req, null, false).includes("$WEFT_TOKEN"));
  });
});

describe("redact", () => {
  it("removes the token wherever it appears", () => {
    assert.equal(redact(`a ${TOKEN} b ${TOKEN}`, TOKEN), "a [redacted] b [redacted]");
    assert.equal(redact("plain", null), "plain");
  });
});

describe("callApi", () => {
  const ok: FetchLike = async () => ({ status: 200, statusText: "OK", text: async () => '{"a":1}' });

  it("sends the bearer in a header and measures latency", async () => {
    let seen: { url: string; headers: Record<string, string>; referrerPolicy?: string } | undefined;
    const f: FetchLike = async (url, init) => {
      seen = { url, headers: init.headers, referrerPolicy: init.referrerPolicy };
      return ok(url, init);
    };
    let t = 100;
    const rec = await callApi(f, "http://h", { method: "get", path: "/api/agents" }, TOKEN, () => (t += 42));
    assert.equal(seen?.url, "http://h/api/agents");
    assert.equal(seen?.headers.Authorization, `Bearer ${TOKEN}`);
    assert.equal(seen?.referrerPolicy, "no-referrer");
    assert.equal(rec.status, 200);
    assert.equal(rec.latencyMs, 42);
    assert.deepEqual(rec.json, { a: 1 });
    assert.equal(prettyBody(rec), '{\n  "a": 1\n}');
  });

  it("refuses a URL that would carry the token", async () => {
    await assert.rejects(callApi(ok, "http://h", { method: "GET", path: `/api/x?token=${TOKEN}` }, TOKEN), /token in a URL/);
  });

  it("records a 401 as a status, not an exception", async () => {
    const f: FetchLike = async () => ({ status: 401, statusText: "Unauthorized", text: async () => "" });
    const rec = await callApi(f, "http://h", { method: "GET", path: "/api/agents" }, TOKEN);
    assert.equal(rec.status, 401);
    assert.equal(rec.error, undefined);
  });

  it("records network failures and redacts the token from echoed bodies", async () => {
    const boom: FetchLike = async () => {
      throw new Error(`connect failed for ${TOKEN}`);
    };
    const rec = await callApi(boom, "http://h", { method: "GET", path: "/x" }, TOKEN);
    assert.equal(rec.status, null);
    assert.ok(rec.error && !rec.error.includes(TOKEN));

    const echo: FetchLike = async () => ({ status: 200, statusText: "OK", text: async () => `you sent ${TOKEN}` });
    const rec2 = await callApi(echo, "http://h", { method: "GET", path: "/x" }, TOKEN);
    assert.ok(!rec2.responseBody.includes(TOKEN));
  });

  it("sends Content-Type only with a body", async () => {
    const seen: Array<Record<string, string>> = [];
    const f: FetchLike = async (_u, init) => {
      seen.push(init.headers);
      return ok(_u, init);
    };
    await callApi(f, "http://h", { method: "POST", path: "/mcp", body: "{}" }, TOKEN);
    await callApi(f, "http://h", { method: "GET", path: "/x" }, TOKEN);
    assert.equal(seen[0]["Content-Type"], "application/json");
    assert.equal(seen[1]["Content-Type"], undefined);
  });
});

describe("MCP helpers", () => {
  it("builds JSON-RPC bodies with increasing ids", () => {
    const a = JSON.parse(rpcBody("tools/list"));
    const b = JSON.parse(rpcBody("tools/call", { name: "x", arguments: {} }));
    assert.equal(a.jsonrpc, "2.0");
    assert.equal(a.method, "tools/list");
    assert.equal("params" in a, false);
    assert.ok(b.id > a.id);
    assert.deepEqual(b.params, { name: "x", arguments: {} });
  });

  it("initialize carries a protocol version", () => {
    const m = JSON.parse(initializeBody());
    assert.equal(m.method, "initialize");
    assert.equal(typeof m.params.protocolVersion, "string");
  });

  it("extracts and sorts tools from tools/list", () => {
    const tools = toolsFromList({ result: { tools: [{ name: "b" }, { name: "a", description: "d" }, { nope: 1 }] } });
    assert.deepEqual(tools.map((t) => t.name), ["a", "b"]);
    assert.deepEqual(toolsFromList({ error: { code: -32002 } }), []);
    assert.deepEqual(toolsFromList(undefined), []);
  });
});

describe("schemaToFields / buildArgs", () => {
  const schema = {
    type: "object",
    required: ["path", "mode"],
    properties: {
      path: { type: "string", description: "File path" },
      mode: { enum: ["r", "w"] },
      limit: { type: "integer" },
      ratio: { type: ["number", "null"] },
      recursive: { type: "boolean" },
      tags: { type: "array", items: { type: "string" } },
      opts: { type: "object" },
    },
  };

  it("maps JSON Schema types to field kinds", () => {
    const kinds = Object.fromEntries(schemaToFields(schema).map((f) => [f.name, f.kind]));
    assert.deepEqual(kinds, {
      path: "string",
      mode: "enum",
      limit: "integer",
      ratio: "number",
      recursive: "boolean",
      tags: "json",
      opts: "json",
    });
    const path = schemaToFields(schema).find((f) => f.name === "path");
    assert.equal(path?.required, true);
    assert.equal(path?.description, "File path");
  });

  it("builds typed arguments and omits blank optionals", () => {
    const fields = schemaToFields(schema);
    const { args, errors } = buildArgs(fields, {
      path: "/a",
      mode: "w",
      limit: "5",
      recursive: true,
      tags: '["x","y"]',
    });
    assert.deepEqual(errors, {});
    assert.deepEqual(args, { path: "/a", mode: "w", limit: 5, recursive: true, tags: ["x", "y"] });
  });

  it("reports missing and malformed values", () => {
    const fields = schemaToFields(schema);
    const { errors } = buildArgs(fields, { limit: "1.5", ratio: "abc", tags: "[", mode: "r" });
    assert.equal(errors.path, "required");
    assert.equal(errors.limit, "must be an integer");
    assert.equal(errors.ratio, "must be a number");
    assert.equal(errors.tags, "must be valid JSON");
    assert.equal(errors.mode, undefined);
  });

  it("follows local $ref and survives a schema with no properties", () => {
    const root = { defs: { Q: { type: "object", properties: { n: { type: "integer" } } } } };
    assert.deepEqual(schemaToFields({ $ref: "#/defs/Q" }, root).map((f) => f.name), ["n"]);
    assert.deepEqual(schemaToFields(undefined), []);
    assert.deepEqual(deref({ $ref: "#/missing" }, root), {});
  });
});

describe("listOperations / buildPath", () => {
  const doc = {
    paths: {
      "/api/agents": { get: { summary: "List agents", tags: ["agents"] } },
      "/api/agents/{name}": {
        parameters: [{ name: "name", in: "path", required: true, schema: { type: "string" } }],
        get: { summary: "One agent", tags: ["agents"] },
        delete: { tags: ["agents"], parameters: [{ name: "force", in: "query", schema: { type: "boolean" } }] },
      },
      "/api/sessions/{key}/stream": { get: { tags: ["sessions"] } },
      "/mcp": {
        post: {
          tags: ["mcp"],
          requestBody: { required: true, content: { "application/json": { schema: { $ref: "#/components/schemas/Rpc" } } } },
        },
      },
    },
    components: { schemas: { Rpc: { type: "object", properties: { method: { type: "string" } } } } },
  };

  it("lists every method on every path, sorted, with path-level params", () => {
    const ops = listOperations(doc);
    assert.deepEqual(
      ops.map((o) => o.key),
      ["GET /api/agents", "GET /api/agents/{name}", "DELETE /api/agents/{name}", "POST /mcp", "GET /api/sessions/{key}/stream"].sort((a, b) => {
        const oa = ops.find((o) => o.key === a)!;
        const ob = ops.find((o) => o.key === b)!;
        return oa.tag.localeCompare(ob.tag) || oa.path.localeCompare(ob.path) || oa.method.localeCompare(ob.method);
      }),
    );
    const del = ops.find((o) => o.key === "DELETE /api/agents/{name}")!;
    assert.deepEqual(del.params.map((p) => `${p.in}:${p.name}`), ["path:name", "query:force"]);
  });

  it("resolves the request body schema and flags streaming routes", () => {
    const ops = listOperations(doc);
    const mcp = ops.find((o) => o.path === "/mcp")!;
    assert.equal(mcp.hasBody, true);
    assert.equal(mcp.bodyRequired, true);
    assert.deepEqual(Object.keys(mcp.bodySchema?.properties ?? {}), ["method"]);
    assert.equal(ops.find((o) => o.path.endsWith("/stream"))!.tryable, false);
    assert.equal(ops.find((o) => o.path === "/api/agents")!.tryable, true);
  });

  it("tolerates a document with no paths", () => {
    assert.deepEqual(listOperations({}), []);
    assert.deepEqual(listOperations(undefined), []);
  });

  it("fills path parameters (encoded) and the query string", () => {
    const del = listOperations(doc).find((o) => o.key === "DELETE /api/agents/{name}")!;
    assert.deepEqual(buildPath(del, { name: "a/b c", force: "true" }), { path: "/api/agents/a%2Fb%20c?force=true", missing: [] });
    assert.deepEqual(buildPath(del, {}), { path: "/api/agents/", missing: ["name"] });
  });
});
