import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { CallView } from "./CallView";
import { HealthPanel } from "./HealthPanel";
import { McpPanel } from "./McpPanel";
import { RestPanel } from "./RestPanel";
import { PlaygroundProvider, type PlaygroundCtx } from "./context.tsx";
import { callApi, remaining, takeFragmentToken, type CallRecord, type CallSpec, type FetchLike } from "./core.ts";

type Status = "none" | "checking" | "active" | "rejected" | "revoked";
type Tab = "health" | "mcp" | "rest";

interface TokenInfo {
  id: string;
  label: string;
  expiresAt: string | null;
}

/**
 * Read the token out of the URL fragment exactly once, at load, and remove
 * it from the address bar. It is never written to storage; a reload needs
 * the link again (ADR-102 D2).
 */
let booted: string | null | undefined;
function bootToken(): string | null {
  if (booted !== undefined) return booted;
  const { token, rest } = takeFragmentToken(window.location.hash);
  if (token) {
    const url = window.location.pathname + window.location.search + (rest ? `#${rest}` : "");
    window.history.replaceState(null, "", url);
  }
  booted = token;
  return token;
}

const TABS: Array<{ id: Tab; label: string }> = [
  { id: "health", label: "Health" },
  { id: "mcp", label: "MCP tools" },
  { id: "rest", label: "REST" },
];

