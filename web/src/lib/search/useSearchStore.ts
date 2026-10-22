"use client";

import { useCallback, useMemo, useSyncExternalStore } from "react";

import {
  COSTS_KEY,
  RECENT_KEY,
  addRecent,
  parseCosts,
  parseRecent,
  readStorage,
  serializeCosts,
  serializeRecent,
  subscribeStorage,
  writeStorage,
  type Costs,
} from "./storage";

/**
 * The raw stored string, so React compares snapshots by value. The server
 * and hydrating render see `null`, then the saved value shows (same
 * pattern as the calculator draft), so hydration always matches.
 */
function useStored(key: string): string | null {
  return useSyncExternalStore(
    subscribeStorage,
    () => readStorage(key),
    () => null,
  );
}

/** Saved prep and shipping (`null` before hydration or when none are saved). */
export function useCosts(): [Costs | null, (costs: Costs) => void] {
  const raw = useStored(COSTS_KEY);
  const costs = useMemo(() => parseCosts(raw), [raw]);
  const save = useCallback((next: Costs) => {
    writeStorage(COSTS_KEY, serializeCosts(next));
  }, []);
  return [costs, save];
}

/** Saved recent searches, newest first, plus a function recording a new one. */
export function useRecentSearches(): [string[], (query: string) => void] {
  const raw = useStored(RECENT_KEY);
  const recent = useMemo(() => parseRecent(raw), [raw]);
  const remember = useCallback((query: string) => {
    const current = parseRecent(readStorage(RECENT_KEY));
    writeStorage(RECENT_KEY, serializeRecent(addRecent(current, query)));
  }, []);
  return [recent, remember];
}
