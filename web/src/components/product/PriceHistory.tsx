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
import { isProductNotFound } from "@/lib/product/name";
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
 * Price history chart (with a table fallback) and 30/90/365-day stats,
 * under a header titled with the Amazon name. An ASIN the api doesn't know
 * (404 `product_not_found`, or 400 `invalid_asin`) gets its own not-found
 * state with a way back to search; a known product with no prices yet gets
 * the empty state. Users who have not finished onboarding are sent there first.
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
  // Unknown product: its own state with a way back instead of a retry.
  const [notFound, setNotFound] = useState(false);
  const [error, setError] = useState<string | null>(null);
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
      if (isProductNotFound(failed.response.status, failed.error?.error)) {
        setNotFound(true);
        return;
      }
      setError(
        interpolate(dict.loadFailed, {
          reason: errorMessage(failed.response.status, failed.error?.error, common.errors),
        }),
      );
    });
    return () => {
      active = false;
    };
  }, [asin, toSignIn, dict.loadFailed, common.errors, attempt]);

  if (gate.status === "error") {
    return <LoadError message={gate.message} retryLabel={common.retry} onRetry={gate.retry} />;
  }
  if (gate.status === "loading") {
    return <p role="status">{common.loading}</p>;
  }
  if (notFound) {
    return (
      <div className="product product--not-found" data-testid="product-not-found">
        <h1>{dict.notFoundHeading}</h1>
        <p role="alert" className="product__not-found-text">
          {interpolate(dict.notFound, { asin })}
        </p>
        <p>
          <Link href={`/${lang}/search`} className="button">
            {dict.searchCta}
          </Link>
        </p>
      </div>
    );
  }
  const empty = loaded !== null && loaded.history.listings.every((l) => l.points.length === 0);
  return (
    <div className="product">
      <ProductHeader
        asin={asin}
        title={loaded?.history.title}
        brand={loaded?.history.brand}
        lang={lang}
        dict={dict}
      />
      {loaded && !error && loaded.history.hidden_listings > 0 ? (
        <p className="hint" data-testid="hidden-listings">
          {interpolate(dict.hidden, { count: loaded.history.hidden_listings })}
        </p>
      ) : null}
      {error !== null ? (
        <LoadError
          message={error}
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
