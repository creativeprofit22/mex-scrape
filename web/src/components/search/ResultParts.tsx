import Link from "next/link";

import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { formatBasisPoints, formatMoney, interpolate } from "@/i18n/format";
import { rowProfit, rowRoi, type Row } from "@/lib/search/rows";
import { retailerName } from "@/lib/ui";

import { Signed } from "./Signed";

/** Everything a result's parts need to render; shared by the table and the cards. */
export type ResultView = {
  lang: Locale;
  dict: Dictionary["search"];
  common: Dictionary["common"];
  retailers: Dictionary["retailers"];
};

/** The title button, which selects the result (style-pack §5). */
export function ResultTitle({
  row,
  selected,
  onSelect,
}: {
  row: Row;
  selected: boolean;
  onSelect: (key: string | null) => void;
}) {
  return (
    <button
      type="button"
      className="plain-button row-select"
      aria-pressed={selected}
      onClick={() => onSelect(selected ? null : row.key)}
    >
      {row.result.title}
    </button>
  );
}

/** The store, its link and the membership tag, on one line under the title. */
export function ResultMeta({ row, view }: { row: Row; view: ResultView }) {
  const { result } = row;
  const { dict } = view;
  return (
    <span className="result-meta">
      <span className="result-meta__store">{retailerName(result.retailer, view.retailers)}</span>
      <a
        className="result-link"
        href={result.url}
        target="_blank"
        rel="noopener noreferrer"
        aria-label={`${dict.storeLink} · ${result.title}`}
      >
        {dict.storeLinkShort}
      </a>
      {result.membership_required ? <span className="tag">{dict.membership}</span> : null}
    </span>
  );
}

export function Availability({ row, view }: { row: Row; view: ResultView }) {
  const { availability } = row.result;
  const labels = view.dict.availability as Record<string, string>;
  const text = labels[availability] ?? view.dict.availability.unknown;
  if (availability === "out_of_stock") return <span className="tag tag-loss">{text}</span>;
  return <>{text}</>;
}

/** The Amazon match: its status tag, then price and history link on one line. */
export function AmazonMatch({ row, view }: { row: Row; view: ResultView }) {
  const { amazon } = row.result;
  const { dict, lang, common } = view;
  if (!amazon) return <span className="muted">{dict.match.none}</span>;
  return (
    <span className="amazon-match">
      {amazon.match_status === "accepted" ? (
        <span className="tag tag-gain">{dict.match.accepted}</span>
      ) : (
        <span className="tag tag-warn">{dict.match.needsReview}</span>
      )}
      <span className="amazon-match__line">
        <span className="amazon-match__price">
          {amazon.amazon_price_centavos === null || amazon.amazon_price_centavos === undefined
            ? common.none
            : formatMoney(amazon.amazon_price_centavos, lang)}
        </span>
        <Link
          className="result-link"
          href={`/${lang}/products/${amazon.asin}`}
          aria-label={interpolate(dict.match.history, { asin: amazon.asin })}
        >
          {dict.match.historyShort}
        </Link>
      </span>
    </span>
  );
}

export function Price({ row, view }: { row: Row; view: ResultView }) {
  return <>{formatMoney(row.result.price_centavos, view.lang)}</>;
}

/** Profit per unit with the user's costs, its ROI error, or none. */
export function Profit({ row, view }: { row: Row; view: ResultView }) {
  const { roi } = row;
  const { dict, common, lang } = view;
  if (roi?.error) {
    return <>{(dict.roiError as Record<string, string>)[roi.error] ?? common.errors.generic}</>;
  }
  const profit = rowProfit(row);
  if (profit === null) return <>{common.none}</>;
  return <Signed value={profit} format={(v) => formatMoney(v, lang)} sign={dict.sign} />;
}

export function Roi({ row, view }: { row: Row; view: ResultView }) {
  const { dict, common, lang } = view;
  const roi = rowRoi(row);
  if (roi === null) return <>{common.none}</>;
  return <Signed value={roi} format={(v) => formatBasisPoints(v, lang)} sign={dict.sign} />;
}
