/**
 * use-auth — token lifecycle hook for the ClawFT dashboard (WEFT-309).
 *
 * The clawft gateway hands out 24h bearer tokens via `POST /api/auth/token`.
 * `weft ui` opens the browser at `https://<host>/#token=<uuid>` so the user
 * never has to copy-paste; the dashboard then talks to the API with that
 * token via `Authorization: Bearer …`.
 *
 * **Security properties:**
 *
 *   1. **URL tokens travel in the fragment, never the query string** (WEFT-569).
 *      Browsers do not include the URL fragment in HTTP requests, so the
 *      token cannot leak to upstream proxies, web-server access logs
 *      (e.g. nginx `$request_uri`), or `Referer` headers when the SPA
 *      loads third-party assets. Older `?token=` query-string callers are
 *      no longer honoured — the only accepted in-band transport is the
 *      fragment.
 *
 *   2. **URL tokens are single-use**. The first read consumes the value:
 *      we copy it to localStorage and immediately strip `#token=` from the
 *      address bar via `history.replaceState`. Reload, share, or screenshot
 *      of the URL therefore never leaks the token.
 *
 *   3. **Logout is terminal AND server-acknowledged** (WEFT-570). We POST
 *      `/api/auth/revoke` with the current bearer so the server-side
 *      `TokenStore::revoke_token` marks the entry. Only after the revoke
 *      attempt do we clear localStorage and arm the per-tab logout flag.
 *      Network failures during revoke fall through to local clear so the
 *      user is never stuck "looking logged in" — but the server is the
 *      authoritative source of truth.
 *
 * The hook is the single source of truth for the bearer token. `api-client.ts`
 * still exposes `setAuthToken` / `getAuthToken` as a compatibility surface,
 * but they now delegate to the same storage that this hook owns.
 */

import { useCallback, useEffect, useState } from "react";

const STORAGE_KEY = "clawft-token";
const LOGOUT_FLAG = "clawft-logged-out";
const URL_FRAGMENT_KEY = "token";
const REVOKE_PATH = "/api/auth/revoke";

/**
 * Read the URL fragment token (if any), persist it, and strip it from the
 * address bar.
 *
 * Returns the token that was consumed, or `null` if the URL had none.
 * Callers may invoke this directly (e.g. from `main.tsx` boot) for the
 * non-React parts of the app, but most code should just use `useAuth()`.
 *
 * The token MUST arrive via `#token=<value>` URL fragment — query-string
 * `?token=` is not honoured because the query string ends up in HTTP
 * server access logs and browser-extension `Referer` propagation.
 * Fragments stay client-side. WEFT-569.
 */
