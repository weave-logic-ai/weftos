/**
 * React context provider for the current BackendAdapter.
 *
 * ModeProvider auto-detects mode from:
 *  1. URL search param ?mode=wasm|axum|mock
 *  2. VITE_BACKEND_MODE env var
 *  3. Default: axum
 *
 * Shows a loading screen during WASM initialization with progress bar.
 */

import {
  useState,
  useEffect,
  useMemo,
  type ReactNode,
} from "react";
import type { BackendAdapter } from "./backend-adapter.ts";
import { AxumAdapter } from "./adapters/axum-adapter.ts";
import { WasmAdapter } from "./adapters/wasm-adapter.ts";
import type { LoadProgress } from "./wasm-loader.ts";
import { ModeContext } from "./mode-store.ts";
import type { ModeContextValue } from "./mode-store.ts";
import { consumeUrlToken, readStoredToken } from "./use-auth.ts";

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

interface ModeProviderProps {
  children: ReactNode;
}

/**
 * Determines backend mode from environment variables and URL params,
 * then initializes the appropriate adapter.
 */
export function ModeProvider({ children }: ModeProviderProps) {
  const [adapter, setAdapter] = useState<BackendAdapter | null>(null);
  const [loadProgress, setLoadProgress] = useState<LoadProgress | null>(null);

  useEffect(() => {
    // Consume any single-use URL token first (WEFT-309). This persists the
    // token to localStorage and strips it from the address bar before any
    // adapter reads `readStoredToken()` below.
    consumeUrlToken();

    // Determine mode: URL param > env var > default "axum"
    const params = new URLSearchParams(window.location.search);
    const urlMode = params.get("mode");
    const envMode = import.meta.env.VITE_BACKEND_MODE as string | undefined;
    const mode = urlMode ?? envMode ?? "axum";

    const apiUrl =
      (import.meta.env.VITE_API_URL as string | undefined) ?? "";
    const wsUrl = apiUrl
      ? apiUrl.replace(/^http/, "ws") + "/ws"
      : `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/ws`;

    async function initAdapter() {
      if (mode === "wasm") {
        const wasmAdapter = new WasmAdapter(
          "/clawft_wasm.js",
          (phase, pct) =>
            setLoadProgress({
              phase: phase as LoadProgress["phase"],
              percent: pct,
              message: phase,
            }),
        );
        await wasmAdapter.init();
        setAdapter(wasmAdapter);
      } else if (mode === "auto") {
        // Try Axum first, fall back to WASM
        try {
          const healthUrl = apiUrl
            ? `${apiUrl}/api/health`
            : "/api/health";
          const response = await fetch(healthUrl, { method: "GET" });
          if (response.ok) {
            const token = readStoredToken() ?? undefined;
            const axumAdapter = new AxumAdapter(apiUrl, wsUrl, token);
            await axumAdapter.init();
            setAdapter(axumAdapter);
          } else {
            throw new Error("Axum not reachable");
          }
        } catch {
          const wasmAdapter = new WasmAdapter(
            "/clawft_wasm.js",
            (phase, pct) =>
              setLoadProgress({
                phase: phase as LoadProgress["phase"],
                percent: pct,
                message: phase,
              }),
          );
          await wasmAdapter.init();
          setAdapter(wasmAdapter);
        }
      } else {
        // Default: Axum mode
        const token = readStoredToken() ?? undefined;
        const axumAdapter = new AxumAdapter(apiUrl, wsUrl, token);
        await axumAdapter.init();
        setAdapter(axumAdapter);
      }
    }

    initAdapter().catch(console.error);
  }, []);

  const value = useMemo<ModeContextValue | null>(() => {
    if (!adapter) return null;
    return {
      adapter,
      mode: adapter.mode,
      capabilities: adapter.capabilities,
      isReady: adapter.capabilities.ready,
      loadProgress,
    };
  }, [adapter, loadProgress]);

  if (!value) {
    return (
      <div className="flex items-center justify-center h-screen bg-gray-50 dark:bg-gray-900">
        <div className="text-center space-y-4">
          <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600 mx-auto" />
          <p className="text-gray-500 dark:text-gray-400">
            {loadProgress?.message ?? "Connecting to backend..."}
          </p>
          {loadProgress && (
            <div className="w-64 bg-gray-200 dark:bg-gray-700 rounded-full h-2">
              <div
                className="bg-blue-600 h-2 rounded-full transition-all duration-300"
                style={{ width: `${loadProgress.percent}%` }}
              />
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <ModeContext.Provider value={value}>{children}</ModeContext.Provider>
  );
}
