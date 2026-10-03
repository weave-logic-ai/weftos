/**
 * Pure logic for the /playground page (ADR-102 D2).
 *
 * No DOM and no React in here, so it runs under `node --test`. The token
 * rules live in this file:
 *
 *   - it arrives in the URL fragment (`#token=`), which browsers never send;
 *   - it is held in memory only (callers keep it in component state);
 *   - it leaves the page only in an `Authorization` header, never in a URL
 *     or query string, and `callApi` refuses a URL that contains it.
 */

// ─── Token from the URL fragment ───────────────────────────────────────

/** Fragment keys accepted for the token (`weft ui` prints `token`). */
const FRAGMENT_KEYS = ["token", "t"];

export interface FragmentTake {
  token: string | null;
  /** The fragment with the token removed (no leading `#`). */
  rest: string;
}

/**
 * Pull the token out of `location.hash` and return what is left of it.
 * Segments other than the token keys (including ones that are not
 * `key=value`) are kept verbatim, in order.
 */
export function takeFragmentToken(hash: string): FragmentTake {
  // A query string is not a fragment; never honour a token from one.
  if (hash.startsWith("?")) return { token: null, rest: hash };
  const frag = hash.startsWith("#") ? hash.slice(1) : hash;
  let token: string | null = null;
  const kept: string[] = [];
  for (const seg of frag.split("&")) {
    const eq = seg.indexOf("=");
    const key = eq === -1 ? "" : decodeURIComponent(safe(seg.slice(0, eq)));
    if (eq !== -1 && FRAGMENT_KEYS.includes(key)) {
      const v = decodeURIComponent(safe(seg.slice(eq + 1).replace(/\+/g, " "))).trim();
      if (v) token ??= v;
      continue;
    }
    if (seg !== "") kept.push(seg);
  }
  return { token, rest: kept.join("&") };
}

/** `decodeURIComponent` input that never throws on a stray `%`. */
function safe(s: string): string {
  return s.replace(/%(?![0-9a-fA-F]{2})/g, "%25");
}

// ─── Expiry countdown ──────────────────────────────────────────────────

export interface Remaining {
  expired: boolean;
  secs: number;
  label: string;
}

/** Time left until `expiresAt` (RFC 3339), as seconds and `1h 02m 03s`. */
export function remaining(expiresAt: string | null | undefined, now: number): Remaining | null {
  if (!expiresAt) return null;
  const end = Date.parse(expiresAt);
  if (Number.isNaN(end)) return null;
  const secs = Math.max(0, Math.floor((end - now) / 1000));
  if (secs === 0) return { expired: true, secs, label: "expired" };
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  const label = h > 0 ? `${h}h ${pad(m)}m ${pad(s)}s` : `${m}m ${pad(s)}s`;
  return { expired: false, secs, label };
}

// ─── curl ──────────────────────────────────────────────────────────────

export interface CurlRequest {
  method: string;
  /** Absolute URL, no token in it. */
  url: string;
  body?: string;
}

/** Env var the masked curl reads the token from. */
export const TOKEN_ENV = "WEFT_TOKEN";