export function consumeUrlToken(): string | null {
  if (typeof window === "undefined") return null;

  // If the user explicitly logged out in this tab, do NOT silently re-auth
  // from a stale `#token=…` left over in the address bar.
  if (sessionStorage.getItem(LOGOUT_FLAG) === "1") return null;

  // The fragment can carry one of two shapes:
  //   - `#token=<uuid>` (canonical, preserves no other state)
  //   - `#token=<uuid>&foo=bar` (token plus a hash-route fragment)
  // We treat the fragment as URLSearchParams-shaped after stripping the
  // leading `#`, which matches `URLSearchParams`' tolerance of `&`.
  const rawHash = window.location.hash.replace(/^#/, "");
  if (!rawHash) return null;

  const params = new URLSearchParams(rawHash);
  const token = params.get(URL_FRAGMENT_KEY);
  if (!token) return null;

  // Persist before stripping so we never lose the value if `replaceState`
  // throws in some pathological browser sandbox.
  localStorage.setItem(STORAGE_KEY, token);

  // Single-use: remove the token from the fragment. Preserve any other
  // hash-route state the SPA (or its router) cares about.
  params.delete(URL_FRAGMENT_KEY);
  const remaining = params.toString();
  const newUrl =
    window.location.pathname +
    window.location.search +
    (remaining ? `#${remaining}` : "");
  try {
    window.history.replaceState(null, "", newUrl);
  } catch {
    // History API blocked (e.g. about: pages). The token is already
    // persisted; we simply leave the URL alone.
  }

  return token;
}

/** Read the persisted token without consuming the URL. */
export function readStoredToken(): string | null {
  if (typeof window === "undefined") return null;
  return localStorage.getItem(STORAGE_KEY);
}

/** Persist a token (e.g. after `POST /api/auth/token`). */
export function writeStoredToken(token: string): void {
  if (typeof window === "undefined") return;
  // A successful explicit token write counts as "logged in again" — clear
  // the per-tab logout flag so subsequent URL tokens are re-honored.
  sessionStorage.removeItem(LOGOUT_FLAG);
  localStorage.setItem(STORAGE_KEY, token);
}

/** Clear the persisted token and arm the per-tab logout latch. */
export function clearStoredToken(): void {
  if (typeof window === "undefined") return;
  localStorage.removeItem(STORAGE_KEY);
  sessionStorage.setItem(LOGOUT_FLAG, "1");
}

/**
 * POST `/api/auth/revoke` with the given bearer so the server-side
 * `TokenStore::revoke_token` marks the entry. Best-effort — a network
 * failure falls through to a local clear so the user is never stuck
 * "looking logged in", but the server is the source of truth and a
 * successful revoke prevents the token from being reused even if it
 * leaked elsewhere. WEFT-570.
 *
 * Exported so non-hook code paths (e.g. an explicit `weft ui logout`
 * CLI handoff) can call it directly. The hook calls it from `logout()`.
 */
export async function revokeServerToken(token: string): Promise<void> {
  if (typeof fetch === "undefined") return;
  try {
    await fetch(REVOKE_PATH, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
      },
      // No body; the server reads the token from Authorization. Keep
      // credentials default ("same-origin") so the call works in both
      // axum same-origin mode and tauri webview mode.
      keepalive: true,
    });
  } catch {
    // Swallow — local clear still proceeds. The user-visible logout is
    // not blocked by transient network issues.
  }
}

export interface UseAuthValue {
  /** The current bearer token, or null if not authenticated. */
  token: string | null;
  /** True after the URL bootstrap pass has run at least once. */
  ready: boolean;
  /** Replace the token (e.g. after a fresh `POST /api/auth/token`). */
  setToken: (token: string) => void;
  /**
   * Revoke server-side, then clear local state and prevent silent re-auth
   * from a stale URL token. WEFT-570 — server revoke is awaited (with a
   * keepalive request) so the user can rely on logout actually
   * invalidating the bearer, not just hiding it from the SPA.
   */
  logout: () => Promise<void>;
}

/**
 * React hook exposing the token lifecycle. Safe to call from any component;
 * the URL bootstrap runs exactly once per page load thanks to
 * `consumeUrlToken`'s side-effect being idempotent (the URL only has the
 * token on first navigation).
 */
export function useAuth(): UseAuthValue {
  const [token, setTokenState] = useState<string | null>(() => readStoredToken());
  const [ready, setReady] = useState(false);

  useEffect(() => {
    const consumed = consumeUrlToken();
    if (consumed) {
      setTokenState(consumed);
    } else {
      setTokenState(readStoredToken());
    }
    setReady(true);
  }, []);

  const setToken = useCallback((next: string) => {
    writeStoredToken(next);
    setTokenState(next);
  }, []);

  const logout = useCallback(async () => {
    // Snapshot current token before clearing so the revoke call can use it.
    // Read directly from storage (not the React state) so stale closures
    // can't accidentally revoke a different bearer.
    const current = readStoredToken();
    if (current) {
      await revokeServerToken(current);
    }
    clearStoredToken();
    setTokenState(null);
  }, []);

  return { token, ready, setToken, logout };
}
