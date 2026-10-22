/**
 * Browser-saved search preferences (decided 2026-10-22: localStorage, no
 * account sync). Everything read back is validated; anything malformed is
 * ignored as if nothing were saved. The parsers are pure; the read/write
 * wrappers swallow storage errors (disabled, full, private mode).
 */

export const COSTS_KEY = "mex-scrape:costs";
export const RECENT_KEY = "mex-scrape:recent-searches";
/** Same-tab change notice; the `storage` event only reaches other tabs. */
export const STORAGE_EVENT = "mex-scrape:storage";

/** Most recent searches kept. */
export const MAX_RECENT = 8;
/** Longest query the api accepts (the search field's `maxLength`). */
export const MAX_QUERY_CHARS = 100;

/** Prep and shipping per unit, in centavos. */
export type Costs = { prep: number; shipping: number };

export const NO_COSTS: Costs = { prep: 0, shipping: 0 };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isCentavos(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function parseJson(raw: string | null): unknown {
  if (raw === null) return undefined;
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

/** Saved costs, or `null` when nothing valid is saved. */
export function parseCosts(raw: string | null): Costs | null {
  const value = parseJson(raw);
  if (!isRecord(value) || value.v !== 1) return null;
  if (!isCentavos(value.prep) || !isCentavos(value.shipping)) return null;
  return { prep: value.prep, shipping: value.shipping };
}

export function serializeCosts(costs: Costs): string {
  return JSON.stringify({ v: 1, prep: costs.prep, shipping: costs.shipping });
}

/** A query as kept in the list: trimmed, 1–100 characters, else `null`. */
export function cleanQuery(query: string): string | null {
  const trimmed = query.trim();
  if (trimmed === "" || trimmed.length > MAX_QUERY_CHARS) return null;
  return trimmed;
}

/** Saved recent searches, newest first; malformed storage reads as none. */
export function parseRecent(raw: string | null): string[] {
  const value = parseJson(raw);
  if (!isRecord(value) || value.v !== 1 || !Array.isArray(value.items)) return [];
  let list: string[] = [];
  // Re-adding oldest first rebuilds the same order with dedupe and the cap.
  for (const item of [...value.items].reverse()) {
    if (typeof item !== "string") return [];
    list = addRecent(list, item);
  }
  return list;
}

export function serializeRecent(items: readonly string[]): string {
  return JSON.stringify({ v: 1, items });
}

/**
 * `query` moved to the front of `list`, dropping any earlier copy that
 * differs only in case, capped at `MAX_RECENT`. An invalid query leaves
 * the list unchanged.
 */
export function addRecent(list: readonly string[], query: string): string[] {
  const clean = cleanQuery(query);
  if (clean === null) return [...list];
  const folded = clean.toLowerCase();
  return [clean, ...list.filter((q) => q.toLowerCase() !== folded)].slice(0, MAX_RECENT);
}

/** Centavos as a peso amount for an input: `1500` → `"15"`, `1550` → `"15.50"`. */
export function centavosToPesosInput(centavos: number): string {
  const pesos = Math.floor(centavos / 100);
  const rest = centavos % 100;
  return rest === 0 ? String(pesos) : `${pesos}.${String(rest).padStart(2, "0")}`;
}

/** The stored string under `key`, or `null` (also when storage is unavailable). */
export function readStorage(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** Saves `value` under `key` and tells same-tab subscribers; failures only lose the save. */
export function writeStorage(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Storage disabled or full: the value lasts only for this page.
  }
  window.dispatchEvent(new CustomEvent(STORAGE_EVENT, { detail: key }));
}

/** Calls `onChange` when either key changes in this tab or another. */
export function subscribeStorage(onChange: () => void): () => void {
  window.addEventListener("storage", onChange);
  window.addEventListener(STORAGE_EVENT, onChange);
  return () => {
    window.removeEventListener("storage", onChange);
    window.removeEventListener(STORAGE_EVENT, onChange);
  };
}
