"use client";

import Link from "next/link";

import { rerunHref } from "@/components/search/RecentSearches";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { interpolate } from "@/i18n/format";
import { productTitle } from "@/lib/product/name";
import { useRecentSearches } from "@/lib/search/useSearchStore";

/**
 * Product page header: the way back (search, plus a rerun of the newest
 * recent search when this browser saved one), then the Amazon name as the
 * title with the ASIN (and brand, when known) as metadata. Until the name
 * loads, or when none is stored, the title is the "price history of
 * {asin}" heading (ADR 0017).
 */
export function ProductHeader({
  asin,
  title,
  brand,
  lang,
  dict,
}: {
  asin: string;
  title: string | null | undefined;
  brand: string | null | undefined;
  lang: Locale;
  dict: Dictionary["product"];
}) {
  const [recent] = useRecentSearches();
  const last = recent[0];
  const brandName = brand?.trim() ?? "";
  return (
    <header className="product-header">
      <p className="product-header__back">
        <Link href={`/${lang}/search`} className="back-link" data-testid="product-back">
          {dict.backToSearch}
        </Link>
        {last !== undefined ? (
          <Link href={rerunHref(lang, last)} data-testid="product-rerun">
            {interpolate(dict.rerunRecent, { query: last })}
          </Link>
        ) : null}
      </p>
      <h1 className="product-header__title">{productTitle(title, interpolate(dict.heading, { asin }))}</h1>
      <dl className="product-header__meta">
        <div>
          <dt className="t-label">{dict.asinLabel}</dt>
          <dd className="t-num">{asin}</dd>
        </div>
        {brandName !== "" ? (
          <div data-testid="product-brand">
            <dt className="t-label">{dict.brandLabel}</dt>
            <dd>{brandName}</dd>
          </div>
        ) : null}
      </dl>
    </header>
  );
}