export function PlaygroundApp() {
  const origin = window.location.origin;
  const tokenRef = useRef<string | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [status, setStatus] = useState<Status>("none");
  const [info, setInfo] = useState<TokenInfo | null>(null);
  const [masked, setMasked] = useState(true);
  const [tab, setTab] = useState<Tab>("health");
  const [now, setNow] = useState(() => Date.now());
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmRec, setConfirmRec] = useState<CallRecord | null>(null);
  const [paste, setPaste] = useState("");

  const call = useCallback(async (spec: CallSpec): Promise<CallRecord> => {
    const t = tokenRef.current;
    if (!t) throw new Error("no token");
    const rec = await callApi(window.fetch.bind(window) as unknown as FetchLike, window.location.origin, spec, t);
    // A 401 on anything but the revoke-confirmation means the token is dead.
    if (rec.status === 401) setStatus((s) => (s === "revoked" ? s : "rejected"));
    return rec;
  }, []);

  const connect = useCallback(
    async (t: string) => {
      tokenRef.current = t;
      setToken(t);
      setStatus("checking");
      setNotice(null);
      const rec = await call({ method: "GET", path: "/api/health" });
      const meta = (rec.json as { token?: { id?: string; label?: string; expires_at?: string } } | undefined)?.token;
      if (meta?.id) {
        setInfo({ id: meta.id, label: meta.label ?? "", expiresAt: meta.expires_at ?? null });
        setStatus("active");
      } else {
        // The public health view has no `token` section: the token is not valid.
        setStatus("rejected");
        setNotice("The gateway did not recognise this token. It may be expired, revoked, or mistyped.");
      }
    },
    [call],
  );

  useEffect(() => {
    const t = bootToken();
    if (!t) return;
    // Deferred so no state is set synchronously inside the effect.
    void Promise.resolve().then(() => connect(t));
  }, [connect]);

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  const left = useMemo(() => remaining(info?.expiresAt, now), [info, now]);
  const expired = status === "active" && !!left?.expired;
  const active = status === "active" && !expired;

  const revoke = async () => {
    if (!window.confirm("Revoke this token? It stops working for every client, not just this page.")) return;
    const rec = await call({ method: "POST", path: "/api/auth/revoke" });
    if (rec.status === 204 || rec.status === 200) {
      setStatus("revoked");
      setNotice("Token revoked on the daemon. Use “Call again” to see the gateway refuse it.");
    } else {
      setNotice(`Revoke failed: ${rec.status ?? rec.error}. The token is still live.`);
    }
  };

  const callAgain = async () => setConfirmRec(await call({ method: "GET", path: "/api/openapi.json" }));

  const ctx: PlaygroundCtx = { call, token, masked, origin, active };

  return (
    <PlaygroundProvider value={ctx}>
      <div className="mx-auto max-w-6xl px-4 py-6">
        <header className="mb-6 flex flex-wrap items-start justify-between gap-4">
          <div>
            <h1 className="text-2xl font-semibold">WeftOS API Playground</h1>
            <p className="mt-1 text-sm text-gray-400">
              Try the gateway&apos;s REST routes and MCP tools with the token from <code>weft token issue</code>.
            </p>
          </div>

          <section aria-label="Token" className="rounded-lg border border-gray-700 bg-gray-800 p-3 text-sm">
            {status === "none" ? (
              <span className="text-gray-400">No token</span>
            ) : (
              <div className="flex flex-wrap items-center gap-3">
                <Badge
                  variant={active ? "success" : "destructive"}
                  data-testid="token-state"
                >
                  {expired ? "expired" : status}
                </Badge>
                {info ? (
                  <span className="font-mono text-xs text-gray-400" data-testid="token-id">
                    id {info.id}
                    {info.label ? ` · ${info.label}` : ""}
                  </span>
                ) : null}
                {left && status === "active" ? (
                  <span
                    className={`font-mono ${left.secs < 60 ? "text-amber-400" : "text-gray-200"}`}
                    data-testid="token-countdown"
                    aria-live="off"
                  >
                    {left.label}
                  </span>
                ) : null}
                <Button size="sm" variant="destructive" onClick={revoke} disabled={!active} data-testid="revoke">
                  Revoke
                </Button>
              </div>
            )}
            <label className="mt-2 flex items-center gap-2 text-xs text-gray-400">
              <input type="checkbox" checked={masked} onChange={(e) => setMasked(e.target.checked)} />
              Mask token in curl
            </label>
          </section>
        </header>

        {notice ? (
          <p role="status" className="mb-4 rounded border border-amber-700 bg-amber-950 p-3 text-sm text-amber-200">
            {notice}
          </p>
        ) : null}

        {status === "none" ? (
          <div className="rounded-lg border border-gray-700 bg-gray-800 p-6">
            <h2 className="text-lg font-semibold">Open this page from a token link</h2>
            <p className="mt-2 text-sm text-gray-300">
              Run <code>weft ui</code> (or <code>weft token issue</code>) and open the link it prints. The token rides
              in the URL fragment, so it is never sent to the server, and this page keeps it in memory only. Reloading
              needs the link again.
            </p>
            <form
              className="mt-4 flex gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                const t = paste.trim();
                setPaste("");
                if (t) void connect(t);
              }}
            >
              <label htmlFor="paste-token" className="sr-only">
                Token
              </label>
              <input
                id="paste-token"
                type="password"
                autoComplete="off"
                placeholder="Or paste a token (kept in memory only)"
                value={paste}
                onChange={(e) => setPaste(e.target.value)}
                className="w-full max-w-md rounded-md border border-gray-600 bg-gray-900 px-2 py-1.5 text-sm"
              />
              <Button type="submit" disabled={!paste.trim()}>
                Connect
              </Button>
            </form>
            <p className="mt-4 text-sm text-gray-400">
              Without a token you can only see the public health view:{" "}
              <a className="text-blue-400 underline" href="/api/health">
                /api/health
              </a>
              .
            </p>
          </div>
        ) : (
          <>
            {(status === "rejected" || status === "revoked" || expired) && (
              <div className="mb-4 flex flex-wrap items-center gap-3 rounded border border-red-800 bg-red-950 p-3 text-sm text-red-200">
                <span>
                  {status === "revoked"
                    ? "This token is revoked."
                    : expired
                      ? "This token has expired."
                      : "The gateway rejected this token."}{" "}
                  Calls are disabled; issue a new one with <code>weft token issue</code>.
                </span>
                <Button size="sm" variant="outline" onClick={callAgain} data-testid="call-again">
                  Call again
                </Button>
              </div>
            )}
            {confirmRec ? (
              <div data-testid="after-revoke">
                <CallView rec={confirmRec} title="After revoke" />
              </div>
            ) : null}

            <nav className="mb-4 flex gap-1 border-b border-gray-700" role="tablist" aria-label="Playground sections">
              {TABS.map((t) => (
                <button
                  key={t.id}
                  type="button"
                  role="tab"
                  aria-selected={tab === t.id}
                  onClick={() => setTab(t.id)}
                  className={`-mb-px border-b-2 px-4 py-2 text-sm font-medium ${
                    tab === t.id ? "border-blue-500 text-white" : "border-transparent text-gray-400 hover:text-gray-200"
                  }`}
                >
                  {t.label}
                </button>
              ))}
            </nav>

            {status === "checking" ? <p className="text-sm text-gray-400">Checking token…</p> : null}
            {status !== "checking" && tab === "health" ? <HealthPanel /> : null}
            {status !== "checking" && tab === "mcp" ? <McpPanel /> : null}
            {status !== "checking" && tab === "rest" ? <RestPanel /> : null}
          </>
        )}
      </div>
    </PlaygroundProvider>
  );
}
