import Link from "next/link";

import type { Locale } from "@/i18n/config";

/** The product name, linking to the home page. */
export function Wordmark({ lang, label }: { lang: Locale; label: string }) {
  return (
    <Link className="wordmark" href={`/${lang}`}>
      {label}
    </Link>
  );
}
