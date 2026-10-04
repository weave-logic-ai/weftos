import { create } from "zustand";
import { api } from "../lib/api-client";
import type { ConfigData } from "../lib/types";

interface ConfigStore {
  config: ConfigData | null;
  draft: ConfigData | null;
  activeTab: string;
  loading: boolean;
  saving: boolean;
  hasChanges: boolean;
  setActiveTab: (tab: string) => void;
  fetchConfig: () => Promise<void>;
  updateDraft: (path: string, value: unknown) => void;
  saveConfig: () => Promise<void>;
  resetDraft: () => void;
}

function deepClone<T>(obj: T): T {
  return JSON.parse(JSON.stringify(obj));
}

function setNestedValue(
  obj: Record<string, unknown>,
  path: string,
  value: unknown,
): Record<string, unknown> {
  const clone = deepClone(obj);
  const parts = path.split(".");
  // Refuse prototype keys so a crafted path cannot pollute Object.prototype.
  if (parts.some((p) => p === "__proto__" || p === "prototype" || p === "constructor")) {
    return clone;
  }
  let current: Record<string, unknown> = clone;
  for (let i = 0; i < parts.length - 1; i++) {
    const part = parts[i];
    if (!Object.prototype.hasOwnProperty.call(current, part) || typeof current[part] !== "object" || current[part] === null) {
      current[part] = {};
    }
    current = current[part] as Record<string, unknown>;
  }
  current[parts[parts.length - 1]] = value;
  return clone;
}

export const useConfigStore = create<ConfigStore>((set, get) => ({
  config: null,
  draft: null,
  activeTab: "general",
  loading: false,
  saving: false,
  hasChanges: false,
  setActiveTab: (tab) => set({ activeTab: tab }),
  fetchConfig: async () => {
    set({ loading: true });
    try {
      const config = await api.config.get();
      set({ config, draft: deepClone(config), loading: false, hasChanges: false });
    } catch {
      set({ loading: false });
    }
  },
  updateDraft: (path, value) => {
    const { draft, config } = get();
    if (!draft) return;
    const updated = setNestedValue(
      draft as unknown as Record<string, unknown>,
      path,
      value,
    ) as unknown as ConfigData;
    const changed = JSON.stringify(updated) !== JSON.stringify(config);
    set({ draft: updated, hasChanges: changed });
  },
  saveConfig: async () => {
    const { draft } = get();
    if (!draft) return;
    set({ saving: true });
    try {
      await api.config.save(draft);
      set({ config: deepClone(draft), saving: false, hasChanges: false });
    } catch {
      set({ saving: false });
    }
  },
  resetDraft: () => {
    const { config } = get();
    if (!config) return;
    set({ draft: deepClone(config), hasChanges: false });
  },
}));
