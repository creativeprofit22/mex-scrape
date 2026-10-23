import type { Locale } from "@/i18n/config";
import { formatBasisPoints, interpolate } from "@/i18n/format";

/** Matcher confidence (basis points, 0–10000) as a labelled `<meter>`. */
export function MatchMeter({
  confidenceBp,
  lang,
  label,
  valueLabel,
}: {
  confidenceBp: number;
  lang: Locale;
  /** Visible label, e.g. "Confianza de la coincidencia". */
  label: string;
  /** Accessible name with the value, e.g. "Confianza de la coincidencia: {value}". */
  valueLabel: string;
}) {
  const value = Math.min(10_000, Math.max(0, confidenceBp));
  const shown = formatBasisPoints(value, lang);
  return (
    <div className="match-meter">
      <span className="match-meter__label" aria-hidden="true">
        {label}
      </span>
      <meter
        className="match-meter__bar"
        min={0}
        max={10_000}
        value={value}
        aria-label={interpolate(valueLabel, { value: shown })}
      />
      <span className="match-meter__value num" aria-hidden="true">
        {shown}
      </span>
    </div>
  );
}
