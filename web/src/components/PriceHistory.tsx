"use client";

import Link from "next/link";
import { useEffect, useState } from "react";

import { PriceChart } from "@/components/chart/PriceChart";
import { PriceTable } from "@/components/chart/PriceTable";
import { LoadError } from "@/components/LoadError";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatMoney, interpolate } from "@/i18n/format";
import { api, type Schemas } from "@/lib/api/client";
import { errorMessage, retailerName } from "@/lib/ui";
import { useRequireOnboarded } from "@/lib/useRequireOnboarded";
import { useSignInRedirect } from "@/lib/useSignInRedirect";

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
              message: errorMessage(failed.response.status, failed.error?.error, common.errors),
              final: false,
            },
      );
    });
    return () => {
      active = false;
    };
  }, [asin, toSignIn, dict.notFound, common.errors, attempt]);

  const money = (v: number | null | undefined) =>
    v === null || v === undefined ? common.none : formatMoney(v, lang);

  if (gate.status === "error") {
    return <LoadError message={gate.message} retryLabel={common.retry} onRetry={gate.retry} />;
  }
  if (gate.status === "loading") {
    return <p role="status">{common.loading}</p>;
  }
  return (
    <>
      <h1>{interpolate(dict.heading, { asin })}</h1>
      {loaded && !error && loaded.history.hidden_listings > 0 ? (
        <p className="hint" data-testid="hidden-listings">
          {interpolate(dict.hidden, { count: loaded.history.hidden_listings })}
        </p>
      ) : null}
      {error?.final ? (
        <>
          <p role="alert" className="error">
            {error.message}
          </p>
          <p>
            <Link href={`/${lang}/search`}>{dict.backToSearch}</Link>
          </p>
        </>
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
        <p role="status">{common.loading}</p>
      ) : loaded.history.listings.every((l) => l.points.length === 0) ? (
        <p>{dict.empty}</p>
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
              <div className="table-wrap" key={`${listing.retailer}/${listing.retailer_sku}`}>
                <table data-testid="price-stats">
                  <caption>
                    {interpolate(dict.statsCaption, {
                      retailer: retailerName(listing.retailer, retailers),
                      sku: listing.retailer_sku,
                    })}
                  </caption>
                  <thead>
                    <tr>
                      <th scope="col">{dict.window}</th>
                      <th scope="col">{dict.low}</th>
                      <th scope="col">{dict.high}</th>
                      <th scope="col">{dict.average}</th>
                      <th scope="col">{dict.observations}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {listing.windows.map((w) => (
                      <tr key={w.days}>
                        <th scope="row">{interpolate(dict.windowDays, { days: w.days })}</th>
                        <td>{money(w.low_centavos)}</td>
                        <td>{money(w.high_centavos)}</td>
                        <td>{money(w.average_centavos)}</td>
                        <td>{w.observations}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ))}
          </section>
        </>
      )}
    </>
  );
}
