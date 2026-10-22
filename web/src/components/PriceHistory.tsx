"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";

import { LoadError } from "@/components/LoadError";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatDay, formatMoney, interpolate } from "@/i18n/format";
import { api, type Schemas } from "@/lib/api/client";
import { currentPath, errorMessage, retailerName, signInPath } from "@/lib/ui";
import { useRequireOnboarded } from "@/lib/useRequireOnboarded";

const DAYS = 365;
const WIDTH = 640;
const HEIGHT = 240;
const PAD = 8;
/** Distinguishable on light and dark backgrounds; series also differ by dash. */
const COLORS = ["#2563eb", "#d97706", "#059669", "#db2777", "#7c3aed", "#0891b2"];
const DASHES = ["", "6 3", "2 3", "8 3 2 3", "4 4", "1 2"];

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
  const router = useRouter();
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
        router.replace(signInPath(lang, currentPath()));
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
  }, [asin, lang, router, dict.notFound, common.errors, attempt]);

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
            <Chart
              history={loaded.history}
              lang={lang}
              dict={dict}
              retailers={retailers}
            />
            <details>
              <summary>{dict.tableToggle}</summary>
              {loaded.history.listings.map((listing) => (
                <div className="table-wrap" key={`${listing.retailer}/${listing.retailer_sku}`}>
                  <table>
                    <caption>
                      {interpolate(dict.tableCaption, {
                        retailer: retailerName(listing.retailer, retailers),
                        sku: listing.retailer_sku,
                      })}
                    </caption>
                    <thead>
                      <tr>
                        <th scope="col">{dict.day}</th>
                        <th scope="col">{dict.low}</th>
                        <th scope="col">{dict.high}</th>
                        <th scope="col">{dict.inStock}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {listing.points.map((p) => (
                        <tr key={p.day_ms}>
                          <th scope="row">{formatDay(p.day_ms, lang)}</th>
                          <td>{money(p.low_centavos)}</td>
                          <td>{money(p.high_centavos)}</td>
                          <td>{p.in_stock ? dict.yes : dict.no}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              ))}
            </details>
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

/** SVG line chart of each listing's daily low; described by `aria-label`. */
function Chart({
  history,
  lang,
  dict,
  retailers,
}: {
  history: Schemas["PriceHistoryResponse"];
  lang: Locale;
  dict: Dictionary["product"];
  retailers: Dictionary["retailers"];
}) {
  const points = history.listings.flatMap((l) => l.points);
  const lows = points.map((p) => p.low_centavos);
  const min = Math.min(...lows);
  const max = Math.max(...lows);
  const from = history.since_ms;
  const to = Math.max(history.as_of_ms, from + 1);
  const span = max - min || 1;
  const x = (ms: number) => PAD + ((ms - from) / (to - from)) * (WIDTH - 2 * PAD);
  const y = (c: number) => HEIGHT - PAD - ((c - min) / span) * (HEIGHT - 2 * PAD);
  const label = interpolate(dict.chartLabel, {
    from: formatDay(from, lang),
    to: formatDay(history.as_of_ms, lang),
    min: formatMoney(min, lang),
    max: formatMoney(max, lang),
  });

  return (
    <figure className="chart">
      <svg
        role="img"
        aria-label={label}
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        width="100%"
        preserveAspectRatio="none"
      >
        <rect
          x={0}
          y={0}
          width={WIDTH}
          height={HEIGHT}
          fill="none"
          stroke="currentColor"
          strokeOpacity={0.3}
        />
        {history.listings.map((listing, i) => (
          <polyline
            key={`${listing.retailer}/${listing.retailer_sku}`}
            fill="none"
            stroke={COLORS[i % COLORS.length]}
            strokeDasharray={DASHES[i % DASHES.length]}
            strokeWidth={2}
            vectorEffect="non-scaling-stroke"
            points={listing.points
              .map((p) => `${x(p.day_ms).toFixed(1)},${y(p.low_centavos).toFixed(1)}`)
              .join(" ")}
          />
        ))}
      </svg>
      <figcaption>
        <ul className="legend">
          {history.listings.map((listing, i) => (
            <li key={`${listing.retailer}/${listing.retailer_sku}`}>
              <svg width="24" height="8" aria-hidden="true">
                <line
                  x1={0}
                  y1={4}
                  x2={24}
                  y2={4}
                  stroke={COLORS[i % COLORS.length]}
                  strokeDasharray={DASHES[i % DASHES.length]}
                  strokeWidth={2}
                />
              </svg>
              {retailerName(listing.retailer, retailers)}
              {" · "}
              {listing.retailer_sku}
            </li>
          ))}
        </ul>
      </figcaption>
    </figure>
  );
}
