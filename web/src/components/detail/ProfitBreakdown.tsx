import { Signed } from "@/components/search/Signed";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatMoney } from "@/i18n/format";
import { rowProfit, type Row } from "@/lib/search/rows";
import type { Costs } from "@/lib/search/storage";

/**
 * Where the profit comes from, as amounts (the API returns fee amounts):
 * Amazon sale minus purchase, Amazon fees, and the saved prep and shipping
 * costs. Profit is the row's `POST /roi` result, so it matches the table.
 */
export function ProfitBreakdown({
  row,
  costs,
  lang,
  dict,
  search,
  none,
  generic,
}: {
  row: Row;
  costs: Costs;
  lang: Locale;
  dict: Dictionary["detail"];
  search: Dictionary["search"];
  none: string;
  generic: string;
}) {
  const amazon = row.result.amazon;
  const money = (v: number | null | undefined) =>
    v === null || v === undefined ? none : formatMoney(v, lang);
  // A cost is subtracted, so it carries a minus; a zero cost has none ("$0.00").
  const cost = (v: number | null | undefined) =>
    v === null || v === undefined
      ? none
      : `${v === 0 ? "" : search.sign.minus}${formatMoney(v, lang)}`;
  const profit = rowProfit(row);
  const roiError = row.roi?.error;

  return (
    <section className="detail__section" aria-labelledby="detail-profit-heading">
      <h3 id="detail-profit-heading" className="detail__heading">
        {dict.profitHeading}
      </h3>
      <dl className="breakdown">
        <div>
          <dt>{dict.sale}</dt>
          <dd className="num">{money(amazon?.amazon_price_centavos)}</dd>
        </div>
        <div>
          <dt>{dict.purchase}</dt>
          <dd className="num">{cost(row.result.price_centavos)}</dd>
        </div>
        <div>
          <dt>{dict.fees}</dt>
          <dd className="num">{cost(amazon?.fees_centavos)}</dd>
        </div>
        <div>
          <dt>{dict.prep}</dt>
          <dd className="num">{cost(costs.prep)}</dd>
        </div>
        <div>
          <dt>{dict.shipping}</dt>
          <dd className="num">{cost(costs.shipping)}</dd>
        </div>
        <div className="breakdown__total">
          <dt>{dict.profit}</dt>
          <dd className="num" data-testid="detail-profit">
            {roiError ? (
              ((search.roiError as Record<string, string>)[roiError] ?? generic)
            ) : profit === null ? (
              none
            ) : (
              <Signed value={profit} format={(v) => formatMoney(v, lang)} sign={search.sign} />
            )}
          </dd>
        </div>
      </dl>
      <p className="hint">{dict.costsHint}</p>
    </section>
  );
}
