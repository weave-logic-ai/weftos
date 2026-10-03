/**
 * /playground end to end (ADR-102 D2): open a token link, call a REST route
 * and an MCP tool, revoke, and see the next call refused with 401.
 *
 * The gateway is faked at the network layer (`page.route`), so the suite
 * needs no daemon; the Rust side has its own tests (gateway_auth.rs) for
 * what the real gateway does with the same requests.
 *
 * The page is `playground.html` here because `vite preview` has no route
 * for `/playground`; the gateway maps that path to this file.
 */

import { test, expect, type Page, type Route } from "@playwright/test";

// A fixture, not a credential.
const TOKEN = "wft_00112233445566778899aabbccddeeff";

interface Gateway {
  requests: Array<{ method: string; url: string; auth: string | null }>;
  live: boolean;
}

const SPEC = {
  openapi: "3.1.0",
  paths: {
    "/api/agents": { get: { summary: "List agents", tags: ["agents"] } },
    "/api/agents/{name}": {
      get: {
        summary: "One agent",
        tags: ["agents"],
        parameters: [{ name: "name", in: "path", required: true, schema: { type: "string" } }],
      },
    },
  },
};

const TOOLS = [
  {
    name: "echo",
    description: "Echo text back",
    inputSchema: {
      type: "object",
      required: ["text"],
      properties: { text: { type: "string", description: "What to echo" }, times: { type: "integer" } },
    },
  },
  { name: "ping", description: "No arguments", inputSchema: { type: "object", properties: {} } },
];

async function fakeGateway(page: Page): Promise<Gateway> {
  const gw: Gateway = { requests: [], live: true };
  const json = (route: Route, status: number, body: unknown) =>
    route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

  await page.route(/\/(api|mcp)(\/|$|\?)/, async (route) => {
    const req = route.request();
    const url = new URL(req.url());
    const auth = req.headers()["authorization"] ?? null;
    gw.requests.push({ method: req.method(), url: req.url(), auth });
    const authed = gw.live && auth === `Bearer ${TOKEN}`;

    if (url.pathname === "/api/health") {
      if (!authed) return json(route, 200, { status: "ok" });
      return json(route, 200, {
        status: "ok",
        version: "0.0.0-test",
        uptime_secs: 12,
        daemon: { reachable: true },
        kernel: { processes: 3 },
        chain: { head: "abc", sequence: 7 },
        mcp: { profile: "full", tools: 2 },
        channels: [{ name: "web", state: "connected" }],
        providers: [{ name: "anthropic", configured: true }],
        token: {
          id: "0123456789abcdef",
          label: "playground",
          issued_at: new Date().toISOString(),
          expires_at: new Date(Date.now() + 15 * 60_000).toISOString(),
        },
      });
    }
    if (!authed) return route.fulfill({ status: 401, headers: { "www-authenticate": "Bearer" } });

    if (url.pathname === "/api/auth/revoke") {
      gw.live = false;
      return route.fulfill({ status: 204 });
    }
    if (url.pathname === "/api/openapi.json") return json(route, 200, SPEC);
    if (url.pathname === "/api/agents") return json(route, 200, [{ name: "default" }]);
    if (url.pathname === "/mcp") {
      const msg = JSON.parse(req.postData() ?? "{}");
      if (msg.method === "notifications/initialized") return route.fulfill({ status: 202 });
      if (msg.method === "initialize") return json(route, 200, { jsonrpc: "2.0", id: msg.id, result: { protocolVersion: "2024-11-05" } });
      if (msg.method === "tools/list") return json(route, 200, { jsonrpc: "2.0", id: msg.id, result: { tools: TOOLS } });
      if (msg.method === "tools/call") {
        const text = String(msg.params?.arguments?.text ?? "");
        return json(route, 200, { jsonrpc: "2.0", id: msg.id, result: { content: [{ type: "text", text: `echo: ${text}` }] } });
      }
    }
    return route.fulfill({ status: 404 });
  });
  return gw;
}

