/**
 * use-auth — token lifecycle hook for the ClawFT dashboard (WEFT-309).
 *
 * The clawft gateway hands out 24h bearer tokens via `POST /api/auth/token`.
 * `weft ui` opens the browser at `https://<host>/?token=<uuid>` so the user
 * never has to copy-paste; the dashboard then talks to the API with that
 * token via `Authorization: Bearer …`.
 *
 * Two security properties this hook enforces that the previous inlined
 * implementation in `api-client.ts` did not:
 *
 *   1. URL tokens are **single-use**. The first read consumes the value:
 *      we copy it to localStorage and immediately strip `?token` from the
 *      address bar via `history.replaceState`. Reload, share, or screenshot
 *      of the URL therefore never leaks the token.
 *
 *   2. Logout is **terminal**. We clear localStorage *and* set a session
 *      flag so a stale `?token` left in the address bar (e.g. from browser
 *      back-button) cannot silently re-auth the user. The flag lives only
 *      for the lifetime of the tab.
 *
 * The hook is the single source of truth for the bearer token. `api-client.ts`
 * still exposes `setAuthToken` / `getAuthToken` as a compatibility surface,
 * but they now delegate to the same storage that this hook owns.
 */

import { useCallback, useEffect, useState } from "react";

const STORAGE_KEY = "clawft-token";
const LOGOUT_FLAG = "clawft-logged-out";
const URL_PARAM = "token";

/**
 * Read the URL token (if any), persist it, and strip it from the address bar.
 *
 * Returns the token that was consumed, or `null` if the URL had none.
 * Callers may invoke this directly (e.g. from `main.tsx` boot) for the
 * non-React parts of the app, but most code should just use `useAuth()`.
 */
export function consumeUrlToken(): string | null {
  if (typeof window === "undefined") return null;

  // If the user explicitly logged out in this tab, do NOT silently re-auth
  // from a stale `?token=…` left over in the address bar.
  if (sessionStorage.getItem(LOGOUT_FLAG) === "1") return null;

  const params = new URLSearchParams(window.location.search);
  const token = params.get(URL_PARAM);
  if (!token) return null;

  // Persist before stripping so we never lose the value if `replaceState`
  // throws in some pathological browser sandbox.
  localStorage.setItem(STORAGE_KEY, token);

  // Single-use: remove the token from the URL so reload / share / screenshot
  // doesn't leak it. Preserve any other params the user (or `weft ui`) set.
  params.delete(URL_PARAM);
  const remaining = params.toString();
  const newUrl =
    window.location.pathname +
    (remaining ? `?${remaining}` : "") +
    window.location.hash;
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

export interface UseAuthValue {
  /** The current bearer token, or null if not authenticated. */
  token: string | null;
  /** True after the URL bootstrap pass has run at least once. */
  ready: boolean;
  /** Replace the token (e.g. after a fresh `POST /api/auth/token`). */
  setToken: (token: string) => void;
  /** Clear the token and prevent silent re-auth from a stale URL token. */
  logout: () => void;
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

  const logout = useCallback(() => {
    clearStoredToken();
    setTokenState(null);
  }, []);

  return { token, ready, setToken, logout };
}
