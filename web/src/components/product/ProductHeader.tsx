"use client";

import Link from "next/link";

import { rerunHref } from "@/components/search/RecentSearches";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";
import { interpolate } from "@/i18n/format";
import { useRecentSearches } from "@/lib/search/useSearchStore";

/**
 * Product page header: the way back (search, plus a rerun of the newest
 * recent search when this browser saved one), then the title and ASIN.
 */
export function ProductHeader({
  asin,
  lang,
  dict,
}: {
  asin: string;
  lang: Locale;
  dict: Dictionary["product"];
}) {
  const [recent] = useRecentSearches();
  const last = recent[0];
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
      <h1>{interpolate(dict.heading, { asin })}</h1>
      <p className="product-header__asin">
        <span className="t-label">{dict.asinLabel}</span> <span className="t-num">{asin}</span>
      </p>
    </header>
  );
}
