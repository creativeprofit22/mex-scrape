import { Suspense } from "react";

import { LanguageToggle } from "@/components/LanguageToggle";
import type { Locale } from "@/i18n/config";
import type { Dictionary } from "@/i18n/dictionaries";

/**
 * The language switch for the frame: keeps the current page, sets the cookie
 * and saves the choice with `PUT /me/locale`. Until the URL is known it is a
 * plain link to the other language's home.
 */
export function FrameLanguage({ lang, nav }: { lang: Locale; nav: Dictionary["nav"] }) {
  const other = lang === "es" ? "en" : "es";
  return (
    <Suspense
      fallback={
        <a
          className="language-toggle"
          href={`/${other}`}
          hrefLang={other}
          lang={other}
          title={nav.switchLanguageLabel}
        >
          {nav.switchLanguage}
        </a>
      }
    >
      <LanguageToggle
        target={other}
        label={nav.switchLanguage}
        title={nav.switchLanguageLabel}
      />
    </Suspense>
  );
}
