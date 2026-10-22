"use client";

import { useLayoutEffect, useRef, useState } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatDay, formatMoney, formatShortDay, formatWholePesos, interpolate } from "@/i18n/format";
import type { Schemas } from "@/lib/api/client";
import { goodPriceBand } from "@/lib/chart/band";
import { DAY_MS, dateTicks } from "@/lib/chart/dates";
import { linearScale, niceTicks } from "@/lib/chart/scale";
import { retailerName } from "@/lib/ui";

/** Series differ by dash as well as colour (`.series-N` in chart.css). */
const DASHES = ["", "6 3", "2 3", "8 3 2 3", "4 4", "1 2"];
const SERIES = 6;
/** Fallback width before the first measurement (and in tests without layout). */
const DEFAULT_WIDTH = 640;
const TOP = 8;
const RIGHT = 12;
/** Room under the plot for the date labels. */
const BOTTOM = 24;
/** Approximate width of one tick-label character at --text-label (tabular). */
const CHAR_PX = 7.5;
/** Minimum horizontal space per date label. */
const DATE_LABEL_PX = 72;

type History = Schemas["PriceHistoryResponse"];

/**
 * SVG line chart of each listing's daily low, with round price ticks
 * (1-2-5 rule), date ticks and the "good price" band. Drawn at the
 * container's real width so labels stay at their CSS size. The `aria-label`
 * summarises range, min and max; pair it with `PriceTable` for the values.
 */
export function PriceChart({
  history,
  lang,
  dict,
  retailers,
  height = 240,
}: {
  history: History;
  lang: Locale;
  dict: Pick<Dictionary["product"], "chartLabel" | "bandLabel">;
  retailers: Dictionary["retailers"];
  /** Total SVG height in px, axes included. */
  height?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(DEFAULT_WIDTH);

  useLayoutEffect(() => {
    const node = ref.current;
    if (!node) return;
    const measure = () => {
      const next = Math.round(node.getBoundingClientRect().width);
      if (next > 0) setWidth(next);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  const points = history.listings.flatMap((l) => l.points);
  const lows = points.map((p) => p.low_centavos);
  const min = Math.min(...lows);
  const max = Math.max(...lows);
  const yTicks = niceTicks(min, max, Math.max(3, Math.floor((height - TOP - BOTTOM) / 40)), {
    minStep: 100,
  });
  const yLabels = yTicks.map((t) => formatWholePesos(t, lang));
  const left = Math.ceil(Math.max(0, ...yLabels.map((l) => l.length)) * CHAR_PX) + 12;
  const plotRight = Math.max(left + 1, width - RIGHT);
  const plotBottom = height - BOTTOM;

  const from = history.since_ms;
  const to = Math.max(history.as_of_ms, from + DAY_MS);
  const x = linearScale([from, to], [left, plotRight]);
  const yDomain: [number, number] = [yTicks[0] ?? min, yTicks[yTicks.length - 1] ?? max];
  const y = linearScale(yDomain, [plotBottom, TOP]);
  const xTicks = dateTicks(from, to, Math.max(2, Math.floor((plotRight - left) / DATE_LABEL_PX)));
  const band = goodPriceBand(points);

  const label = interpolate(dict.chartLabel, {
    from: formatDay(from, lang),
    to: formatDay(history.as_of_ms, lang),
    min: formatMoney(min, lang),
    max: formatMoney(max, lang),
  });
  const bandText = band
    ? interpolate(dict.bandLabel, { low: formatMoney(band.low, lang), high: formatMoney(band.high, lang) })
    : null;

  return (
    <figure className="chart">
      <div ref={ref} className="chart__frame">
        <svg role="img" aria-label={label} width={width} height={height} viewBox={`0 0 ${width} ${height}`}>
          {band ? (
            <rect
              className="chart__band"
              x={left}
              y={y(band.high)}
              width={plotRight - left}
              height={Math.max(1, y(band.low) - y(band.high))}
            />
          ) : null}
          <g aria-hidden="true">
            {yTicks.map((t, i) => (
              <g key={t}>
                <line className="chart__grid" x1={left} x2={plotRight} y1={y(t)} y2={y(t)} />
                <text className="chart__tick" x={left - 8} y={y(t)} textAnchor="end" dominantBaseline="middle">
                  {yLabels[i]}
                </text>
              </g>
            ))}
            {xTicks.map((t) => {
              const tx = x(t);
              // Keep the first and last labels inside the frame.
              const anchor = tx - left < 28 ? "start" : plotRight - tx < 28 ? "end" : "middle";
              return (
                <g key={t}>
                  <line className="chart__axis" x1={tx} x2={tx} y1={plotBottom} y2={plotBottom + 4} />
                  <text className="chart__tick" x={tx} y={height - 6} textAnchor={anchor}>
                    {formatShortDay(t, lang)}
                  </text>
                </g>
              );
            })}
            <line className="chart__axis" x1={left} x2={plotRight} y1={plotBottom} y2={plotBottom} />
          </g>
          {history.listings.map((listing, i) => {
            const series = `chart__series series-${i % SERIES}`;
            const dash = DASHES[i % DASHES.length];
            const key = `${listing.retailer}/${listing.retailer_sku}`;
            const only = listing.points.length === 1 ? listing.points[0] : undefined;
            return only ? (
              <circle key={key} className={series} cx={x(only.day_ms)} cy={y(only.low_centavos)} r={3} />
            ) : (
              <polyline
                key={key}
                className={series}
                fill="none"
                strokeDasharray={dash || undefined}
                points={listing.points
                  .map((p) => `${x(p.day_ms).toFixed(1)},${y(p.low_centavos).toFixed(1)}`)
                  .join(" ")}
              />
            );
          })}
        </svg>
      </div>
      <figcaption>
        <ul className="legend">
          {history.listings.map((listing, i) => (
            <li key={`${listing.retailer}/${listing.retailer_sku}`}>
              <svg width="24" height="8" aria-hidden="true">
                <line
                  className={`chart__series series-${i % SERIES}`}
                  x1={0}
                  y1={4}
                  x2={24}
                  y2={4}
                  strokeDasharray={DASHES[i % DASHES.length] || undefined}
                />
              </svg>
              {retailerName(listing.retailer, retailers)}
              {" · "}
              {listing.retailer_sku}
            </li>
          ))}
          {bandText ? (
            <li data-testid="chart-band">
              <svg width="24" height="12" aria-hidden="true">
                <rect className="chart__band" x={0} y={0} width={24} height={12} />
              </svg>
              {bandText}
            </li>
          ) : null}
        </ul>
      </figcaption>
    </figure>
  );
}