test.describe("API playground", () => {
  test("token link -> REST call -> MCP call -> revoke -> next call is 401", async ({ page }) => {
    const gw = await fakeGateway(page);
    await page.goto(`/playground.html#token=${TOKEN}`);

    // Connected, with an expiry countdown, and the token is out of the URL.
    await expect(page.getByTestId("token-state")).toHaveText("active");
    await expect(page.getByTestId("token-countdown")).toHaveText(/^\d+m \d\ds$/);
    expect(new URL(page.url()).hash).toBe("");
    expect(page.url()).not.toContain(TOKEN);

    // Health tab: the tokened view.
    await expect(page.getByTestId("health-summary")).toContainText("version 0.0.0-test");
    await expect(page.getByText("providers", { exact: true }).first()).toBeVisible();

    // REST try-it, generated from the OpenAPI document.
    await page.getByRole("tab", { name: "REST" }).click();
    await expect(page.getByTestId("rest-op-count")).toContainText("2 operations");
    await page.getByRole("button", { name: /GET\s+\/api\/agents$/ }).click();
    await page.getByTestId("rest-send").click();
    await expect(page.getByTestId("call-status")).toContainText("200");
    await expect(page.getByTestId("call-latency")).toContainText("ms");
    await expect(page.getByTestId("response")).toContainText("default");

    // MCP form, generated from tools/list.
    await page.getByRole("tab", { name: "MCP tools" }).click();
    await expect(page.getByTestId("mcp-tool-count")).toContainText("2 tools");
    await page.getByRole("button", { name: "echo", exact: true }).click();
    await page.getByLabel(/^text/).fill("hello");
    await page.getByTestId("mcp-call").click();
    await expect(page.getByTestId("mcp-output")).toHaveText("echo: hello");

    // curl: masked by default, header-only token when unmasked.
    const curl = page.getByTestId("curl").last();
    await expect(curl).toContainText('"Authorization: Bearer $WEFT_TOKEN"');
    await expect(curl).not.toContainText(TOKEN);
    await page.getByLabel("Mask token in curl").uncheck();
    await expect(curl).toContainText(`Authorization: Bearer ${TOKEN}`);
    const firstLine = (await curl.innerText()).split("\n")[0];
    expect(firstLine).not.toContain(TOKEN);

    // Revoke, then the gateway refuses the same token.
    page.once("dialog", (d) => d.accept());
    await page.getByTestId("revoke").click();
    await expect(page.getByTestId("token-state")).toHaveText("revoked");
    await page.getByTestId("call-again").click();
    await expect(page.getByTestId("after-revoke").getByTestId("call-status")).toContainText("401");

    // Nothing ever carried the token in a URL; every authed call used the header.
    for (const r of gw.requests) expect(r.url, r.url).not.toContain(TOKEN);
    expect(gw.requests.some((r) => r.auth === `Bearer ${TOKEN}`)).toBe(true);

    // Memory only: no storage, no cookie.
    const stored = await page.evaluate(() => JSON.stringify([{ ...localStorage }, { ...sessionStorage }, document.cookie]));
    expect(stored).not.toContain(TOKEN);
  });

  test("no token: shows how to get one and calls nothing", async ({ page }) => {
    const gw = await fakeGateway(page);
    await page.goto("/playground.html");
    await expect(page.getByRole("heading", { name: /Open this page from a token link/ })).toBeVisible();
    expect(gw.requests).toEqual([]);
  });

  test("an unknown token is rejected and disables calls", async ({ page }) => {
    const gw = await fakeGateway(page);
    gw.live = false;
    await page.goto(`/playground.html#token=${TOKEN}`);
    await expect(page.getByTestId("token-state")).toHaveText("rejected");
    await expect(page.getByRole("tab", { name: "REST" })).toBeVisible();
  });
});
