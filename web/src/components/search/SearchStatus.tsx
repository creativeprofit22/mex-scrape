import Link from "next/link";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { interpolate } from "@/i18n/format";
import type { Schemas } from "@/lib/api/client";
import { RETRY_STATES, STORE_STATES, stateFamily, type ViewState } from "@/lib/search/status";
import { retailerName } from "@/lib/ui";

type Search = Schemas["SearchResponse"];

/** Outcomes that mean a store's offers are missing. */
const BAD_OUTCOMES = new Set(["failed", "blocked_needs_kyc", "skipped_robots"]);

function statusLine(state: ViewState, dict: Dictionary["search"], search: Search | null): string {
  const s = dict.status;
  switch (state) {
    case "running-stores":
      return interpolate(s.runningStores, { count: search?.retailers.length ?? 0 });
    case "all-hidden":
      return s.allHidden;
    default:
      return s[state];
  }
}

function reasonLine(state: ViewState, dict: Dictionary["search"], search: Search | null): string | null {
  if (!search) return null;
  const reasons = dict.reasons as Record<string, string>;
  const known = search.reason ? reasons[search.reason] : undefined;
  if (state === "refused") return known ?? dict.reasons.refused;
  if (state === "partial" || state === "failed") return known ?? null;
  return null;
}

function nextHint(state: ViewState, dict: Dictionary["search"]): string | null {
  const n = dict.next;
  switch (state) {
    case "network":
      return n.network;
    case "timeout":
      return n.timeout;
    case "refused":
      return n.refused;
    case "empty":
      return n.empty;
    case "all-hidden":
      return n.allHidden;
    default:
      return null;
  }
}

/**
 * The status line (`role=status`), any request error (`role=alert`), why a
 * search ended as it did, the next action for the state, and each store's
 * outcome once known.
 */
export function SearchStatus({
  state,
  search,
  error,
  lang,
  dict,
  retryLabel,
  retailers,
  onRetry,
}: {
  state: ViewState;
  search: Search | null;
  error: string | null;
  lang: Locale;
  dict: Dictionary["search"];
  retryLabel: string;
  retailers: Dictionary["retailers"];
  onRetry: () => void;
}) {
  const reason = reasonLine(state, dict, search);
  const hint = nextHint(state, dict);
  const outcomes = dict.outcomes as Record<string, string>;
  const showStores = search !== null && STORE_STATES.has(state) && search.retailers.length > 0;
  return (
    <div className="search-status" data-state={stateFamily(state)} data-view={state}>
      <p role="status" data-testid="search-status" className="search-status__line">
        {statusLine(state, dict, search)}
      </p>
      <p role="alert" className="error">
        {error}
      </p>
      {reason ? <p className="search-status__reason">{reason}</p> : null}
      {hint || RETRY_STATES.has(state) || state === "all-hidden" ? (
        <div className="search-status__next">
          {hint ? <p className="hint">{hint}</p> : null}
          {RETRY_STATES.has(state) ? (
            <button type="button" className="secondary" onClick={onRetry}>
              {retryLabel}
            </button>
          ) : null}
          {state === "all-hidden" ? (
            <Link href={`/${lang}/onboarding`}>{dict.next.memberships}</Link>
          ) : null}
        </div>
      ) : null}
      {showStores ? (
        <ul className="store-outcomes" aria-label={dict.outcomesHeading}>
          {search.retailers.map((r) => (
            <li
              key={r.retailer}
              className={BAD_OUTCOMES.has(r.outcome) ? "store-outcome store-outcome--bad" : "store-outcome"}
            >
              {interpolate(dict.outcomeLine, {
                retailer: retailerName(r.retailer, retailers),
                detail:
                  r.outcome === "ok"
                    ? r.offers === 1
                      ? dict.offersOne
                      : interpolate(dict.offers, { count: r.offers })
                    : (outcomes[r.outcome] ?? r.outcome),
              })}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
