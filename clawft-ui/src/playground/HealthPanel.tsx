import { useCallback, useEffect, useState } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Button } from "../components/ui/button";
import { CallView } from "./CallView";
import { usePlayground } from "./context.tsx";
import type { CallRecord } from "./core.ts";

const SECTIONS = ["daemon", "kernel", "chain", "mcp", "channels", "providers", "token"] as const;

export function HealthPanel() {
  const { call, active, origin } = usePlayground();
  const [rec, setRec] = useState<CallRecord | null>(null);
  const [anon, setAnon] = useState<string | null>(null);

  const load = useCallback(async () => setRec(await call({ method: "GET", path: "/api/health" })), [call]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    (async () => {
      const r = await call({ method: "GET", path: "/api/health" });
      if (!cancelled) setRec(r);
    })();
    return () => {
      cancelled = true;
    };
  }, [call, active]);

  // The same route with no Authorization header: what the public sees.
  const loadAnon = async () => {
    try {
      const r = await fetch(`${origin}/api/health`, { cache: "no-store", referrerPolicy: "no-referrer" });
      setAnon(`${r.status}\n${JSON.stringify(await r.json(), null, 2)}`);
    } catch (e) {
      setAnon(e instanceof Error ? e.message : String(e));
    }
  };

  const doc = rec?.json as Record<string, unknown> | undefined;

  return (
    <div>
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={load} disabled={!active}>
          Refresh
        </Button>
        <Button size="sm" variant="outline" onClick={loadAnon}>
          Compare: no token
        </Button>
        <a className="text-sm text-blue-400 underline" href="/api/health">
          /api/health (public view)
        </a>
      </div>

      {doc ? (
        <>
          <p className="mb-3 text-sm text-gray-300" data-testid="health-summary">
            status <strong>{String(doc.status)}</strong>
            {doc.version ? <> · version {String(doc.version)}</> : null}
            {typeof doc.uptime_secs === "number" ? <> · uptime {doc.uptime_secs}s</> : null}
          </p>
          <div className="grid gap-3 md:grid-cols-2">
            {SECTIONS.filter((k) => doc[k] !== undefined).map((k) => (
              <Card key={k}>
                <CardHeader className="p-4 pb-2">
                  <CardTitle className="text-sm uppercase tracking-wide">{k}</CardTitle>
                </CardHeader>
                <CardContent className="p-4 pt-0">
                  <pre className="max-h-48 overflow-auto text-xs text-gray-300">{JSON.stringify(doc[k], null, 2)}</pre>
                </CardContent>
              </Card>
            ))}
          </div>
        </>
      ) : null}

      {anon ? (
        <div className="mt-4">
          <h4 className="mb-1 text-xs font-semibold uppercase tracking-wide text-gray-400">Without a token</h4>
          <pre className="rounded bg-gray-950 p-3 text-xs text-gray-200" data-testid="health-anon">
            {anon}
          </pre>
        </div>
      ) : null}

      <CallView rec={rec} title="health" />
    </div>
  );
}