/** Quote for a POSIX shell with single quotes. */
export function shq(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

/**
 * A copyable `curl`. The token only ever goes in the `Authorization`
 * header. Masked (the default), the header reads `$WEFT_TOKEN`, so the
 * snippet is safe to paste into a chat or a ticket and runs after
 * `export WEFT_TOKEN=…`; unmasked, the real token is inlined.
 */
export function buildCurl(req: CurlRequest, token: string | null, masked: boolean): string {
  const bearer = masked || !token ? `$${TOKEN_ENV}` : token;
  const lines = [`curl -sS -X ${req.method.toUpperCase()} ${shq(req.url)}`];
  // Double quotes so `$WEFT_TOKEN` expands; an inlined token is single-quoted
  // when it holds shell metacharacters (issued tokens are `wft_` + hex).
  if (!masked && token && /["$`\\!]/.test(token)) {
    lines.push(`-H ${shq(`Authorization: Bearer ${token}`)}`);
  } else {
    lines.push(`-H "Authorization: Bearer ${bearer}"`);
  }
  if (req.body !== undefined && req.body !== "") {
    lines.push(`-H ${shq("Content-Type: application/json")}`, `--data-raw ${shq(req.body)}`);
  }
  return lines.join(" \\\n  ");
}

/** Replace every occurrence of the token in `text` (response bodies, errors). */
export function redact(text: string, token: string | null): string {
  if (!token) return text;
  return text.split(token).join("[redacted]");
}

// ─── Calling the gateway ───────────────────────────────────────────────

export interface CallSpec {
  method: string;
  /** Path starting with `/`, query string allowed, never a token. */
  path: string;
  body?: string;
}

export interface CallRecord {
  method: string;
  url: string;
  requestBody?: string;
  status: number | null;
  statusText: string;
  latencyMs: number;
  responseBody: string;
  /** Parsed JSON when the response is JSON. */
  json?: unknown;
  /** Network-level failure (no HTTP response). */
  error?: string;
  at: number;
}

export type FetchLike = (
  input: string,
  init: { method: string; headers: Record<string, string>; body?: string; cache: "no-store"; referrerPolicy: "no-referrer" },
) => Promise<{ status: number; statusText: string; text(): Promise<string> }>;

/**
 * Send one request with the bearer in a header. Throws before sending if
 * the token would end up in the URL.
 */
export async function callApi(
  fetchFn: FetchLike,
  origin: string,
  spec: CallSpec,
  token: string,
  now: () => number = () => performance.now(),
): Promise<CallRecord> {
  if (!spec.path.startsWith("/")) throw new Error("path must start with /");
  const url = origin + spec.path;
  if (token && url.includes(token)) throw new Error("refusing to put the token in a URL");
  const headers: Record<string, string> = { Authorization: `Bearer ${token}`, Accept: "application/json" };
  if (spec.body !== undefined && spec.body !== "") headers["Content-Type"] = "application/json";
  const started = now();
  const rec: CallRecord = {
    method: spec.method.toUpperCase(),
    url,
    requestBody: spec.body,
    status: null,
    statusText: "",
    latencyMs: 0,
    responseBody: "",
    at: Date.now(),
  };
  try {
    const resp = await fetchFn(url, {
      method: rec.method,
      headers,
      body: spec.body === "" ? undefined : spec.body,
      cache: "no-store",
      referrerPolicy: "no-referrer",
    });
    const text = await resp.text();
    rec.latencyMs = Math.round(now() - started);
    rec.status = resp.status;
    rec.statusText = resp.statusText;
    rec.responseBody = redact(text, token);
    try {
      rec.json = JSON.parse(text);
    } catch {
      /* not JSON */
    }
  } catch (e) {
    rec.latencyMs = Math.round(now() - started);
    rec.error = redact(e instanceof Error ? e.message : String(e), token);
  }
  return rec;
}

/** Pretty-print a response body when it is JSON. */
export function prettyBody(rec: CallRecord): string {
  if (rec.json !== undefined) return JSON.stringify(rec.json, null, 2);
  return rec.responseBody;
}

// ─── MCP (JSON-RPC over POST /mcp) ─────────────────────────────────────

export const MCP_PATH = "/mcp";
const MCP_PROTOCOL_VERSION = "2024-11-05";

let rpcId = 0;

export function rpcBody(method: string, params?: unknown): string {
  const msg: Record<string, unknown> = { jsonrpc: "2.0", id: ++rpcId, method };
  if (params !== undefined) msg.params = params;
  return JSON.stringify(msg);
}

export function initializeBody(): string {
  return rpcBody("initialize", {
    protocolVersion: MCP_PROTOCOL_VERSION,
    capabilities: {},
    clientInfo: { name: "weftos-playground", version: "1" },
  });
}

export interface McpTool {
  name: string;
  description?: string;
  inputSchema?: JsonSchema;
}

/** Tools from a `tools/list` response, or `[]` when it is an error. */
export function toolsFromList(json: unknown): McpTool[] {
  const tools = (json as { result?: { tools?: unknown } } | undefined)?.result?.tools;
  if (!Array.isArray(tools)) return [];
  return tools
    .filter((t): t is McpTool => !!t && typeof (t as McpTool).name === "string")
    .sort((a, b) => a.name.localeCompare(b.name));
}

// ─── JSON Schema → form fields ─────────────────────────────────────────

export interface JsonSchema {
  type?: string | string[];
  description?: string;
  properties?: Record<string, JsonSchema>;
  required?: string[];
  enum?: unknown[];
  default?: unknown;
  items?: JsonSchema;
  $ref?: string;
  oneOf?: JsonSchema[];
  anyOf?: JsonSchema[];
  allOf?: JsonSchema[];
  [k: string]: unknown;
}

export type FieldKind = "string" | "number" | "integer" | "boolean" | "enum" | "json";

export interface Field {
  name: string;
  kind: FieldKind;
  required: boolean;
  description: string;
  enumValues?: string[];
  defaultValue?: unknown;
  /** Shown as a hint for json fields (`array`, `object`). */
  typeHint?: string;
}

/** Follow a local `#/…` `$ref`. Unknown refs resolve to an empty schema. */
export function deref(schema: JsonSchema | undefined, root: unknown, depth = 0): JsonSchema {
  if (!schema) return {};
  if (!schema.$ref || depth > 16 || !schema.$ref.startsWith("#/")) return schema;
  let cur: unknown = root;
  for (const seg of schema.$ref.slice(2).split("/")) {
    cur = (cur as Record<string, unknown> | undefined)?.[seg.replace(/~1/g, "/").replace(/~0/g, "~")];
  }
  return deref((cur ?? {}) as JsonSchema, root, depth + 1);
}

function primaryType(s: JsonSchema): string | undefined {
  if (Array.isArray(s.type)) return s.type.find((t) => t !== "null");
  return s.type;
}

/** One field per top-level property; nested values become JSON fields. */
export function schemaToFields(schema: JsonSchema | undefined, root: unknown = schema): Field[] {
  const s = deref(schema, root);
  const props = s.properties ?? {};
  const required = new Set(s.required ?? []);
  return Object.entries(props).map(([name, raw]) => {
    const p = deref(raw, root);
    const type = primaryType(p);
    const base = {
      name,
      required: required.has(name),
      description: typeof p.description === "string" ? p.description : "",
      defaultValue: p.default,
    };
    if (Array.isArray(p.enum) && p.enum.length > 0) {
      return { ...base, kind: "enum" as const, enumValues: p.enum.map(String) };
    }
    if (type === "string") return { ...base, kind: "string" as const };
    if (type === "integer") return { ...base, kind: "integer" as const };
    if (type === "number") return { ...base, kind: "number" as const };
    if (type === "boolean") return { ...base, kind: "boolean" as const };
    return { ...base, kind: "json" as const, typeHint: type ?? "any" };
  });
}

export type FormValues = Record<string, string | boolean | undefined>;

export interface BuiltArgs {
  args: Record<string, unknown>;
  errors: Record<string, string>;
}

/** Turn form state into an `arguments` object; blank optional fields are omitted. */
export function buildArgs(fields: Field[], values: FormValues): BuiltArgs {
  const args: Record<string, unknown> = {};
  const errors: Record<string, string> = {};
  for (const f of fields) {
    const v = values[f.name];
    const blank = v === undefined || v === "";
    if (f.kind === "boolean") {
      if (v === undefined) {
        if (f.required) args[f.name] = false;
      } else {
        args[f.name] = v === true || v === "true";
      }
      continue;
    }
    if (blank) {
      if (f.required) errors[f.name] = "required";
      continue;
    }
    const text = String(v);
    switch (f.kind) {
      case "string":
      case "enum":
        args[f.name] = text;
        break;
      case "integer":
      case "number": {
        const n = Number(text);
        if (!Number.isFinite(n) || (f.kind === "integer" && !Number.isInteger(n))) {
          errors[f.name] = f.kind === "integer" ? "must be an integer" : "must be a number";
        } else {
          args[f.name] = n;
        }
        break;
      }
      case "json":
        try {
          args[f.name] = JSON.parse(text);
        } catch {
          errors[f.name] = "must be valid JSON";
        }
        break;
    }
  }
  return { args, errors };
}

// ─── OpenAPI → REST operations ─────────────────────────────────────────

export interface Param {
  name: string;
  in: "path" | "query" | "header" | "cookie";
  required: boolean;
  description: string;
  schema: JsonSchema;
}

export interface Operation {
  key: string;
  method: string;
  path: string;
  summary: string;
  tag: string;
  params: Param[];
  hasBody: boolean;
  bodyRequired: boolean;
  bodySchema?: JsonSchema;
  /** Streaming/upgrade routes the playground cannot drive with fetch. */
  tryable: boolean;
}

const METHODS = ["get", "post", "put", "delete", "patch"];

interface OpenApiDoc {
  paths?: Record<string, Record<string, unknown>>;
  [k: string]: unknown;
}

const NOT_TRYABLE = [/\/stream$/, /^\/ws$/, /^\/events$/];

/** Flatten an OpenAPI 3.x document into a sorted list of operations. */
export function listOperations(doc: unknown): Operation[] {
  const paths = (doc as OpenApiDoc | undefined)?.paths ?? {};
  const out: Operation[] = [];
  for (const [path, item] of Object.entries(paths)) {
    const shared = Array.isArray(item.parameters) ? (item.parameters as Param[]) : [];
    for (const method of METHODS) {
      const op = item[method] as Record<string, unknown> | undefined;
      if (!op) continue;
      const own = Array.isArray(op.parameters) ? (op.parameters as Param[]) : [];
      const params = [...shared, ...own].map((p) => ({
        name: p.name,
        in: p.in,
        required: p.in === "path" ? true : !!p.required,
        description: p.description ?? "",
        schema: deref(p.schema, doc),
      }));
      const body = op.requestBody as
        | { required?: boolean; content?: Record<string, { schema?: JsonSchema }> }
        | undefined;
      const tags = Array.isArray(op.tags) ? (op.tags as string[]) : [];
      out.push({
        key: `${method.toUpperCase()} ${path}`,
        method: method.toUpperCase(),
        path,
        summary: typeof op.summary === "string" ? op.summary : "",
        tag: tags[0] ?? "other",
        params: params.filter((p) => p.in === "path" || p.in === "query"),
        hasBody: !!body,
        bodyRequired: !!body?.required,
        bodySchema: deref(body?.content?.["application/json"]?.schema, doc),
        tryable: !NOT_TRYABLE.some((re) => re.test(path)),
      });
    }
  }
  return out.sort((a, b) => a.tag.localeCompare(b.tag) || a.path.localeCompare(b.path) || a.method.localeCompare(b.method));
}

export interface BuiltPath {
  path: string;
  missing: string[];
}

/** Fill `{name}` placeholders and append the query string. */
export function buildPath(op: Operation, values: Record<string, string>): BuiltPath {
  const missing: string[] = [];
  let path = op.path;
  for (const p of op.params.filter((x) => x.in === "path")) {
    const v = (values[p.name] ?? "").trim();
    if (!v) missing.push(p.name);
    path = path.replace(`{${p.name}}`, encodeURIComponent(v));
  }
  const q = new URLSearchParams();
  for (const p of op.params.filter((x) => x.in === "query")) {
    const v = (values[p.name] ?? "").trim();
    if (v) q.set(p.name, v);
    else if (p.required) missing.push(p.name);
  }
  const qs = q.toString();
  return { path: qs ? `${path}?${qs}` : path, missing };
}
