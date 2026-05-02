/**
 * Browser configuration component for WASM mode.
 *
 * Shown on first launch in WASM mode. Handles:
 * - API key input with Web Crypto encryption before IndexedDB storage
 * - Provider selection (Anthropic, OpenAI, Ollama, LM Studio, custom)
 * - CORS proxy URL configuration for providers without browser CORS support
 * - Model selection
 */

import { useState, useCallback, useEffect } from "react";
import { validateCorsProxyUrl } from "../../lib/url-validator.ts";

// ---------------------------------------------------------------------------
// Provider and model configuration
// ---------------------------------------------------------------------------

interface ProviderOption {
  value: string;
  label: string;
  browserDirect: boolean;
}

const PROVIDERS: ProviderOption[] = [
  {
    value: "anthropic",
    label: "Anthropic (direct browser access)",
    browserDirect: true,
  },
  {
    value: "openai",
    label: "OpenAI (requires CORS proxy)",
    browserDirect: false,
  },
  {
    value: "ollama",
    label: "Ollama (local, http://localhost:11434)",
    browserDirect: true,
  },
  {
    value: "lmstudio",
    label: "LM Studio (local, http://localhost:1234)",
    browserDirect: true,
  },
  {
    value: "custom",
    label: "Custom OpenAI-compatible",
    browserDirect: false,
  },
];

const MODELS: Record<string, string[]> = {
  anthropic: [
    "claude-sonnet-4-5-20250929",
    "claude-opus-4-20250514",
    "claude-haiku-3-5-20241022",
  ],
  openai: ["gpt-4o", "gpt-4o-mini", "o3-mini"],
  ollama: ["llama3.3", "mistral", "codellama"],
  lmstudio: ["loaded-model"],
  custom: [],
};

// ---------------------------------------------------------------------------
// IndexedDB helpers
// ---------------------------------------------------------------------------

async function openConfigDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("clawft-config", 1);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains("config")) {
        db.createObjectStore("config");
      }
      if (!db.objectStoreNames.contains("crypto-keys")) {
        db.createObjectStore("crypto-keys");
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

/**
 * Encrypt API key using Web Crypto AES-256-GCM.
 * The CryptoKey is stored non-extractably in IndexedDB.
 * Returns base64-encoded IV + ciphertext.
 */
async function encryptApiKey(apiKey: string): Promise<string> {
  const key = await crypto.subtle.generateKey(
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );

  const iv = crypto.getRandomValues(new Uint8Array(12));
  const encoded = new TextEncoder().encode(apiKey);
  const ciphertext = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv },
    key,
    encoded,
  );

  // Store the CryptoKey in IndexedDB (non-extractable, stays in browser)
  const db = await openConfigDb();
  const tx = db.transaction("crypto-keys", "readwrite");
  tx.objectStore("crypto-keys").put(key, "api-key-encryption");
  await new Promise<void>((resolve) => {
    tx.oncomplete = () => resolve();
  });

  // Return IV + ciphertext as base64
  const combined = new Uint8Array(
    iv.length + new Uint8Array(ciphertext).length,
  );
  combined.set(iv);
  combined.set(new Uint8Array(ciphertext), iv.length);
  return btoa(String.fromCharCode(...combined));
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

interface BrowserConfigProps {
  onConfigured: (config: Record<string, unknown>) => void;
}

