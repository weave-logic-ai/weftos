import { useEffect, useMemo, useState } from "react";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { CallView } from "./CallView";
import { usePlayground } from "./context.tsx";
import { buildPath, listOperations, schemaToFields, type CallRecord, type Operation } from "./core.ts";

const OPENAPI_PATH = "/api/openapi.json";

const METHOD_STYLE: Record<string, string> = {
  GET: "text-green-400",
  POST: "text-blue-400",
  PUT: "text-amber-400",
  PATCH: "text-amber-400",
  DELETE: "text-red-400",
};

const input =
  "w-full rounded-md border border-gray-600 bg-gray-800 px-2 py-1.5 text-sm text-gray-100 focus:border-blue-500 focus:outline-none";

export function RestPanel() {
  const { call, active } = usePlayground();
  const [ops, setOps] = useState<Operation[]>([]);
  const [specRec, setSpecRec] = useState<CallRecord | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [body, setBody] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [rec, setRec] = useState<CallRecord | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    (async () => {
      const r = await call({ method: "GET", path: OPENAPI_PATH });
      if (cancelled) return;
      setSpecRec(r);
      setOps(r.status === 200 ? listOperations(r.json) : []);
      setLoaded(true);
    })();
    return () => {
      cancelled = true;
    };
  }, [call, active]);

  const op = ops.find((o) => o.key === selected) ?? null;
  const visible = useMemo(() => {
    const f = filter.toLowerCase();
    return ops.filter((o) => o.key.toLowerCase().includes(f) || o.summary.toLowerCase().includes(f));
  }, [ops, filter]);
  const tags = useMemo(() => [...new Set(visible.map((o) => o.tag))], [visible]);

  const pick = (o: Operation) => {
    setSelected(o.key);
    setValues({});
    setProblem(null);
    setRec(null);
    // Seed the body with the schema's required keys so it is a valid start.
    const seed: Record<string, string> = {};
    for (const f of schemaToFields(o.bodySchema).filter((x) => x.required)) seed[f.name] = "";
    setBody(o.hasBody ? (Object.keys(seed).length > 0 ? JSON.stringify(seed, null, 2) : "{}") : "");
  };

  const send = async () => {
    if (!op) return;
    const built = buildPath(op, values);
    if (built.missing.length > 0) {
      setProblem(`missing: ${built.missing.join(", ")}`);
      return;
    }
    if (op.hasBody && body.trim()) {
      try {
        JSON.parse(body);
      } catch {
        setProblem("body must be valid JSON");
        return;
      }
    }
    setProblem(null);
    setBusy(true);
    try {
      setRec(await call({ method: op.method, path: built.path, body: op.hasBody ? body.trim() : undefined }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-6 md:grid-cols-[22rem_1fr]">
      <aside aria-label="REST operations">
        <input
          type="search"
          aria-label="Filter operations"
          placeholder="Filter operations"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          className={`${input} mb-2`}
        />
        <p className="mb-2 text-xs text-gray-500" data-testid="rest-op-count">
          {loaded ? `${ops.length} operations from ${OPENAPI_PATH}` : `Loading ${OPENAPI_PATH}…`}
        </p>
        <div className="max-h-[32rem] overflow-auto">
          {tags.map((tag) => (
            <div key={tag} className="mb-2">
              <h3 className="px-2 text-xs font-semibold uppercase tracking-wide text-gray-500">{tag}</h3>
              <ul>
                {visible
                  .filter((o) => o.tag === tag)
                  .map((o) => (
                    <li key={o.key}>
                      <button
                        type="button"
                        onClick={() => pick(o)}
                        aria-current={o.key === selected}
                        className={`flex w-full gap-2 truncate rounded px-2 py-1 text-left font-mono text-xs ${
                          o.key === selected ? "bg-blue-900 text-white" : "text-gray-300 hover:bg-gray-800"
                        }`}
                      >
                        <span className={`w-12 shrink-0 ${METHOD_STYLE[o.method] ?? ""}`}>{o.method}</span>
                        <span className="truncate">{o.path}</span>
                      </button>
                    </li>
                  ))}
              </ul>
            </div>
          ))}
        </div>
        {loaded && ops.length === 0 ? <CallView rec={specRec} title="openapi.json" /> : null}
      </aside>

      <div>
        {op ? (
          <>
            <h3 className="font-mono text-base font-semibold">
              <span className={METHOD_STYLE[op.method]}>{op.method}</span> {op.path}
            </h3>
            {op.summary ? <p className="mt-1 text-sm text-gray-400">{op.summary}</p> : null}
            {op.method !== "GET" ? (
              <Badge variant="destructive" className="mt-2">
                changes state; the token has full scope
              </Badge>
            ) : null}
            {!op.tryable ? (
              <p className="mt-3 text-sm text-amber-300">
                This route streams or upgrades the connection; use a client that supports it (the curl below is a
                starting point).
              </p>
            ) : null}

            <div className="mt-3 space-y-3">
              {op.params.map((p) => (
                <div key={`${p.in}:${p.name}`}>
                  <label htmlFor={`rest-${p.name}`} className="mb-1 flex items-baseline gap-2 text-sm font-medium">
                    <span className="font-mono">{p.name}</span>
                    <span className="text-xs font-normal text-gray-500">{p.in}</span>
                    {p.required ? <span className="text-xs text-red-400">required</span> : null}
                  </label>
                  <input
                    id={`rest-${p.name}`}
                    className={input}
                    value={values[p.name] ?? ""}
                    onChange={(e) => setValues((s) => ({ ...s, [p.name]: e.target.value }))}
                  />
                  {p.description ? <p className="mt-1 text-xs text-gray-500">{p.description}</p> : null}
                </div>
              ))}
              {op.hasBody ? (
                <div>
                  <label htmlFor="rest-body" className="mb-1 block text-sm font-medium">
                    Request body <span className="text-xs font-normal text-gray-500">JSON</span>
                  </label>
                  <textarea
                    id="rest-body"
                    rows={6}
                    spellCheck={false}
                    value={body}
                    onChange={(e) => setBody(e.target.value)}
                    className={`${input} font-mono text-xs`}
                  />
                </div>
              ) : null}
            </div>
            {problem ? <p className="mt-2 text-xs text-red-400">{problem}</p> : null}
            <div className="mt-4">
              <Button onClick={send} disabled={busy || !active || !op.tryable} data-testid="rest-send">
                {busy ? "Sending…" : "Send request"}
              </Button>
            </div>
            <CallView rec={rec} title="REST call" />
          </>
        ) : (
          <p className="text-sm text-gray-400">Pick an operation on the left. The list is generated from the OpenAPI document.</p>
        )}
      </div>
    </div>
  );
}
