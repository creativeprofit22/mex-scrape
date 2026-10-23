import type { Ref } from "react";

import { Signed } from "@/components/search/Signed";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatBasisPoints, formatMoney } from "@/i18n/format";
import type { Schemas } from "@/lib/api/client";
import type { SignGlyphs } from "@/lib/search/format";

/**
 * The result panel: profit per unit as a display numeral and ROI, both
 * signed (gain/loss colour plus sign and ↓), then the breakdown. Before
 * the first calculation it says what to do.
 */
export function CalculatorResult({
  ref,
  result,
  pending,
  lang,
  dict,
  none,
  sign,
}: {
  ref?: Ref<HTMLElement>;
  result: Schemas["RoiItemResponse"] | null;
  pending: boolean;
  lang: Locale;
  dict: Dictionary["calculator"];
  none: string;
  sign: SignGlyphs & { down: string };
}) {
  const money = (v: number | null | undefined) =>
    v === null || v === undefined ? none : formatMoney(v, lang);
  const percent = (v: number | null | undefined) =>
    v === null || v === undefined ? none : formatBasisPoints(v, lang);
  const signed = (v: number | null | undefined, format: (absolute: number) => string) =>
    v === null || v === undefined ? none : <Signed value={v} format={format} sign={sign} />;

  return (
    <section ref={ref} className="calculator__result" aria-labelledby="calc-result" aria-busy={pending || undefined}>
      <h2 id="calc-result" className="calculator__heading">
        {dict.resultHeading}
      </h2>
      <div aria-live="polite">
        {result ? (
          <>
            <dl className="calculator__figures">
              <div className="calculator__profit">
                <dt className="calculator__label">{dict.profit}</dt>
                <dd className="calculator__numeral" data-testid="calc-profit">
                  {signed(result.profit_centavos, (v) => formatMoney(v, lang))}
                </dd>
              </div>
              <div className="calculator__roi">
                <dt className="calculator__label">{dict.roi}</dt>
                <dd className="calculator__roi-value" data-testid="calc-roi">
                  {signed(result.roi_bp, (v) => formatBasisPoints(v, lang))}
                </dd>
              </div>
            </dl>
            <dl className="breakdown">
              <div>
                <dt>{dict.margin}</dt>
                <dd className="num">{percent(result.margin_bp)}</dd>
              </div>
              <div>
                <dt>{dict.totalCost}</dt>
                <dd className="num">{money(result.total_cost_centavos)}</dd>
              </div>
              <div>
                <dt>{dict.fixedPerUnit}</dt>
                <dd className="num">{money(result.fixed_cost_per_unit_centavos)}</dd>
              </div>
            </dl>
          </>
        ) : (
          <p className="calculator__empty">{pending ? dict.calculating : dict.resultEmpty}</p>
        )}
      </div>
    </section>
  );
}
