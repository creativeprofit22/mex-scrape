"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { localeCookie, localizedPath, type Locale } from "@/i18n/config";
import { api } from "@/lib/api/client";

const ONE_YEAR_SECONDS = 60 * 60 * 24 * 365;

/** Switches to `target`, keeping the current page, and remembers the choice. */
export function LanguageToggle({
  target,
  label,
  title,
}: {
  target: Locale;
  label: string;
  title: string;
}) {
  const pathname = usePathname();

  function remember() {
    document.cookie = `${localeCookie}=${target}; path=/; max-age=${ONE_YEAR_SECONDS}; samesite=lax`;
    // Saved on the account too when signed in; signed out it answers 401.
    void api
      .PUT("/me/locale", { body: { locale: target } })
      .catch(() => undefined);
  }

  return (
    <Link
      href={localizedPath(pathname, target)}
      hrefLang={target}
      lang={target}
      title={title}
      onClick={remember}
    >
      {label}
    </Link>
  );
}
