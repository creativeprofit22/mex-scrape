import { useId } from "react";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatMoney, interpolate } from "@/i18n/format";
import type { Schemas } from "@/lib/api/client";
import { retailerName } from "@/lib/ui";

/**
 * One listing's 30/90/365-day low, high and average, in tabular figures.
 * On phones the table scrolls sideways, so its wrapper is a labelled,
 * focusable region (keyboard users can scroll it too).
 */
export function PriceStatsTable({
  listing,
  lang,
  dict,
  none,
  retailers,
}: {
  listing: Schemas["PriceStatsResponse"]["listings"][number];
  lang: Locale;
  dict: Dictionary["product"];
  none: string;
  retailers: Dictionary["retailers"];
}) {
  const money = (v: number | null | undefined) =>
    v === null || v === undefined ? none : formatMoney(v, lang);
  const captionId = useId();
  return (
    <div className="table-wrap" role="region" aria-labelledby={captionId} tabIndex={0}>
      <table className="stats-table" data-testid="price-stats">
        <caption id={captionId}>
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
  );
}
