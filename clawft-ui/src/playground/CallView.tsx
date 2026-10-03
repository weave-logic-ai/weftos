import { useState } from "react";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { buildCurl, prettyBody, type CallRecord } from "./core.ts";
import { usePlayground } from "./context.tsx";

function statusVariant(status: number | null): "success" | "destructive" | "secondary" | "default" {
  if (status === null) return "destructive";
  if (status < 300) return "success";
  if (status < 400) return "default";
  return "destructive";
}

/** Request, response, status, latency and a copyable curl for one call. */
export function CallView({ rec, title }: { rec: CallRecord | null; title?: string }) {
  const { token, masked } = usePlayground();
  const [copied, setCopied] = useState(false);
  if (!rec) return null;

  const curl = buildCurl({ method: rec.method, url: rec.url, body: rec.requestBody }, token, masked);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(curl);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };

  return (
    <section className="mt-4 space-y-3" aria-label={title ?? "Call result"}>
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="font-mono text-gray-300">
          {rec.method} {new URL(rec.url).pathname}
          {new URL(rec.url).search}
        </span>
        <Badge variant={statusVariant(rec.status)} data-testid="call-status">
          {rec.status === null ? "no response" : `${rec.status} ${rec.statusText}`.trim()}
        </Badge>
        <Badge variant="secondary" data-testid="call-latency">
          {rec.latencyMs} ms
        </Badge>
      </div>

      <div>
        <div className="mb-1 flex items-center justify-between">
          <h4 className="text-xs font-semibold uppercase tracking-wide text-gray-400">
            curl {masked ? "(token read from $WEFT_TOKEN)" : "(token inlined)"}
          </h4>
          <Button size="sm" variant="outline" onClick={copy}>
            {copied ? "Copied" : "Copy curl"}
          </Button>
        </div>
        <pre
          className="overflow-x-auto rounded bg-gray-950 p-3 text-xs text-gray-200"
          data-testid="curl"
        >
          {curl}
        </pre>
      </div>

      {rec.requestBody ? (
        <div>
          <h4 className="mb-1 text-xs font-semibold uppercase tracking-wide text-gray-400">Request body</h4>
          <pre className="max-h-48 overflow-auto rounded bg-gray-950 p-3 text-xs text-gray-200">
            {rec.requestBody}
          </pre>
        </div>
      ) : null}

      <div>
        <h4 className="mb-1 text-xs font-semibold uppercase tracking-wide text-gray-400">Response</h4>
        <pre
          className="max-h-96 overflow-auto rounded bg-gray-950 p-3 text-xs text-gray-200"
          data-testid="response"
        >
          {rec.error ?? (prettyBody(rec) || "(empty body)")}
        </pre>
      </div>
    </section>
  );
}
