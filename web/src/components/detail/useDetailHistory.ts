"use client";

import { useEffect, useState } from "react";

import type { Dictionary } from "@/i18n/dictionaries";
import { api, type Schemas } from "@/lib/api/client";
import { errorMessage } from "@/lib/ui";

/** Days of history the panel shows. */
export const PANEL_DAYS = 90;

export type DetailHistory =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; history: Schemas["PriceHistoryResponse"] }
  | { status: "error"; message: string };

type Settled = { asin: string; attempt: number; state: DetailHistory };

/**
 * The 90-day price history of `asin` (`null`: nothing to load). Replies for
 * an ASIN that is no longer picked are dropped; a 401 goes to sign-in.
 */
export function useDetailHistory(
  asin: string | null,
  toSignIn: () => void,
  errors: Dictionary["common"]["errors"],
): { history: DetailHistory; retry: () => void } {
  const [attempt, setAttempt] = useState(0);
  const [settled, setSettled] = useState<Settled | null>(null);

  useEffect(() => {
    if (asin === null) return;
    let active = true;
    void api
      .GET("/products/{asin}/price-history", {
        params: { path: { asin }, query: { days: PANEL_DAYS } },
      })
      .then(({ data, error, response }) => {
        if (!active) return;
        if (response.status === 401) {
          toSignIn();
          return;
        }
        setSettled({
          asin,
          attempt,
          state: data
            ? { status: "ready", history: data }
            : { status: "error", message: errorMessage(response.status, error?.error, errors) },
        });
      });
    return () => {
      active = false;
    };
  }, [asin, attempt, toSignIn, errors]);

  // Derived, so switching rows shows "loading" at once instead of the
  // previous product's chart.
  const history: DetailHistory =
    asin === null
      ? { status: "idle" }
      : settled && settled.asin === asin && settled.attempt === attempt
        ? settled.state
        : { status: "loading" };
  return { history, retry: () => setAttempt((n) => n + 1) };
}