export function BrowserConfig({ onConfigured }: BrowserConfigProps) {
  const [provider, setProvider] = useState("anthropic");
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("");
  const [corsProxy, setCorsProxy] = useState("");
  const [customBaseUrl, setCustomBaseUrl] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selectedProvider = PROVIDERS.find((p) => p.value === provider);
  const needsProxy = !selectedProvider?.browserDirect;

  // Re-validate the proxy URL whenever the input changes (covers the legacy-
  // stored values surfaced after `useEffect` below restores them).
  useEffect(() => {
    if (!needsProxy) {
      setError(null);
      return;
    }
    const result = validateCorsProxyUrl(corsProxy);
    if (!result.valid) {
      setError(result.error ?? "Invalid CORS proxy URL.");
    } else if (error?.startsWith("HTTP CORS proxy") || error?.startsWith("Unsupported")) {
      // Clear the validator-specific error once the user fixes it.
      setError(null);
    }
    // We intentionally exclude `error` from deps to avoid clear/set ping-pong.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [corsProxy, needsProxy]);

  const handleSave = useCallback(async () => {
    setSaving(true);
    setError(null);

    try {
      // Hard-stop on an invalid proxy URL so we never write a non-HTTPS public
      // proxy to IndexedDB. Keeps the HTTPS-in-production rule enforced even
      // if a user races the save button before the effect has run.
      if (needsProxy) {
        const result = validateCorsProxyUrl(corsProxy);
        if (!result.valid) {
          setError(result.error ?? "Invalid CORS proxy URL.");
          setSaving(false);
          return;
        }
      }

      const encryptedKey = apiKey ? await encryptApiKey(apiKey) : undefined;

      const config: Record<string, unknown> = {
        defaults: {
          model:
            model || (MODELS[provider] ?? [])[0] || "claude-sonnet-4-5-20250929",
          max_tokens: 4096,
        },
        providers: {
          [provider]: {
            api_key: apiKey,
            base_url:
              provider === "ollama"
                ? "http://localhost:11434/v1"
                : provider === "lmstudio"
                  ? "http://localhost:1234/v1"
                  : customBaseUrl || undefined,
            browser_direct: selectedProvider?.browserDirect ?? false,
            cors_proxy:
              needsProxy && corsProxy ? corsProxy : undefined,
          },
        },
        routing: { strategy: "static" },
      };

      // Store encrypted config in IndexedDB for persistence
      const db = await openConfigDb();
      const tx = db.transaction("config", "readwrite");
      tx.objectStore("config").put(
        {
          ...config,
          providers: {
            [provider]: { encrypted_key: encryptedKey },
          },
        },
        "current",
      );
      await new Promise<void>((resolve) => {
        tx.oncomplete = () => resolve();
      });

      onConfigured(config);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to save configuration",
      );
    } finally {
      setSaving(false);
    }
  }, [
    provider,
    apiKey,
    model,
    corsProxy,
    customBaseUrl,
    selectedProvider,
    needsProxy,
    onConfigured,
  ]);

  return (
    <div className="max-w-lg mx-auto mt-12 space-y-6">
      <div className="rounded-lg border border-gray-200 bg-white shadow-sm dark:border-gray-700 dark:bg-gray-800">
        <div className="border-b border-gray-200 p-6 dark:border-gray-700">
          <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100">
            Browser Mode Setup
          </h2>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">
            Configure your LLM provider. Your API key is encrypted and stored
            locally in your browser. It is sent directly from your browser to the
            provider -- no server involved.
          </p>
        </div>

        <div className="space-y-4 p-6">
          {/* Provider selector */}
          <div className="space-y-2">
            <label className="text-sm font-medium text-gray-700 dark:text-gray-300">
              Provider
            </label>
            <select
              value={provider}
              onChange={(e) => {
                setProvider(e.target.value);
                setModel("");
              }}
              className="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm dark:border-gray-600 dark:bg-gray-700 dark:text-gray-100"
            >
              {PROVIDERS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </select>
          </div>

          {/* API key input (not shown for local providers) */}
          {provider !== "ollama" && provider !== "lmstudio" && (
            <div className="space-y-2">
              <label className="text-sm font-medium text-gray-700 dark:text-gray-300">
                API Key
              </label>
              <input
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder="sk-..."
                className="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm dark:border-gray-600 dark:bg-gray-700 dark:text-gray-100"
              />
              <p className="text-xs text-gray-400 dark:text-gray-500">
                Your key is sent directly from your browser to {provider}. Use a
                key with restricted permissions for browser usage.
              </p>
            </div>
          )}

          {/* Model selector */}
          <div className="space-y-2">
            <label className="text-sm font-medium text-gray-700 dark:text-gray-300">
              Model
            </label>
            <select
              value={model}
              onChange={(e) => setModel(e.target.value)}
              className="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm dark:border-gray-600 dark:bg-gray-700 dark:text-gray-100"
            >
              <option value="">Select model</option>
              {(MODELS[provider] ?? []).map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </div>

          {/* CORS proxy URL (for providers that need it) */}
          {needsProxy && (
            <div className="space-y-2">
              <label className="text-sm font-medium text-gray-700 dark:text-gray-300">
                CORS Proxy URL
              </label>
              <input
                value={corsProxy}
                onChange={(e) => setCorsProxy(e.target.value)}
                placeholder="https://your-proxy.example.com/"
                className="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm dark:border-gray-600 dark:bg-gray-700 dark:text-gray-100"
              />
              <p className="text-xs text-gray-400 dark:text-gray-500">
                {provider} does not support direct browser access. Route API
                calls through a CORS proxy that adds the required headers.
              </p>
            </div>
          )}

          {/* Custom base URL */}
          {provider === "custom" && (
            <div className="space-y-2">
              <label className="text-sm font-medium text-gray-700 dark:text-gray-300">
                Base URL
              </label>
              <input
                value={customBaseUrl}
                onChange={(e) => setCustomBaseUrl(e.target.value)}
                placeholder="https://api.example.com/v1"
                className="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm dark:border-gray-600 dark:bg-gray-700 dark:text-gray-100"
              />
            </div>
          )}

          {/* Error display */}
          {error && (
            <div className="rounded-md bg-red-50 p-3 text-sm text-red-700 dark:bg-red-900/20 dark:text-red-400">
              {error}
            </div>
          )}

          {/* Save button */}
          <button
            onClick={handleSave}
            disabled={saving}
            className="w-full rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50 dark:bg-blue-500 dark:hover:bg-blue-600"
          >
            {saving ? "Saving..." : "Save and Start"}
          </button>
        </div>
      </div>
    </div>
  );
}
