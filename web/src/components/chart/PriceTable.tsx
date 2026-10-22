import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatDay, formatMoney, interpolate } from "@/i18n/format";
import type { Schemas } from "@/lib/api/client";
import { retailerName } from "@/lib/ui";

/** The chart's values as tables, one per listing, behind a disclosure. */
export function PriceTable({
  history,
  lang,
  dict,
  none,
  retailers,
}: {
  history: Schemas["PriceHistoryResponse"];
  lang: Locale;
  dict: Pick<
    Dictionary["product"],
    "tableToggle" | "tableCaption" | "day" | "low" | "high" | "inStock" | "yes" | "no"
  >;
  /** Shown for a missing amount. */
  none: string;
  retailers: Dictionary["retailers"];
}) {
  const money = (v: number | null | undefined) =>
    v === null || v === undefined ? none : formatMoney(v, lang);
  return (
    <details>
      <summary>{dict.tableToggle}</summary>
      {history.listings.map((listing) => (
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
  );
}
