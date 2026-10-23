"use client";

import Link from "next/link";
import { useEffect, useState } from "react";

import { PriceChart } from "@/components/chart/PriceChart";
import { PriceTable } from "@/components/chart/PriceTable";
import { LoadError } from "@/components/feedback/LoadError";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { interpolate } from "@/i18n/format";
import { api, type Schemas } from "@/lib/api/client";
import { errorMessage } from "@/lib/ui";
import { useRequireOnboarded } from "@/lib/useRequireOnboarded";
import { useSignInRedirect } from "@/lib/useSignInRedirect";

import { PriceStatsTable } from "./PriceStatsTable";
import { ProductHeader } from "./ProductHeader";

const DAYS = 365;

type Loaded = {
  history: Schemas["PriceHistoryResponse"];
  stats: Schemas["PriceStatsResponse"];
};

/**
 * Price history chart (with a table fallback) and 30/90/365-day stats.
 * Users who have not finished onboarding are sent there first.
 */
export function PriceHistory({
  asin,
  lang,
  dict,
  common,
  retailers,
}: {
  asin: string;
  lang: Locale;
  dict: Dictionary["product"];
  common: Dictionary["common"];
  retailers: Dictionary["retailers"];
}) {
  const toSignIn = useSignInRedirect(lang);
  const gate = useRequireOnboarded(lang, common.errors);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  // `final` errors (unknown product) get a way back instead of a retry.
  const [error, setError] = useState<{ message: string; final: boolean } | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    void Promise.all([
      api.GET("/products/{asin}/price-history", {
        params: { path: { asin }, query: { days: DAYS } },
      }),
      api.GET("/products/{asin}/price-stats", { params: { path: { asin } } }),
    ]).then(([history, stats]) => {
      if (!active) return;
      if (history.response.status === 401 || stats.response.status === 401) {
        toSignIn();
        return;
      }
      if (history.data && stats.data) {
        setLoaded({ history: history.data, stats: stats.data });
        return;
      }
      const failed = history.data ? stats : history;
      const notFound = failed.response.status === 404 || failed.error?.error === "invalid_asin";
      setError(
        notFound
          ? { message: dict.notFound, final: true }
          : {
              message: interpolate(dict.loadFailed, {
                reason: errorMessage(failed.response.status, failed.error?.error, common.errors),
              }),
              final: false,
            },
      );
    });
    return () => {
      active = false;
    };
  }, [asin, toSignIn, dict.notFound, dict.loadFailed, common.errors, attempt]);

  if (gate.status === "error") {
    return <LoadError message={gate.message} retryLabel={common.retry} onRetry={gate.retry} />;
  }
  if (gate.status === "loading") {
    return <p role="status">{common.loading}</p>;
  }
  const empty = loaded !== null && loaded.history.listings.every((l) => l.points.length === 0);
  return (
    <div className="product">
      <ProductHeader asin={asin} lang={lang} dict={dict} />
      {loaded && !error && loaded.history.hidden_listings > 0 ? (
        <p className="hint" data-testid="hidden-listings">
          {interpolate(dict.hidden, { count: loaded.history.hidden_listings })}
        </p>
      ) : null}
      {error?.final ? (
        <div className="stack load-error" data-testid="product-not-found">
          <p role="alert" className="error">
            {error.message}
          </p>
          <p>
            <Link href={`/${lang}/search`}>{dict.backToSearch}</Link>
          </p>
        </div>
      ) : error ? (
        <LoadError
          message={error.message}
          retryLabel={common.retry}
          onRetry={() => {
            setError(null);
            setAttempt((n) => n + 1);
          }}
        />
      ) : !loaded ? (
        <ProductSkeleton label={dict.loading} />
      ) : empty ? (
        <p className="product__empty" data-testid="product-empty">
          {dict.empty}
        </p>
      ) : (
        <>
          <section aria-labelledby="chart-heading">
            <h2 id="chart-heading">{dict.chartHeading}</h2>
            <PriceChart history={loaded.history} lang={lang} dict={dict} retailers={retailers} />
            <PriceTable
              history={loaded.history}
              lang={lang}
              dict={dict}
              none={common.none}
              retailers={retailers}
            />
          </section>
          <section aria-labelledby="stats-heading">
            <h2 id="stats-heading">{dict.statsHeading}</h2>
            {loaded.stats.listings.map((listing) => (
              <PriceStatsTable
                key={`${listing.retailer}/${listing.retailer_sku}`}
                listing={listing}
                lang={lang}
                dict={dict}
                none={common.none}
                retailers={retailers}
              />
            ))}
          </section>
        </>
      )}
    </div>
  );
}

/** Static stand-in for the chart while both requests are in flight. */
function ProductSkeleton({ label }: { label: string }) {
  return (
    <section className="product__loading" aria-busy="true" data-testid="product-loading">
      <p role="status">{label}</p>
      <div className="product__skeleton" aria-hidden="true">
        <span className="skeleton skeleton--label" />
        <span className="skeleton product__skeleton-chart" />
      </div>
    </section>
  );
}
