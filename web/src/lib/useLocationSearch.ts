import { useSyncExternalStore } from "react";

const subscribe = () => () => {};

/**
 * `window.location.search`, read at render without a hydration mismatch:
 * the server and the hydrating render see `""`, later renders the real query.
 * Client navigations re-render through `usePathname`, which re-reads it.
 */
export function useLocationSearch(): string {
  return useSyncExternalStore(
    subscribe,
    () => window.location.search,
    () => "",
  );
}
