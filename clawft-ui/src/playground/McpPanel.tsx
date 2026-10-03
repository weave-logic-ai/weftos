import { useEffect, useMemo, useState } from "react";
import { Button } from "../components/ui/button";
import { CallView } from "./CallView";
import { SchemaForm } from "./SchemaForm";
import { usePlayground } from "./context.tsx";
import {
  buildArgs,
  initializeBody,
  MCP_PATH,
  rpcBody,
  schemaToFields,
  toolsFromList,
  type CallRecord,
  type FormValues,
  type McpTool,
} from "./core.ts";

/** Text blocks of a `tools/call` result, and whether the tool flagged an error. */
function resultText(rec: CallRecord): { text: string; isError: boolean } | null {
  const result = (rec.json as { result?: { content?: Array<{ type?: string; text?: string }>; isError?: boolean } } | undefined)
    ?.result;
  if (!result?.content) return null;
  const text = result.content
    .filter((c) => c.type === "text" && typeof c.text === "string")
    .map((c) => c.text)
    .join("\n");
  return { text, isError: !!result.isError };
}

export function McpPanel() {
  const { call, active } = usePlayground();
  const [tools, setTools] = useState<McpTool[]>([]);
  const [listRec, setListRec] = useState<CallRecord | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [values, setValues] = useState<FormValues>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [raw, setRaw] = useState(false);
  const [rawText, setRawText] = useState("{}");
  const [rec, setRec] = useState<CallRecord | null>(null);
  const [busy, setBusy] = useState(false);

  // Handshake, then list. Re-run on demand with the Reload button.
  const [reloads, setReloads] = useState(0);
  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    (async () => {
      const init = await call({ method: "POST", path: MCP_PATH, body: initializeBody() });
      if (cancelled) return;
      if (init.status !== 200) {
        setListRec(init);
        setLoaded(true);
        return;
      }
      await call({
        method: "POST",
        path: MCP_PATH,
        body: JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
      });
      const list = await call({ method: "POST", path: MCP_PATH, body: rpcBody("tools/list", {}) });
      if (cancelled) return;
      setListRec(list);
      setTools(toolsFromList(list.json));
      setLoaded(true);
    })();
    return () => {
      cancelled = true;
    };
  }, [call, active, reloads]);

  const tool = tools.find((t) => t.name === selected) ?? null;
  const fields = useMemo(() => schemaToFields(tool?.inputSchema), [tool]);
  const visible = tools.filter((t) => t.name.toLowerCase().includes(filter.toLowerCase()));

  const pick = (name: string) => {
    setSelected(name);
    setValues({});
    setErrors({});
    setRawText("{}");
    setRec(null);
  };

  const run = async () => {
    if (!tool) return;
    let args: Record<string, unknown>;
    if (raw) {
      try {
        args = JSON.parse(rawText);
      } catch {
        setErrors({ _raw: "arguments must be valid JSON" });
        return;
      }
    } else {
      const built = buildArgs(fields, values);
      setErrors(built.errors);
      if (Object.keys(built.errors).length > 0) return;
      args = built.args;
    }
    setBusy(true);
    try {
      setRec(await call({ method: "POST", path: MCP_PATH, body: rpcBody("tools/call", { name: tool.name, arguments: args }) }));
    } finally {
      setBusy(false);
    }
  };

  const out = rec ? resultText(rec) : null;

  return (
    <div className="grid gap-6 md:grid-cols-[18rem_1fr]">
      <aside aria-label="MCP tools">
        <div className="mb-2 flex items-center gap-2">
          <input
            type="search"
            aria-label="Filter tools"
            placeholder="Filter tools"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            className="w-full rounded-md border border-gray-600 bg-gray-800 px-2 py-1.5 text-sm"
          />
          <Button size="sm" variant="outline" onClick={() => setReloads((n) => n + 1)} disabled={!active}>
            Reload
          </Button>
        </div>
        <p className="mb-2 text-xs text-gray-500" data-testid="mcp-tool-count">
          {loaded ? `${tools.length} tools from tools/list` : "Connecting to /mcp…"}
        </p>
        <ul className="max-h-[32rem] space-y-0.5 overflow-auto">
          {visible.map((t) => (
            <li key={t.name}>
              <button
                type="button"
                onClick={() => pick(t.name)}
                aria-current={t.name === selected}
                className={`w-full truncate rounded px-2 py-1 text-left font-mono text-xs ${
                  t.name === selected ? "bg-blue-900 text-white" : "text-gray-300 hover:bg-gray-800"
                }`}
              >
                {t.name}
              </button>
            </li>
          ))}
        </ul>
        {loaded && tools.length === 0 ? <CallView rec={listRec} title="tools/list" /> : null}
      </aside>

      <div>
        {tool ? (
          <>
            <h3 className="font-mono text-base font-semibold">{tool.name}</h3>
            {tool.description ? <p className="mt-1 text-sm text-gray-400">{tool.description}</p> : null}
            <div className="my-3 flex items-center gap-2 text-sm">
              <label className="flex items-center gap-2">
                <input type="checkbox" checked={raw} onChange={(e) => setRaw(e.target.checked)} />
                Raw JSON arguments
              </label>
            </div>
            {raw ? (
              <div>
                <label htmlFor="mcp-raw" className="sr-only">
                  Arguments JSON
                </label>
                <textarea
                  id="mcp-raw"
                  rows={8}
                  spellCheck={false}
                  value={rawText}
                  onChange={(e) => setRawText(e.target.value)}
                  className="w-full rounded-md border border-gray-600 bg-gray-800 p-2 font-mono text-xs"
                />
                {errors._raw ? <p className="text-xs text-red-400">{errors._raw}</p> : null}
              </div>
            ) : (
              <SchemaForm
                idPrefix="mcp"
                fields={fields}
                values={values}
                errors={errors}
                onChange={(n, v) => setValues((s) => ({ ...s, [n]: v }))}
              />
            )}
            <div className="mt-4">
              <Button onClick={run} disabled={busy || !active} data-testid="mcp-call">
                {busy ? "Calling…" : "Call tool"}
              </Button>
            </div>
            {out ? (
              <div className="mt-4">
                <h4 className="mb-1 text-xs font-semibold uppercase tracking-wide text-gray-400">
                  Tool output{out.isError ? " (tool reported an error)" : ""}
                </h4>
                <pre
                  className={`max-h-64 overflow-auto rounded p-3 text-xs ${
                    out.isError ? "bg-red-950 text-red-200" : "bg-gray-950 text-gray-200"
                  }`}
                  data-testid="mcp-output"
                >
                  {out.text || "(no text content)"}
                </pre>
              </div>
            ) : null}
            <CallView rec={rec} title="tools/call" />
          </>
        ) : (
          <p className="text-sm text-gray-400">Pick a tool on the left. Forms are generated from its input schema.</p>
        )}
      </div>
    </div>
  );
}
