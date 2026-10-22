"use client";

import Link from "next/link";
import { useId } from "react";

import type { Locale } from "@/i18n/config";
import { useRecentSearches } from "@/lib/search/useSearchStore";

/** Query parameter that reruns a search on arrival (then is removed). */
export const RERUN_PARAM = "q";

/** Search page link that reruns `query` (see SearchPanel). */
export function rerunHref(lang: Locale, query: string): string {
  return `/${lang}/search?${RERUN_PARAM}=${encodeURIComponent(query)}`;
}

/**
 * Searches saved in this browser, newest first; each link reruns one.
 * Renders nothing until something is saved (and on the server).
 */
export function RecentSearches({
  lang,
  heading,
  className,
}: {
  lang: Locale;
  heading: string;
  className?: string;
}) {
  const id = useId();
  const [recent] = useRecentSearches();
  if (recent.length === 0) return null;
  return (
    <nav className={["recent-searches", className].filter(Boolean).join(" ")} aria-labelledby={id}>
      <h2 id={id} className="recent-searches__heading">
        {heading}
      </h2>
      <ul className="recent-searches__list">
        {recent.map((query) => (
          <li key={query}>
            <Link href={rerunHref(lang, query)}>{query}</Link>
          </li>
        ))}
      </ul>
    </nav>
  );
}
