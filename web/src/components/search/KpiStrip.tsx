import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatMoney, interpolate } from "@/i18n/format";
import type { Kpis } from "@/lib/search/kpis";

import { KpiTile, KpiTileSkeleton } from "./KpiTile";
import { Signed } from "./Signed";

const TILES = 4;

/**
 * Whole-search summary: best profit (display numeral, gain/loss), offers,
 * stores with offers and offers hidden by membership. `null` shows static
 * skeleton tiles while the search runs.
 */
export function KpiStrip({
  kpis,
  lang,
  dict,
  none,
}: {
  kpis: Kpis | null;
  lang: Locale;
  dict: Dictionary["search"];
  none: string;
}) {
  if (kpis === null) {
    return (
      <div className="kpi-strip" aria-hidden="true">
        {Array.from({ length: TILES }, (_, i) => (
          <KpiTileSkeleton key={i} />
        ))}
      </div>
    );
  }
  const { kpi } = dict;
  return (
    <dl className="kpi-strip" aria-label={kpi.label}>
      <KpiTile label={kpi.bestProfit} testId="kpi-best-profit">
        {kpis.bestProfit === null ? (
          none
        ) : (
          <Signed
            value={kpis.bestProfit.centavos}
            format={(v) => formatMoney(v, lang)}
            sign={dict.sign}
          />
        )}
      </KpiTile>
      <KpiTile label={kpi.offers} testId="kpi-offers">
        {kpis.offers}
      </KpiTile>
      <KpiTile label={kpi.stores} testId="kpi-stores">
        {interpolate(kpi.storesValue, kpis.stores)}
      </KpiTile>
      <KpiTile label={kpi.hidden} testId="kpi-hidden">
        {kpis.hidden}
      </KpiTile>
    </dl>
  );
}
