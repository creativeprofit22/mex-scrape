import type { Schemas } from "@/lib/api/client";

/** What the search page is showing; every state has copy and a next action. */
export type ViewState =
  | "idle"
  | "queued"
  | "running"
  | "running-stores"
  | "done"
  | "partial"
  | "failed"
  | "refused"
  | "timeout"
  | "network"
  | "error"
  | "empty"
  | "all-hidden";

/** Statuses after which polling stops and results are final. */
export const FINISHED_STATUSES: ReadonlySet<string> = new Set([
  "done",
  "partial",
  "failed",
  "refused",
]);

/** States whose next action is to run the same search again. */
export const RETRY_STATES: ReadonlySet<ViewState> = new Set([
  "network",
  "error",
  "failed",
  "timeout",
]);

/** States that show the per-store outcome chips. */
export const STORE_STATES: ReadonlySet<ViewState> = new Set([
  "running-stores",
  "done",
  "partial",
  "empty",
  "all-hidden",
]);

/** Coarse state family, exposed as `data-state` (09-states: loading / empty / error). */
export type StateFamily = "idle" | "loading" | "done" | "empty" | "error";

const FAMILY: Readonly<Record<ViewState, StateFamily>> = {
  idle: "idle",
  queued: "loading",
  running: "loading",
  "running-stores": "loading",
  done: "done",
  partial: "done",
  empty: "empty",
  "all-hidden": "empty",
  failed: "error",
  refused: "error",
  timeout: "error",
  network: "error",
  error: "error",
};

export function stateFamily(state: ViewState): StateFamily {
  return FAMILY[state];
}

export type ViewInput = {
  search: Pick<Schemas["SearchResponse"], "status" | "retailers" | "results" | "hidden_results"> | null;
  pending: boolean;
  timedOut: boolean;
  /** The last request's failure: `network` when it never reached the api. */
  failure: "network" | "other" | null;
};

/**
 * The page state. A request failure wins (polling stopped), then the
 * timeout. While running, outcomes appear only once every store was
 * fetched (the worker writes them together), so `running-stores` means
 * "stores done, comparing with Amazon".
 */
export function viewState({ search, pending, timedOut, failure }: ViewInput): ViewState {
  if (failure === "network") return "network";
  if (failure === "other") return "error";
  if (timedOut) return "timeout";
  if (!search) return pending ? "queued" : "idle";
  switch (search.status) {
    case "queued":
      return "queued";
    case "failed":
      return "failed";
    case "refused":
      return "refused";
    case "partial":
      return "partial";
    case "done":
      if (search.results.length > 0) return "done";
      return search.hidden_results > 0 ? "all-hidden" : "empty";
    default:
      return search.retailers.length > 0 ? "running-stores" : "running";
  }
}
