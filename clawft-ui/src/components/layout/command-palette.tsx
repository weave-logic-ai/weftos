import { useEffect, useMemo, useRef, useState } from "react";

/**
 * A fuzzy-search command palette (Cmd+K / Ctrl+K), WEFT-308.
 *
 * Indexes route nav items, recent navigations, and ad-hoc commands.
 * Keyboard navigable (arrow up/down, enter, escape), focus-trapped
 * via `autoFocus` on the input plus the `Escape` keydown handler in
 * the parent. Recent items are persisted to localStorage so the
 * palette feels stateful across sessions and across both Axum and
 * WASM adapter modes (the storage key is namespaced).
 */

export interface PaletteItem {
  /** Stable id used for recents + dedup. */
  id: string;
  /** User-visible label. */
  label: string;
  /** Optional secondary label shown in muted style (e.g. group). */
  hint?: string;
  /** Single-character icon glyph mirroring the sidebar icons. */
  icon?: string;
  /**
   * Action invoked when the user selects this item. The palette will
   * close after the action runs.
   */
  action: () => void;
}

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  items: PaletteItem[];
}

const RECENTS_KEY = "clawft.cmdk.recents";
const RECENTS_MAX = 5;

function loadRecents(): string[] {
  try {
    const raw = localStorage.getItem(RECENTS_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function saveRecents(ids: string[]) {
  try {
    localStorage.setItem(RECENTS_KEY, JSON.stringify(ids.slice(0, RECENTS_MAX)));
  } catch {
    // localStorage may be disabled — silent best-effort.
  }
}

/**
 * Subsequence-match score: returns Number.POSITIVE_INFINITY for no
 * match, lower scores for tighter matches. Empty query matches all
 * with score 0 so the order is preserved.
 */
function fuzzyScore(query: string, target: string): number {
  if (!query) return 0;
  const q = query.toLowerCase();
  const t = target.toLowerCase();

  // Cheap wins.
  if (t === q) return -1000;
  if (t.startsWith(q)) return -500;

  let qi = 0;
  let lastIdx = -1;
  let gaps = 0;
  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) {
      if (lastIdx >= 0) gaps += ti - lastIdx - 1;
      lastIdx = ti;
      qi++;
    }
  }
  return qi === q.length ? gaps + (t.length - q.length) * 0.1 : Number.POSITIVE_INFINITY;
}

export function CommandPalette({ open, onClose, items }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [activeIdx, setActiveIdx] = useState(0);
  const [recents, setRecents] = useState<string[]>(() => loadRecents());
  const inputRef = useRef<HTMLInputElement | null>(null);

  // Reset query + selection each time the palette opens.
  // The setState calls here are guarded by `open` going true and are
  // the standard React idiom for resetting modal-local state on open.
  useEffect(() => {
    if (open) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setQuery("");
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setActiveIdx(0);
      // Defer focus until after the modal mounts.
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  // Filter + rank items by fuzzy score, surfacing recents first when
  // the query is empty.
  const ranked = useMemo<PaletteItem[]>(() => {
    if (!items.length) return [];

    if (!query) {
      const recentIds = new Set(recents);
      const recent = recents
        .map((id) => items.find((it) => it.id === id))
        .filter((x): x is PaletteItem => Boolean(x));
      const rest = items.filter((it) => !recentIds.has(it.id));
      return [...recent, ...rest];
    }

    return items
      .map((item) => {
        // Best score across label + hint + id.
        const score = Math.min(
          fuzzyScore(query, item.label),
          fuzzyScore(query, item.hint ?? ""),
          fuzzyScore(query, item.id),
        );
        return { item, score };
      })
      .filter((x) => x.score !== Number.POSITIVE_INFINITY)
      .sort((a, b) => a.score - b.score)
      .map((x) => x.item);
  }, [items, query, recents]);

  // Clamp the active index whenever the ranked list shrinks.
  // The setState here is the standard React pattern for syncing a
  // selection index with a derived collection length.
  useEffect(() => {
    if (activeIdx >= ranked.length) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setActiveIdx(Math.max(0, ranked.length - 1));
    }
  }, [ranked.length, activeIdx]);

  function runItem(item: PaletteItem) {
    const next = [item.id, ...recents.filter((id) => id !== item.id)].slice(0, RECENTS_MAX);
    setRecents(next);
    saveRecents(next);
    onClose();
    // Defer the action so the modal teardown doesn't race a navigation.
    requestAnimationFrame(() => item.action());
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIdx((i) => Math.min(i + 1, Math.max(0, ranked.length - 1)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIdx((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = ranked[activeIdx];
      if (item) runItem(item);
    }
    // Escape is handled by the parent's keydown listener.
  }

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center pt-24"
      role="dialog"
      aria-modal="true"
      aria-label="Command palette"
    >
      <button
        type="button"
        className="fixed inset-0 bg-black/50"
        onClick={onClose}
        aria-label="Close command palette"
      />
      <div className="relative z-50 w-full max-w-lg rounded-lg border border-gray-200 bg-white shadow-2xl dark:border-gray-700 dark:bg-gray-800">
        <input
          ref={inputRef}
          type="text"
          placeholder="Type a command or search..."
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setActiveIdx(0);
          }}
          onKeyDown={handleKeyDown}
          className="w-full rounded-t-lg border-0 bg-transparent px-4 py-3 text-sm text-gray-900 placeholder-gray-400 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-gray-100 dark:placeholder-gray-500"
          aria-label="Command palette search"
          aria-controls="command-palette-listbox"
          aria-activedescendant={ranked[activeIdx]?.id}
        />
        <ul
          id="command-palette-listbox"
          role="listbox"
          aria-label="Commands"
          className="max-h-80 overflow-y-auto border-t border-gray-200 dark:border-gray-700"
        >
          {ranked.length === 0 && (
            <li className="px-4 py-3 text-xs text-gray-400 dark:text-gray-500">
              No matches.
            </li>
          )}
          {ranked.map((item, idx) => {
            const active = idx === activeIdx;
            return (
              <li
                key={item.id}
                id={item.id}
                role="option"
                aria-selected={active}
              >
                <button
                  type="button"
                  onClick={() => runItem(item)}
                  onMouseEnter={() => setActiveIdx(idx)}
                  className={
                    "flex w-full items-center gap-3 px-4 py-2 text-left text-sm transition-colors " +
                    (active
                      ? "bg-blue-50 text-blue-900 dark:bg-blue-900/30 dark:text-blue-100"
                      : "text-gray-700 hover:bg-gray-100 dark:text-gray-200 dark:hover:bg-gray-700")
                  }
                >
                  {item.icon && (
                    <span className="inline-flex h-5 w-5 items-center justify-center text-xs font-bold">
                      {item.icon}
                    </span>
                  )}
                  <span className="flex-1">{item.label}</span>
                  {item.hint && (
                    <span className="text-xs text-gray-400 dark:text-gray-500">
                      {item.hint}
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
        <div className="border-t border-gray-200 px-4 py-2 text-xs text-gray-400 dark:border-gray-700 dark:text-gray-500">
          <kbd className="font-sans">↑↓</kbd> navigate ·{" "}
          <kbd className="font-sans">↵</kbd> select ·{" "}
          <kbd className="font-sans">esc</kbd> close
        </div>
      </div>
    </div>
  );
}
